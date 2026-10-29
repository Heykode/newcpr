//! Pure body and header compatibility; identity and transport stay with their owners.

mod compatibility;
mod headers;
mod request;
mod search_history;

pub(crate) use compatibility::*;
pub(crate) use headers::provider_managed_header;
pub(crate) use request::{normalize_codex_request_body, normalize_reasoning_replay};
pub(crate) use search_history::normalize_search_history;
