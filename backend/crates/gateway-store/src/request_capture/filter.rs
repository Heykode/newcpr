//! Structured redaction occurs before any body is written to disk.

use gateway_core::diagnostics::body_fingerprint;
use serde_json::{Value, json};

const MAX_FRAME: usize = 8 * 1024 * 1024;

pub(super) fn redact(mut value: Value, media: bool) -> Value {
    visit(&mut value, "", media, 0, &mut 65_536);
    value
}

fn visit(value: &mut Value, key: &str, media: bool, depth: usize, nodes: &mut usize) {
    let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
    if depth > 64 || *nodes == 0 {
        *value = json!({"omitted":"structure_limit"});
        return;
    }
    *nodes -= 1;
    if normalized.contains("authorization")
        || normalized.contains("cookie")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.contains("apikey")
        || normalized.contains("accesstoken")
        || normalized.contains("refreshtoken")
        || normalized == "idtoken"
        || matches!(
            normalized.as_str(),
            "token" | "credentials" | "credential" | "bearer"
        )
        || normalized.contains("attestation")
        || normalized == "headers"
        || normalized == "xoaiis"
        || normalized == "xoaiisupdate"
    {
        *value = "<redacted>".into();
        return;
    }
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                visit(value, name, media, depth + 1, nodes);
            }
        }
        Value::Array(items) => {
            for value in items {
                visit(value, key, media, depth + 1, nodes);
            }
        }
        Value::String(text) => {
            let is_media = text.starts_with("data:")
                || matches!(
                    normalized.as_str(),
                    "b64json" | "imagedata" | "audiodata" | "videodata" | "base64"
                );
            if is_media && !media {
                *value = body_fingerprint(text.as_bytes());
            } else if text.contains("://") {
                // Keep ordinary text, but never retain URL userinfo or signed query strings.
                *text = redact_urls(text);
            }
        }
        _ => {}
    }
}

fn redact_urls(text: &str) -> String {
    text.split_inclusive(char::is_whitespace)
        .map(|part| {
            if part.contains("://") {
                // Also covers multiple embedded/escaped URLs in one tool argument.
                // Paths can themselves contain credentials, not just query/userinfo.
                let trailing = &part[part.trim_end().len()..];
                format!("<redacted-url>{trailing}")
            } else {
                part.to_owned()
            }
        })
        .collect()
}

/// Reassembles chunked JSON/SSE without trusting Content-Type or a truncated prefix.
pub(super) struct BodyFilter {
    bytes: Vec<u8>,
    overflow: bool,
}
impl BodyFilter {
    pub(super) fn new() -> Self {
        Self {
            bytes: Vec::new(),
            overflow: false,
        }
    }
    pub(super) fn push(&mut self, bytes: &[u8]) {
        if self.overflow {
            return;
        }
        if bytes.len() > MAX_FRAME.saturating_sub(self.bytes.len()) {
            self.bytes.clear();
            self.overflow = true;
        } else {
            self.bytes.extend_from_slice(bytes);
        }
    }
    pub(super) fn finish(self, media: bool) -> (Vec<Value>, bool) {
        if self.overflow {
            return (vec![json!({"omitted":"body_size_limit"})], true);
        }
        if let Ok(value) = serde_json::from_slice::<Value>(&self.bytes) {
            return (vec![redact(value, media)], false);
        }
        let Ok(text) = std::str::from_utf8(&self.bytes) else {
            return (
                vec![json!({"omitted":"unsupported_body","body":body_fingerprint(&self.bytes)})],
                true,
            );
        };
        let mut values = Vec::new();
        let mut data = String::new();
        let mut incomplete = false;
        let mut saw_data = false;
        for line in text.lines().chain(std::iter::once("")) {
            if line.is_empty() {
                if !data.is_empty() {
                    saw_data = true;
                    if data.trim() == "[DONE]" {
                        values.push(json!({"sseDone":true}));
                    } else if let Ok(value) = serde_json::from_str::<Value>(&data) {
                        values.push(redact(value, media));
                    } else {
                        values.push(json!({"omitted":"unparseable_event","body":body_fingerprint(data.as_bytes())}));
                        incomplete = true;
                    }
                    data.clear();
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
        if !saw_data {
            return (
                vec![json!({"omitted":"unparseable_body","body":body_fingerprint(&self.bytes)})],
                true,
            );
        }
        (values, incomplete)
    }
}
