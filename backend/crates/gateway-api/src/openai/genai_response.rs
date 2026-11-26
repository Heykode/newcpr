//! GenAI 原生帧与完整响应校验；共享 Core 终态和结算生命周期。
use super::responses::ResponseEncodeError;
use bytes::Bytes;
use gateway_core::event::ProviderEvent;
use gateway_protocol::genai::{GENAI_PROTOCOL, GenerateContentObservation};
use serde_json::Value;

#[derive(Default)]
pub(super) struct GenAiEncoder {
    observation: GenerateContentObservation,
    failed: bool,
    completed: bool,
}
impl GenAiEncoder {
    pub(super) fn push_sse(
        &mut self,
        event: &ProviderEvent,
    ) -> Result<Vec<Bytes>, ResponseEncodeError> {
        let Some(wire) = event
            .wire_event()
            .filter(|wire| wire.protocol() == GENAI_PROTOCOL)
        else {
            return Ok(Vec::new());
        };
        let frames = if let Some(raw) = wire.raw_sse_frame() {
            vec![raw.clone()]
        } else if wire.has_json_data() {
            vec![Bytes::from(
                gateway_protocol::openai::sse::encode_sse_event_with_metadata(
                    wire.event_type().unwrap_or_default(),
                    &wire.data().to_string(),
                    wire.sse_id(),
                    wire.sse_retry(),
                ),
            )]
        } else {
            Vec::new()
        };
        // Validate the actual bytes delivered, including raw passthrough frames.
        for frame in &frames {
            let text =
                std::str::from_utf8(frame).map_err(|_| ResponseEncodeError::Serialization)?;
            for event in gateway_protocol::openai::sse::parse_sse_events(text)
                .map_err(|_| ResponseEncodeError::Serialization)?
            {
                let value: Value = serde_json::from_str(&event.data)
                    .map_err(|_| ResponseEncodeError::Serialization)?;
                if !value.is_object() {
                    return Err(ResponseEncodeError::InvalidOutputItems);
                }
                self.failed |= value.get("error").is_some_and(Value::is_object);
                self.observation.observe(&value);
            }
        }
        Ok(frames)
    }
    // GenAI finishReason is per candidate. Completion is only known at normal EOF.
    pub(super) fn is_completed(&self) -> bool {
        self.completed
    }
    pub(super) fn has_wire_failure(&self) -> bool {
        self.failed
    }
    pub(super) fn finish(&mut self) -> Result<(), ResponseEncodeError> {
        if !self.failed && self.observation.finish().is_none() {
            return Err(ResponseEncodeError::MissingWireTerminal);
        }
        self.completed = true;
        Ok(())
    }
}

pub(super) fn collect(events: &[ProviderEvent], count: bool) -> Result<Value, ResponseEncodeError> {
    let mut wires = events
        .iter()
        .filter_map(ProviderEvent::wire_event)
        .filter(|wire| wire.protocol() == GENAI_PROTOCOL);
    let wire = wires
        .next()
        .ok_or(ResponseEncodeError::MissingWireTerminal)?;
    if wires.next().is_some() {
        return Err(ResponseEncodeError::InvalidOutputItems);
    }
    let value: Value = match wire.raw_json_body() {
        Some(raw) => serde_json::from_slice(raw).map_err(|_| ResponseEncodeError::Serialization)?,
        None => wire.data().clone(),
    };
    if count && value.get("totalTokens").and_then(Value::as_u64).is_none() {
        return Err(ResponseEncodeError::InvalidOutputItems);
    }
    Ok(value)
}
