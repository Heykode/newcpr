//! Chat Completions compatibility over the existing Responses executor.

use axum::body::Bytes;
use serde_json::Value;

mod http;
pub(in crate::openai) mod request;
pub(in crate::openai) mod response;

pub(crate) use http::chat_completions;

pub(in crate::openai) fn chat_error_frame(data: &Value) -> Bytes {
    let error = data
        .pointer("/response/error")
        .or_else(|| data.get("error"));
    Bytes::from(format!("data: {}\n\n", serde_json::json!({"error": error})))
}
