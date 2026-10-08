//! Excel-only framing and terminal aliases; native Codex retains its SSE contract.

use gateway_protocol::openai::sse::{
    MAX_SSE_EVENT_BUFFER_BYTES, SseError, SseEvent, SseEventDecoder,
};
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct ResponseDecoder {
    sse: SseEventDecoder,
    json: Option<bool>,
    pending: Vec<u8>,
    scanned: usize,
    depth: usize,
    quoted: bool,
    escaped: bool,
    complete: bool,
}

impl ResponseDecoder {
    pub(super) fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, SseError> {
        if self.json == Some(false) {
            return self.sse.push(chunk);
        }
        if self.complete {
            return if chunk.iter().all(u8::is_ascii_whitespace) {
                Ok(Vec::new())
            } else {
                Err(invalid(
                    "Excel returned trailing data after a JSON response",
                ))
            };
        }
        if self.pending.len().saturating_add(chunk.len()) > MAX_SSE_EVENT_BUFFER_BYTES {
            return Err(SseError::BufferExceeded {
                max_bytes: MAX_SSE_EVENT_BUFFER_BYTES,
            });
        }
        self.pending.extend_from_slice(chunk);
        if self.json.is_none() {
            while self.scanned < self.pending.len() {
                let byte = self.pending[self.scanned];
                if byte.is_ascii_whitespace() {
                    self.scanned += 1;
                    continue;
                }
                if byte == 0xef {
                    let tail = &self.pending[self.scanned..];
                    if tail.len() < 3 && b"\xef\xbb\xbf".starts_with(tail) {
                        return Ok(Vec::new());
                    }
                    if tail.starts_with(b"\xef\xbb\xbf") {
                        self.scanned += 3;
                        continue;
                    }
                }
                self.json = Some(byte == b'{');
                break;
            }
            if self.json.is_none() {
                return Ok(Vec::new());
            }
            self.pending.drain(..self.scanned);
            self.scanned = 0;
            if self.json == Some(false) {
                let prefix = std::mem::take(&mut self.pending);
                return self.sse.push(&prefix);
            }
        }
        // Scan framing once per byte. serde_json validates the complete value;
        // reparsing every chunk would be quadratic for large tool arguments.
        while self.scanned < self.pending.len() {
            let byte = self.pending[self.scanned];
            self.scanned += 1;
            if self.quoted {
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.quoted = false;
                }
                continue;
            }
            match byte {
                b'"' => self.quoted = true,
                b'{' | b'[' => self.depth += 1,
                b'}' | b']' => {
                    self.depth = self
                        .depth
                        .checked_sub(1)
                        .ok_or_else(|| invalid("Excel returned malformed response JSON"))?;
                    if self.depth == 0 {
                        if !self.pending[self.scanned..]
                            .iter()
                            .all(u8::is_ascii_whitespace)
                        {
                            return Err(invalid(
                                "Excel returned trailing data after a JSON response",
                            ));
                        }
                        let data = std::str::from_utf8(&self.pending[..self.scanned])
                            .map_err(|_| invalid("Excel returned invalid response encoding"))?;
                        let event = SseEvent {
                            event: None,
                            data: data.into(),
                            id: None,
                            retry: None,
                        };
                        self.pending.clear();
                        self.complete = true;
                        return Ok(vec![event]);
                    }
                }
                _ => {}
            }
        }
        Ok(Vec::new())
    }

    pub(super) fn finish(&mut self) -> Result<Vec<SseEvent>, SseError> {
        match self.json {
            Some(false) => self.sse.finish(),
            Some(true) if !self.complete => Err(invalid("Excel response JSON was truncated")),
            None if self.scanned != self.pending.len() => {
                Err(invalid("Excel response prefix was truncated"))
            }
            _ => Ok(Vec::new()),
        }
    }
}

fn invalid(message: &str) -> SseError {
    SseError::ParseError(message.into())
}

fn terminal(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "response.completed" | "response.done" | "done" | "completed" => "response.completed",
        "response.failed" | "failed" => "response.failed",
        "response.incomplete" | "incomplete" => "response.incomplete",
        "response.cancelled" | "response.canceled" | "cancelled" | "canceled" => {
            "response.cancelled"
        }
        "response.error" | "error" => "error",
        _ => return None,
    })
}

pub(super) fn normalize(mut event: SseEvent) -> Result<SseEvent, SseError> {
    if event.data.trim() == "[DONE]" {
        event.data = "[DONE]".into();
        return Ok(event);
    }
    let mut value: Value = serde_json::from_str(&event.data)
        .map_err(|_| invalid("Excel returned malformed event JSON"))?;
    if !value.is_object() {
        return Err(invalid("Excel returned a non-object event"));
    }
    let typed = value
        .get("type")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let header = event.event.as_deref().unwrap_or_default();
    let declared = typed.unwrap_or(header);
    let nested = value.get("response").filter(|v| v.is_object());
    let response = nested.unwrap_or(&value);
    let status = response.get("status").and_then(Value::as_str);
    let has_error = value.get("error").is_some_and(|v| !v.is_null())
        || response.get("error").is_some_and(|v| !v.is_null());
    let terminals = [
        terminal(declared),
        terminal(header),
        status.and_then(terminal),
    ];
    let kind = if terminals.contains(&Some("error")) {
        Some("error")
    } else if terminals[..2].contains(&Some("response.cancelled")) {
        Some("response.cancelled")
    } else if has_error || terminals.contains(&Some("response.failed")) {
        Some("response.failed")
    } else if terminals.contains(&Some("response.cancelled")) {
        Some("response.cancelled")
    } else if terminals.contains(&Some("response.incomplete"))
        || response
            .get("incomplete_details")
            .is_some_and(|v| !v.is_null())
    {
        Some("response.incomplete")
    } else {
        terminal(declared).or_else(|| {
            (matches!(declared, "" | "response") && status == Some("completed"))
                .then_some("response.completed")
        })
    };
    let Some(kind) = kind else {
        return Ok(event);
    };
    if kind == "response.completed"
        && (response
            .get("status")
            .is_some_and(|s| !s.is_null() && s.as_str() != Some("completed"))
            || (value.get("response").is_some() && nested.is_none())
            || response
                .get("id")
                .and_then(Value::as_str)
                .is_none_or(|s| s.trim().is_empty())
            || !response.get("output").is_some_and(Value::is_array))
    {
        return Err(invalid(
            "Excel terminal response lacks a completed response body",
        ));
    }
    // Flat terminal bodies are wrapped without dropping IDs, usage or opaque fields.
    // Errors lacking a response body stay top-level for the existing sanitizer.
    if nested.is_none()
        && (kind == "response.completed"
            || value.get("id").is_some()
            || value.get("output").is_some())
    {
        value = json!({"response":value});
    }
    if let Some(response) = value.get_mut("response").filter(|v| v.is_object()) {
        response["status"] = match kind {
            "response.completed" => "completed",
            "response.incomplete" => "incomplete",
            "response.cancelled" => "cancelled",
            _ => "failed",
        }
        .into();
    }
    value["type"] = kind.into();
    event.event = Some(kind.into());
    event.data = value.to_string();
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response() -> Value {
        json!({"id":"resp_fixture","object":"response","status":"completed",
            "output":[{"type":"message","content":[{"type":"output_text","text":"[braces] } \\\" \n \u{4e2d}\u{6587}"}]}],
            "usage":{"input_tokens":17,"output_tokens":3},"opaque":9007199254740993u64})
    }

    fn decode(wire: &[u8], size: usize) -> Result<Vec<Value>, SseError> {
        let mut decoder = ResponseDecoder::default();
        let mut events = Vec::new();
        for chunk in wire.chunks(size) {
            events.extend(decoder.push(chunk)?);
        }
        events.extend(decoder.finish()?);
        events
            .into_iter()
            .filter(|event| event.data != "[DONE]")
            .map(|event| {
                normalize(event).and_then(|event| {
                    serde_json::from_str(&event.data).map_err(|_| invalid("invalid test JSON"))
                })
            })
            .collect()
    }

    #[test]
    fn json_framing_preserves_strings_unicode_large_numbers_and_opaque_fields() {
        let response = response();
        for prefix in [&b""[..], &b"\xef\xbb\xbf"[..], &b" \r\n\t"[..]] {
            let mut wire = prefix.to_vec();
            wire.extend_from_slice(serde_json::to_string_pretty(&response).unwrap().as_bytes());
            wire.extend_from_slice(b" \n");
            for size in [1, 2, 3, 7, wire.len()] {
                let events = decode(&wire, size).unwrap();
                assert_eq!(
                    events,
                    vec![json!({"type":"response.completed","response":response})]
                );
            }
        }
    }

    #[test]
    fn sse_multiline_bom_crlf_and_unterminated_tail_use_existing_parser() {
        let value = json!({"response":response()});
        let mut wire = vec![0xef, 0xbb, 0xbf];
        wire.extend_from_slice(
            b": heartbeat\r\n\r\nevent: response.done\r\nid: fixture\r\nretry: 1000\r\n",
        );
        for line in serde_json::to_string_pretty(&value).unwrap().lines() {
            wire.extend_from_slice(format!("data: {line}\r\n").as_bytes());
        }
        for size in [1, 3, 17, wire.len()] {
            assert_eq!(
                decode(&wire, size).unwrap(),
                vec![json!({"type":"response.completed","response":response()})]
            );
        }
    }

    #[test]
    fn terminal_alias_requires_complete_shape_and_preserves_failure_precedence() {
        for failure in [
            "failed",
            "incomplete",
            "cancelled",
            "canceled",
            "error",
            "response.error",
        ] {
            for in_header in [false, true] {
                let mut value = json!({"type":"response.done","response":response()});
                if !in_header {
                    value["response"]["status"] = failure.into();
                }
                let wire = format!(
                    "event: {}\ndata: {}\n\n",
                    if in_header {
                        failure
                    } else {
                        "response.completed"
                    },
                    value
                );
                let events = decode(wire.as_bytes(), 3).unwrap();
                assert_ne!(events[0]["type"], "response.completed");
            }
        }
        for patch in [
            json!({"status":12}),
            json!({"status":"in_progress"}),
            json!({"output":null}),
            json!({"id":""}),
        ] {
            let mut body = response();
            body.as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            let wire = json!({"type":"response.done","response":body}).to_string();
            assert!(decode(wire.as_bytes(), 1).is_err());
        }
        let mut missing_status = response();
        missing_status.as_object_mut().unwrap().remove("status");
        let explicit = json!({"type":"response.done","response":missing_status});
        assert_eq!(
            decode(explicit.to_string().as_bytes(), 1).unwrap()[0]["type"],
            "response.completed"
        );
        assert_ne!(
            decode(missing_status.to_string().as_bytes(), 1).unwrap()[0]["type"],
            "response.completed"
        );
    }

    #[test]
    fn errors_truncation_and_unknown_events_never_become_completed() {
        for wire in [
            "{",
            "{\"output\":[}",
            "{\"text\":\"unterminated",
            "{}{}",
            "{}garbage",
            "data: [1]\n\n",
            "data: {invalid}\n\n",
        ] {
            assert!(decode(wire.as_bytes(), wire.len()).is_err(), "{wire}");
        }
        for patch in [
            json!({"error":{"code":"invalid_request"}}),
            json!({"incomplete_details":{"reason":"max_output_tokens"}}),
        ] {
            let mut body = response();
            body.as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert_ne!(
                decode(body.to_string().as_bytes(), 3).unwrap()[0]["type"],
                "response.completed"
            );
        }
        let unknown = json!({"type":"response.future","response":response()});
        assert_eq!(
            decode(format!("data: {unknown}\n\n").as_bytes(), 1).unwrap(),
            vec![unknown]
        );
        assert!(
            decode(b": keepalive\n\ndata: [DONE]\n\n", 1)
                .unwrap()
                .is_empty()
        );
        assert!(decode(b" \r\n\t", 1).unwrap().is_empty());
    }
}
