//! HTTP delivery shares one execution lifecycle across Responses and Chat.

use bytes::Bytes;
use gateway_core::{
    error::{GatewayError, GatewayErrorKind},
    event::ProviderEvent,
};
use gateway_protocol::openai::{
    output_recovery::recover_response_output, sse::response_failed_sse_event_with_id_and_param,
};
use serde_json::{Value, json};

use super::{
    chat::delivery::{ChatEncoder, validate_response},
    error::{client_error_code, gateway_error_contract},
    responses::{OpenAiResponsesEncoder, ProtocolError, ProtocolErrorBody, ResponseEncodeError},
};

// Official chat_completions/delivery.rs distinguishes projection from framing failures.
const CHAT_DELIVERY_ERROR: &str = "The gateway could not deliver the Chat response.";

#[derive(Clone, Default)]
pub(in crate::openai) enum HttpResponseFormat {
    #[default]
    Responses,
    Messages,
    MessageTokenCount,
    GenAi,
    GenAiTokenCount,
    Chat,
}

impl HttpResponseFormat {
    pub(in crate::openai) fn is_messages(&self) -> bool {
        matches!(self, Self::Messages | Self::MessageTokenCount)
    }
}

pub(in crate::openai) struct HttpResponseEncoder {
    responses: OpenAiResponsesEncoder,
    chat: Option<ChatEncoder>,
    messages: Option<super::messages_response::MessagesEncoder>,
    token_count: bool,
    genai: Option<super::genai_response::GenAiEncoder>,
}

impl HttpResponseEncoder {
    pub(in crate::openai) fn new(format: &HttpResponseFormat) -> Self {
        Self {
            responses: OpenAiResponsesEncoder::new(),
            chat: matches!(format, HttpResponseFormat::Chat).then(ChatEncoder::default),
            messages: format
                .is_messages()
                .then(super::messages_response::MessagesEncoder::default),
            token_count: matches!(
                format,
                HttpResponseFormat::MessageTokenCount | HttpResponseFormat::GenAiTokenCount
            ),
            genai: matches!(
                format,
                HttpResponseFormat::GenAi | HttpResponseFormat::GenAiTokenCount
            )
            .then(super::genai_response::GenAiEncoder::default),
        }
    }

    pub(in crate::openai) fn is_chat(&self) -> bool {
        self.chat.is_some()
    }

    pub(in crate::openai) fn requires_terminal_confirmation(&self) -> bool {
        self.is_chat() || self.messages.is_some() || self.genai.is_some()
    }
    pub(in crate::openai) fn terminal_bytes(&self) -> Bytes {
        if self.messages.is_some() || self.genai.is_some() {
            Bytes::new()
        } else {
            Bytes::from_static(gateway_protocol::openai::sse::DONE_SSE_FRAME.as_bytes())
        }
    }
    pub(in crate::openai) fn missing_terminal_error(&self) -> GatewayError {
        if self.is_chat() {
            HttpEncodeError::ChatDelivery.gateway_error()
        } else {
            GatewayError::new(
                GatewayErrorKind::UpstreamUnavailable,
                "The upstream message ended without a terminal event.",
            )
            .with_client_code("invalid_upstream_response")
        }
    }

    pub(in crate::openai) fn push_sse(
        &mut self,
        event: &ProviderEvent,
    ) -> Result<Vec<Bytes>, HttpEncodeError> {
        if let Some(genai) = self.genai.as_mut() {
            return genai.push_sse(event).map_err(HttpEncodeError::Responses);
        }
        if let Some(messages) = self.messages.as_mut() {
            return Ok(messages.push_sse(event));
        }
        Ok(match self.chat.as_mut() {
            Some(chat) => chat.push(event),
            None => self.responses.push_sse(event),
        })
    }

    pub(in crate::openai) fn is_completed(&self) -> bool {
        if let Some(genai) = &self.genai {
            return genai.is_completed();
        }
        if let Some(messages) = &self.messages {
            return messages.is_completed();
        }
        self.chat
            .as_ref()
            .map_or_else(|| self.responses.is_completed(), |chat| chat.completed)
    }

    pub(in crate::openai) fn has_wire_failure(&self) -> bool {
        if let Some(genai) = &self.genai {
            return genai.has_wire_failure();
        }
        if let Some(messages) = &self.messages {
            return messages.has_wire_failure();
        }
        self.chat
            .as_ref()
            .map_or_else(|| self.responses.has_wire_failure(), |chat| chat.failed)
    }

    pub(in crate::openai) fn finish_sse(&mut self) -> Result<(), HttpEncodeError> {
        if let Some(genai) = self.genai.as_mut() {
            return genai.finish().map_err(HttpEncodeError::Responses);
        }
        Ok(())
    }

    pub(in crate::openai) fn finish_collected(
        mut self,
        events: &[ProviderEvent],
    ) -> Result<Value, HttpEncodeError> {
        if self.genai.is_some() {
            return super::genai_response::collect(events, self.token_count)
                .map_err(HttpEncodeError::Responses);
        }
        if self.messages.is_some() {
            return super::messages_response::collect(events, self.token_count)
                .map_err(HttpEncodeError::Responses);
        }
        if self.chat.is_some() {
            let mut accumulator = gateway_protocol::chat_completions::ChatAccumulator::default();
            let mut raw = None;
            let mut chunks = false;
            for wire in events
                .iter()
                .filter_map(ProviderEvent::wire_event)
                .filter(|wire| wire.protocol() == gateway_protocol::chat_completions::CHAT_PROTOCOL)
            {
                if let Some(body) = wire.raw_json_body() {
                    if chunks || raw.replace(body).is_some() {
                        return Err(HttpEncodeError::ChatDelivery);
                    }
                } else {
                    if raw.is_some() {
                        return Err(HttpEncodeError::ChatDelivery);
                    }
                    chunks = true;
                    accumulator
                        .push(wire.data())
                        .map_err(|_| HttpEncodeError::ChatDelivery)?;
                }
            }
            if let Some(raw) = raw {
                validate_response(raw).map_err(|_| HttpEncodeError::ChatDelivery)?;
                return serde_json::from_slice(raw).map_err(|_| HttpEncodeError::ChatDelivery);
            }
            return accumulator
                .finish()
                .map_err(|_| HttpEncodeError::ChatDelivery);
        }
        for event in events {
            self.responses.observe_event(event);
        }
        let mut response = self
            .responses
            .finish()
            .map_err(HttpEncodeError::Responses)?;
        recover_response_output(
            &mut response,
            events.iter().filter_map(|event| {
                let wire = event
                    .wire_event()
                    .filter(|wire| wire.protocol() == "openai" && wire.has_json_data())?;
                let kind = wire
                    .event_type()
                    .or_else(|| wire.data()["type"].as_str())
                    .unwrap_or_default();
                Some((kind, wire.data()))
            }),
        );
        Ok(response)
    }

    pub(in crate::openai) fn error_frame(&self, error: &GatewayError) -> Bytes {
        let (status, default_type, default_code) = gateway_error_contract(error.kind());
        if self.genai.is_some() {
            return super::genai::error_frame(status, error.client_message());
        }
        if self.messages.is_some() {
            return super::messages::stream_error(&super::messages::error_payload(
                &json!({"error":{"message":error.client_message()}}),
                status,
            ));
        }
        let error_type = error.client_error_type().unwrap_or(default_type);
        let code = client_error_code(error.client_error_code().unwrap_or(default_code));
        if self.is_chat() {
            let mut body = json!({
                "error": {
                    "type": error_type,
                    "code": code,
                    "message": error.client_message(),
                }
            });
            if let Some(param) = error.client_error_param() {
                body["error"]["param"] = param.into();
            }
            chat_sse_frame(body)
        } else {
            Bytes::from(response_failed_sse_event_with_id_and_param(
                self.responses.response_id(),
                error_type,
                code,
                error.client_message(),
                error.client_error_param(),
            ))
        }
    }
}

fn chat_sse_frame(value: Value) -> Bytes {
    Bytes::from(format!("data: {value}\n\n"))
}

#[derive(Debug)]
pub(in crate::openai) enum HttpEncodeError {
    Responses(ResponseEncodeError),
    ChatDelivery,
}

impl HttpEncodeError {
    pub(in crate::openai) fn protocol_body(&self) -> ProtocolErrorBody {
        match self {
            Self::Responses(error) => error.protocol_body(),
            Self::ChatDelivery => ProtocolErrorBody {
                error: ProtocolError {
                    kind: "server_error",
                    code: "invalid_upstream_response",
                    message: CHAT_DELIVERY_ERROR.to_owned(),
                    param: None,
                },
            },
        }
    }

    pub(in crate::openai) fn gateway_error(&self) -> GatewayError {
        let (message, code) = match self {
            Self::ChatDelivery => (CHAT_DELIVERY_ERROR, "invalid_upstream_response"),
            Self::Responses(_) => (
                "upstream response could not be converted to the requested format",
                "incompatible_upstream_response",
            ),
        };
        GatewayError::new(GatewayErrorKind::UpstreamUnavailable, message).with_client_code(code)
    }
}
