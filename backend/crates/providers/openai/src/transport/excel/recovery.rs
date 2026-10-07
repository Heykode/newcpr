//! Single-text BPS recovery semantics aligned with Sub2API production@9f7d96be.
//! Buffer only trusted recovery probes: normal user streams must remain live.

use bytes::BytesMut;
use futures::StreamExt;
use gateway_protocol::openai::sse::SseError;
use serde::Deserialize;
use serde_json::Value;

use crate::transport::{CodexBackendSseStream, CodexClientError};

pub(super) const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_LINE_BYTES: usize = 1024 * 1024;

pub(crate) fn verify_stream(
    mut source: CodexBackendSseStream,
    nonce: String,
) -> CodexBackendSseStream {
    Box::pin(async_stream::try_stream! {
        let mut wire = BytesMut::new();
        while let Some(chunk) = source.next().await {
            let chunk = chunk?;
            if chunk.len() > MAX_BODY_BYTES.saturating_sub(wire.len()) {
                Err(invalid("response exceeds 2 MiB"))?;
            }
            wire.extend_from_slice(&chunk);
        }
        // Do not release a terminal until EOF: later failures, duplicate completions
        // and transport errors must not be hidden by the ordinary terminal adapter.
        validate(&wire, &nonce)?;
        yield wire.freeze();
    })
}

#[derive(Deserialize)]
struct ProbeEvent {
    #[serde(rename = "type", default)]
    kind: String,
    response: Option<Value>,
}

fn validate(wire: &[u8], nonce: &str) -> Result<(), CodexClientError> {
    let mut completed = None;
    for line in wire.split(|byte| *byte == b'\n') {
        if line.len() >= MAX_LINE_BYTES {
            return Err(invalid("SSE line exceeds 1 MiB"));
        }
        let Some(data) = line.strip_prefix(b"data:") else {
            continue;
        };
        let data = std::str::from_utf8(data)
            .map_err(|_| invalid("invalid SSE encoding"))?
            .trim();
        if data == "[DONE]" {
            continue;
        }
        let event: ProbeEvent =
            serde_json::from_str(data).map_err(|_| invalid("invalid SSE event"))?;
        match event.kind.as_str() {
            "response.failed" | "response.incomplete" | "response.cancelled" | "error" => {
                return Err(invalid("response did not complete normally"));
            }
            "response.completed" => {
                if completed.is_some() {
                    return Err(invalid("duplicate completion"));
                }
                let response = event.response.ok_or_else(|| invalid("missing response"))?;
                if response.get("status").and_then(Value::as_str) != Some("completed") {
                    return Err(invalid("invalid completion status"));
                }
                completed = Some(response);
            }
            _ => {}
        }
    }
    let response = completed.ok_or_else(|| invalid("missing completion"))?;
    if exact_text(&response, nonce) {
        Ok(())
    } else {
        Err(invalid("response is not the exact assistant nonce text"))
    }
}

fn exact_text(response: &Value, nonce: &str) -> bool {
    let Some(output) = response.get("output").and_then(Value::as_array) else {
        return false;
    };
    let mut text = String::new();
    let mut saw_text = false;
    for item in output {
        if item.get("type").and_then(Value::as_str) == Some("reasoning") {
            continue;
        }
        if item.get("type").and_then(Value::as_str) != Some("message")
            || item.get("role").and_then(Value::as_str) != Some("assistant")
        {
            return false;
        }
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            return false;
        };
        for part in content {
            if part.get("type").and_then(Value::as_str) != Some("output_text") {
                return false;
            }
            let Some(value) = part.get("text").and_then(Value::as_str) else {
                return false;
            };
            saw_text = true;
            text.push_str(value);
        }
    }
    saw_text && text.trim() == nonce
}

fn invalid(reason: &'static str) -> CodexClientError {
    CodexClientError::InvalidSse(SseError::ParseError(format!(
        "Excel recovery probe: {reason}"
    )))
}
