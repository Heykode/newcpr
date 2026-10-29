//! Safe update messages keep URL-free transport causes in operational diagnostics.

use super::{OperationError, upstream};

pub(super) fn http(message: &'static str, source: reqwest::Error) -> OperationError {
    let failure = upstream(message);
    let failure = if source.url().is_some() {
        failure.redact_sensitive_context()
    } else {
        failure
    };
    failure.with_source(source.without_url())
}
