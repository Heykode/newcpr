//! Official Responses-to-Messages converter, owned by one execution stream.
use super::*;
use gateway_core::engine::provider::NativeResponseTranslator;
use gateway_protocol::anthropic::{ANTHROPIC_PROTOCOL, ResponsesToMessagesStream};

pub(super) struct ResponsesResponseTranslator {
    stream: ResponsesToMessagesStream,
}

impl ResponsesResponseTranslator {
    pub(super) fn new(original: &Map<String, Value>) -> Self {
        Self {
            stream: ResponsesToMessagesStream::new(original),
        }
    }
}

impl NativeResponseTranslator for ResponsesResponseTranslator {
    fn source_protocol(&self) -> &str {
        PROVIDER_NAME
    }
    fn target_protocol(&self) -> &str {
        ANTHROPIC_PROTOCOL
    }
    fn translate(
        &mut self,
        event: &ProtocolWireEvent,
    ) -> Result<Vec<ProtocolWireEvent>, ProviderError> {
        if let Some(raw) = event.raw_json_body() {
            let response = serde_json::from_slice(raw).map_err(|_| {
                provider_error(ProviderErrorKind::Protocol, UpstreamSendState::Sent)
            })?;
            let message = self.stream.complete_json(&response).map_err(|_| {
                provider_error(ProviderErrorKind::Protocol, UpstreamSendState::Sent)
            })?;
            let raw = serde_json::to_vec(&message).map_err(|_| {
                provider_error(ProviderErrorKind::Protocol, UpstreamSendState::Sent)
            })?;
            return Ok(vec![
                ProtocolWireEvent::raw_json(ANTHROPIC_PROTOCOL, raw.into()).map_err(|_| {
                    provider_error(ProviderErrorKind::Protocol, UpstreamSendState::Sent)
                })?,
            ]);
        }
        if !event.has_json_data() {
            return Ok(Vec::new());
        }
        let events = self
            .stream
            .push(event.data())
            .map_err(|_| provider_error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
        if matches!(
            event.data().get("type").and_then(Value::as_str),
            Some("response.completed" | "response.incomplete")
        ) {
            self.stream.finish().map_err(|_| {
                provider_error(ProviderErrorKind::Protocol, UpstreamSendState::Sent)
            })?;
        }
        wire_events(ANTHROPIC_PROTOCOL, events)
    }
}

fn wire_events(
    protocol: &str,
    events: Vec<Value>,
) -> Result<Vec<ProtocolWireEvent>, ProviderError> {
    events
        .into_iter()
        .map(|value| {
            ProtocolWireEvent::json(
                protocol,
                value.get("type").and_then(Value::as_str).map(str::to_owned),
                value,
            )
            .map_err(|_| provider_error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))
        })
        .collect()
}

/// Official source-header filtering for the Messages -> Responses boundary.
pub(super) fn retain_target_headers(context: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(entries) = context
        .get_mut(crate::transport::request::PASSTHROUGH_HEADERS_CONTEXT_KEY)
        .and_then(serde_json::Value::as_array_mut)
    {
        entries.retain(|entry| {
            entry
                .as_array()
                .and_then(|entry| entry.first())
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| {
                    let name = name.to_ascii_lowercase();
                    !name.starts_with("anthropic-") && !name.starts_with("x-anthropic-")
                })
        });
    }
}
