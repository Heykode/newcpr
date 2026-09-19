use gateway_core::{
    error::{ProviderError, ProviderErrorKind},
    upstream::UpstreamSendState,
};
use gateway_protocol::openai::sse::{SseEvent, SseEventDecoder};
use serde_json::Value;

use crate::transport::{canonical::CodexCanonicalError, protocol::responses::ResponsesSseFailure};

const MAX_PROBE_RESPONSE_BYTES: usize = 1024 * 1024;

/// Maintenance needs completion and failures, not content, billing or model projection.
#[derive(Default)]
pub(super) struct ProbeResponse {
    decoder: SseEventDecoder,
    bytes: usize,
    completed: bool,
}

impl ProbeResponse {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<(), CodexCanonicalError> {
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > MAX_PROBE_RESPONSE_BYTES {
            return Err(protocol_error());
        }
        let events = self.decoder.push(bytes).map_err(|_| protocol_error())?;
        self.observe(events)
    }

    pub(super) fn finish(&mut self) -> Result<bool, CodexCanonicalError> {
        let events = self.decoder.finish().map_err(|_| protocol_error())?;
        self.observe(events)?;
        Ok(self.completed)
    }

    fn observe(&mut self, events: Vec<SseEvent>) -> Result<(), CodexCanonicalError> {
        for event in events {
            if event.data.trim() == "[DONE]" {
                continue;
            }
            let value: Value = serde_json::from_str(&event.data).map_err(|_| protocol_error())?;
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .or(event.event.as_deref());
            let status = value.pointer("/response/status").and_then(Value::as_str);
            if matches!(
                kind,
                Some("response.failed" | "error" | "response.incomplete")
            ) || matches!(status, Some("failed" | "incomplete"))
                || value.get("error").is_some_and(|error| !error.is_null())
                || value
                    .pointer("/response/error")
                    .is_some_and(|error| !error.is_null())
            {
                return Err(CodexCanonicalError::Upstream(Box::new(
                    ResponsesSseFailure::from_raw_event(
                        kind.unwrap_or("error"),
                        &event.data,
                        &value,
                    ),
                )));
            }
            if kind == Some("response.completed") {
                self.completed = true;
            }
        }
        Ok(())
    }
}

fn protocol_error() -> CodexCanonicalError {
    CodexCanonicalError::Protocol(
        ProviderError::new(ProviderErrorKind::Protocol, UpstreamSendState::Sent)
            .redact_sensitive_context("invalid or oversized state probe stream"),
    )
}
