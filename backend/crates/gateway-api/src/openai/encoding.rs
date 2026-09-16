//! HTTP delivery shares one execution lifecycle across Responses and Chat.

use bytes::Bytes;
use gateway_core::{
    error::{GatewayError, GatewayErrorKind},
    event::ProviderEvent,
};
use gateway_protocol::openai::{
    chat::{ChatConversionError, ChatStreamEncoder, chat_response_from_events},
    output_recovery::recover_response_output,
    sse::response_failed_sse_event_with_id,
};
use serde_json::{Value, json};

use super::{
    error::gateway_error_contract,
    responses::{OpenAiResponsesEncoder, ProtocolError, ProtocolErrorBody, ResponseEncodeError},
};

#[derive(Clone, Copy, Default)]
pub(in crate::openai) enum HttpResponseFormat {
    #[default]
    Responses,
    Chat {
        include_usage: bool,
    },
}

pub(in crate::openai) struct HttpResponseEncoder {
    responses: OpenAiResponsesEncoder,
    chat: Option<ChatStreamEncoder>,
}

impl HttpResponseEncoder {
    pub(in crate::openai) fn new(format: HttpResponseFormat) -> Self {
        Self {
            responses: OpenAiResponsesEncoder::new(),
            chat: match format {
                HttpResponseFormat::Responses => None,
                HttpResponseFormat::Chat { include_usage } => {
                    Some(ChatStreamEncoder::new(include_usage))
                }
            },
        }
    }

    pub(in crate::openai) fn is_chat(&self) -> bool {
        self.chat.is_some()
    }

    pub(in crate::openai) fn push_sse(
        &mut self,
        event: &ProviderEvent,
    ) -> Result<Vec<Bytes>, HttpEncodeError> {
        let Some(chat) = self.chat.as_mut() else {
            return Ok(self.responses.push_sse(event));
        };
        let Some(wire) = event
            .wire_event()
            .filter(|wire| wire.protocol() == "openai" && wire.has_json_data())
        else {
            return Ok(Vec::new());
        };
        chat.push(wire.event_type(), wire.data())
            .map(|chunks| chunks.into_iter().map(chat_sse_frame).collect())
            .map_err(HttpEncodeError::Chat)
    }

    pub(in crate::openai) fn is_completed(&self) -> bool {
        self.chat.as_ref().map_or_else(
            || self.responses.is_completed(),
            ChatStreamEncoder::is_completed,
        )
    }

    pub(in crate::openai) fn has_wire_failure(&self) -> bool {
        // Chat conversion failures become explicit error frames at the caller.
        self.chat.is_none() && self.responses.has_wire_failure()
    }

    pub(in crate::openai) fn finish_collected(
        mut self,
        events: &[ProviderEvent],
    ) -> Result<Value, HttpEncodeError> {
        if self.chat.is_some() {
            return chat_response_from_events(events.iter().filter_map(|event| {
                let wire = event
                    .wire_event()
                    .filter(|wire| wire.protocol() == "openai" && wire.has_json_data())?;
                Some((wire.event_type(), wire.data()))
            }))
            .map_err(HttpEncodeError::Chat);
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
        let (_, default_type, default_code) = gateway_error_contract(error.kind());
        let error_type = error.client_error_type().unwrap_or(default_type);
        let code = error.client_error_code().unwrap_or(default_code);
        if self.is_chat() {
            chat_sse_frame(json!({
                "error": {
                    "type": error_type,
                    "code": code,
                    "message": error.client_message(),
                }
            }))
        } else {
            Bytes::from(response_failed_sse_event_with_id(
                self.responses.response_id(),
                error_type,
                code,
                error.client_message(),
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
    Chat(ChatConversionError),
}

impl HttpEncodeError {
    pub(in crate::openai) fn protocol_body(&self) -> ProtocolErrorBody {
        match self {
            Self::Responses(error) => error.protocol_body(),
            Self::Chat(error) => ProtocolErrorBody {
                error: ProtocolError {
                    kind: "server_error",
                    code: "incompatible_upstream_response",
                    message: error.to_string(),
                    param: error.param().map(str::to_owned),
                },
            },
        }
    }

    pub(in crate::openai) fn gateway_error(&self) -> GatewayError {
        GatewayError::new(
            GatewayErrorKind::UpstreamUnavailable,
            "upstream response could not be converted to the requested format",
        )
        .with_client_code("incompatible_upstream_response")
    }
}
