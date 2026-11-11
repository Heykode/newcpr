//! Official Messages wire framing and non-streaming accumulation.
use super::{messages, responses::ResponseEncodeError};
use bytes::Bytes;
use gateway_core::event::ProviderEvent;
use gateway_protocol::anthropic::ANTHROPIC_PROTOCOL;
use gateway_protocol::{
    anthropic::MessagesAccumulator, openai::sse::encode_sse_event_with_metadata,
};
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct MessagesEncoder {
    response_id: Option<String>,
    completed: bool,
    failed: bool,
}
impl MessagesEncoder {
    pub(super) fn push_sse(&mut self, event: &ProviderEvent) -> Vec<Bytes> {
        let Some(wire) = event
            .wire_event()
            .filter(|wire| wire.protocol() == ANTHROPIC_PROTOCOL)
        else {
            return Vec::new();
        };
        if let Some(raw) = wire.raw_json_body() {
            let events = serde_json::from_slice(raw).ok().and_then(|value| {
                gateway_protocol::anthropic::message_to_stream_events(value).ok()
            });
            let Some(events) = events else {
                self.failed = true;
                return vec![messages::stream_error(
                    &json!({"error":{"message":"The upstream message is invalid."}}),
                )];
            };
            self.response_id = events
                .first()
                .and_then(|value| value.pointer("/message/id"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            self.completed = true;
            return events
                .into_iter()
                .map(|value| {
                    Bytes::from(encode_sse_event_with_metadata(
                        value["type"].as_str().unwrap_or_default(),
                        &value.to_string(),
                        None,
                        None,
                    ))
                })
                .collect();
        }
        let kind = wire
            .event_type()
            .or_else(|| wire.data().get("type").and_then(Value::as_str));
        match kind {
            Some("message_start") => {
                self.response_id = wire
                    .data()
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }
            Some("message_stop") => self.completed = true,
            Some("error") => self.failed = true,
            _ => {}
        }
        if let Some(raw) = wire.raw_sse_frame() {
            return vec![raw.clone()];
        }
        vec![Bytes::from(encode_sse_event_with_metadata(
            kind.unwrap_or_default(),
            &wire.data().to_string(),
            wire.sse_id(),
            wire.sse_retry(),
        ))]
    }
    pub(super) fn is_completed(&self) -> bool {
        self.completed
    }
    pub(super) fn has_wire_failure(&self) -> bool {
        self.failed
    }
}
pub(super) fn collect(
    events: &[ProviderEvent],
    counted: bool,
) -> Result<Value, ResponseEncodeError> {
    let mut accumulator = MessagesAccumulator::default();
    let mut response = None;
    for wire in events
        .iter()
        .filter_map(ProviderEvent::wire_event)
        .filter(|wire| wire.protocol() == ANTHROPIC_PROTOCOL)
    {
        if let Some(raw) = wire.raw_json_body() {
            return serde_json::from_slice(raw).map_err(|_| ResponseEncodeError::Serialization);
        }
        let value = wire.data();
        if counted {
            if value.get("input_tokens").and_then(Value::as_u64).is_some() {
                response = Some(value.clone());
            }
        } else if value.get("type").and_then(Value::as_str) == Some("message") {
            response = Some(value.clone());
        } else if wire.has_json_data()
            && let Some(message) = accumulator
                .push(value)
                .map_err(|_| ResponseEncodeError::InvalidOutputItems)?
        {
            response = Some(message);
        }
    }
    response.ok_or(ResponseEncodeError::MissingWireTerminal)
}
