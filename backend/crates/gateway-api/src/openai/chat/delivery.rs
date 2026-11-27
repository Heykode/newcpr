//! Chat 出口只编码已转换的错误，不另建正文或提交状态机

use bytes::Bytes;
use gateway_core::event::{GatewayEvent, ProviderEvent};
use serde_json::Value;

pub(in crate::openai) fn chat_error_frame(data: &Value) -> Bytes {
    let error = data
        .pointer("/response/error")
        .or_else(|| data.get("error"))
        .unwrap_or(data);
    Bytes::from(format!("data: {}\n\n", serde_json::json!({"error": error})))
}

pub(in crate::openai) fn validate_response(bytes: &[u8]) -> Result<(), ()> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    if value.get("object").and_then(Value::as_str) != Some("chat.completion")
        || value.get("id").and_then(Value::as_str).is_none()
        || !value
            .get("choices")
            .and_then(Value::as_array)
            .is_some_and(|choices| {
                choices.len() == 1
                    && choices[0].get("message").is_some_and(Value::is_object)
                    && choices[0]
                        .get("finish_reason")
                        .is_some_and(Value::is_string)
            })
    {
        return Err(());
    }
    Ok(())
}

/// finish 与 usage 必须随原生 Completed 交付，避免终结失败后已向客户端报成功
#[derive(Default)]
pub(in crate::openai) struct ChatEncoder {
    terminal: Vec<Bytes>,
    pub(in crate::openai) completed: bool,
    pub(in crate::openai) failed: bool,
}

impl ChatEncoder {
    pub(in crate::openai) fn push(&mut self, event: &ProviderEvent) -> Vec<Bytes> {
        let mut output = Vec::new();
        if let Some(wire) = event
            .wire_event()
            .filter(|wire| wire.protocol() == gateway_protocol::chat_completions::CHAT_PROTOCOL)
        {
            self.failed |= wire.data().get("error").is_some_and(Value::is_object);
            let bytes = wire.raw_sse_frame().cloned().or_else(|| {
                wire.has_json_data()
                    .then(|| Bytes::from(format!("data: {}\n\n", wire.data())))
            });
            if let Some(bytes) = bytes {
                if wire
                    .data()
                    .pointer("/choices/0/finish_reason")
                    .is_some_and(Value::is_string)
                    || wire
                        .data()
                        .get("choices")
                        .and_then(Value::as_array)
                        .is_some_and(Vec::is_empty)
                {
                    self.terminal.push(bytes);
                } else {
                    output.push(bytes);
                }
            }
        }
        if event
            .canonical_facts()
            .iter()
            .any(|fact| matches!(fact, GatewayEvent::Completed(_)))
        {
            self.completed = true;
            output.append(&mut self.terminal);
        }
        output
    }
}
