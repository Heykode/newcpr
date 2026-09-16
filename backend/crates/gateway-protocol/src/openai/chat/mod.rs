//! Pure JSON conversion. Transport, authentication and accounting stay with the caller.

mod messages;
mod request;
mod response;
mod stream;
mod tools;

pub use request::{DecodedChatRequest, decode_chat_request};
pub use response::{chat_response, chat_response_from_events};
pub use stream::ChatStreamEncoder;

use serde_json::{Map, Value};

/// A deliberately payload-free error, safe for downstream error envelopes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ChatConversionError {
    message: &'static str,
    param: Option<String>,
    code: &'static str,
}

impl ChatConversionError {
    pub fn message(&self) -> &str {
        self.message
    }

    pub fn param(&self) -> Option<&str> {
        self.param.as_deref()
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    fn invalid(param: impl Into<String>) -> Self {
        Self {
            message: "Invalid Chat Completions parameter.",
            param: Some(param.into()),
            code: "invalid_request_error",
        }
    }

    fn unsupported(param: impl Into<String>) -> Self {
        Self {
            message: "This parameter cannot be represented by the Responses backend.",
            param: Some(param.into()),
            code: "unsupported_parameter",
        }
    }

    fn output(param: impl Into<String>) -> Self {
        Self {
            message: "The upstream response cannot be represented as Chat Completions.",
            param: Some(param.into()),
            code: "invalid_upstream_response",
        }
    }

    fn failed() -> Self {
        Self {
            message: "The upstream response failed.",
            param: None,
            code: "upstream_error",
        }
    }
}

type Result<T> = std::result::Result<T, ChatConversionError>;

fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| ChatConversionError::invalid(path))
}

fn string<'a>(value: &'a Value, path: &str) -> Result<&'a str> {
    value
        .as_str()
        .ok_or_else(|| ChatConversionError::invalid(path))
}

fn nonempty<'a>(value: &'a Value, path: &str) -> Result<&'a str> {
    let text = string(value, path)?;
    if text.trim().is_empty() {
        return Err(ChatConversionError::invalid(path));
    }
    Ok(text)
}

fn only_keys(value: &Map<String, Value>, allowed: &[&str], path: &str) -> Result<()> {
    if value.keys().any(|key| !allowed.contains(&key.as_str())) {
        // Unknown keys are caller controlled. Do not reflect arbitrary key text.
        return Err(ChatConversionError::unsupported(if path.is_empty() {
            "request"
        } else {
            path
        }));
    }
    Ok(())
}
