//! WebSocket aggregate/stream 共用事件归约器。

use gateway_protocol::openai::events;
use serde_json::Value;

use crate::transport::protocol::websocket::{
    websocket_event_frame, websocket_event_type, websocket_metadata_headers,
    websocket_metadata_turn_state, websocket_response_completed_id,
    websocket_response_is_interrupted,
};
use crate::transport::response_meta;

use super::super::pool::{CodexWebSocketConnectionMetadata, WebSocketContinuationState};
use super::CodexWebSocketExchangeError;

const MAX_CONCATENATED_EVENT_DOCUMENTS: usize = 16;
const MAX_CONCATENATED_EVENT_BYTES: usize = 16 * 1024 * 1024;

/// Preserve the single-parse fast path; validate a complete repair before delivery.
pub(super) fn parse_websocket_event_documents(
    raw: &str,
) -> Result<impl Iterator<Item = (&str, Value)>, CodexWebSocketExchangeError> {
    let (single, recovered) = if let Ok(value) = serde_json::from_str::<Value>(raw) {
        (Some((raw, value)), Vec::new())
    } else {
        if raw.len() > MAX_CONCATENATED_EVENT_BYTES {
            return Err(CodexWebSocketExchangeError::InvalidEventJson);
        }
        let mut decoder = serde_json::Deserializer::from_str(raw).into_iter::<Value>();
        let mut recovered = Vec::new();
        loop {
            let start = decoder.byte_offset();
            let Some(value) = decoder.next() else { break };
            let value = value.map_err(|_| CodexWebSocketExchangeError::InvalidEventJson)?;
            let event_type = value.get("type").and_then(Value::as_str).map(str::trim);
            if recovered.len() == MAX_CONCATENATED_EVENT_DOCUMENTS
                || !event_type.is_some_and(|kind| !kind.is_empty() && !kind.contains(['\r', '\n']))
            {
                return Err(CodexWebSocketExchangeError::InvalidEventJson);
            }
            recovered.push((raw[start..decoder.byte_offset()].trim(), value));
        }
        if recovered.len() < 2 {
            return Err(CodexWebSocketExchangeError::InvalidEventJson);
        }
        (None, recovered)
    };
    Ok(single.into_iter().chain(recovered))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::transport::websocket) enum WebSocketTerminalKind {
    Completed,
    Interrupted,
    Incomplete,
    Failed,
}

pub(super) enum ExchangeAction {
    RateLimits(events::ParsedRateLimits),
    Forward {
        frame: String,
        terminal: Option<WebSocketTerminalKind>,
    },
    Ignore,
}

pub(super) struct ReducedWebSocketEvent {
    pub(super) created_response_id: Option<String>,
    pub(super) action: ExchangeAction,
    pub(super) diagnostic_event_type: Option<String>,
    pub(super) turn_state_update: Option<String>,
    pub(super) error_rate_limits: Option<events::ParsedRateLimits>,
}

pub(super) fn reduce_websocket_event(
    raw: &str,
    value: &Value,
    metadata: &mut CodexWebSocketConnectionMetadata,
    continuation: &mut WebSocketContinuationState,
) -> Result<ReducedWebSocketEvent, CodexWebSocketExchangeError> {
    let diagnostic_event_type = diagnostic_event_type(websocket_event_type(value));
    if let Some(parsed) = events::parse_rate_limits_event(value) {
        let headers = events::rate_limits_to_header_pairs(&parsed);
        metadata.rate_limit_headers.extend(headers);
        return Ok(ReducedWebSocketEvent {
            action: ExchangeAction::RateLimits(parsed),
            created_response_id: None,
            diagnostic_event_type,
            turn_state_update: None,
            error_rate_limits: None,
        });
    }

    response_meta::merge_response_metadata(
        &mut metadata.response_metadata,
        websocket_metadata_headers(value),
    );
    if let Some(model) = response_meta::reported_model_from_event(value) {
        metadata.response_metadata.effective_model = Some(model.to_owned());
    }
    let turn_state_update = websocket_metadata_turn_state(value).and_then(|turn_state| {
        if metadata.turn_state.is_some() {
            return None;
        }
        metadata.turn_state = Some(turn_state.clone());
        Some(turn_state)
    });

    let event = websocket_event_type(value);
    if let Some(response_id) = websocket_response_completed_id(value) {
        continuation.record_completed(response_id);
    }

    let terminal = match event {
        Some("response.completed") => Some(WebSocketTerminalKind::Completed),
        Some("response.incomplete") if websocket_response_is_interrupted(value) => {
            Some(WebSocketTerminalKind::Interrupted)
        }
        Some("response.incomplete") => Some(WebSocketTerminalKind::Incomplete),
        Some("response.failed" | "error") => Some(WebSocketTerminalKind::Failed),
        _ => None,
    };
    let action = match websocket_event_frame(value, raw) {
        Some(frame) => ExchangeAction::Forward { frame, terminal },
        None => ExchangeAction::Ignore,
    };
    Ok(ReducedWebSocketEvent {
        created_response_id: (event == Some("response.created"))
            .then(|| value.pointer("/response/id").and_then(Value::as_str))
            .flatten()
            .map(ToOwned::to_owned),
        action,
        diagnostic_event_type,
        turn_state_update,
        error_rate_limits: events::parse_error_rate_limits(value, None),
    })
}

fn diagnostic_event_type(event_type: Option<&str>) -> Option<String> {
    const MAX_EVENT_TYPE_BYTES: usize = 128;

    let event_type = event_type?;
    (!event_type.is_empty()
        && event_type.len() <= MAX_EVENT_TYPE_BYTES
        && event_type.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        }))
    .then(|| event_type.to_owned())
}
