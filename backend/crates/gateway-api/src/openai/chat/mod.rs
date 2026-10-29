//! Chat Completions compatibility over the existing Responses executor.

mod http;
pub(in crate::openai) mod request;
pub(in crate::openai) mod response;

pub(crate) use http::chat_completions;
