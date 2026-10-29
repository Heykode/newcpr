//! HTTP delivery shares one execution lifecycle across Responses and Chat.

use bytes::Bytes;
use gateway_core::{
    error::{ClientVisibleUpstreamError, GatewayError, GatewayErrorKind},
    event::ProviderEvent,
};
use gateway_protocol::openai::{
    output_recovery::recover_response_output,
    sse::{SseEventDecoder, response_failed_sse_event_with_id_and_param},
};
use serde_json::{Value, json};

use super::{
    chat::response::{ChatIdentity, ChatStreamEncoder, project_response},
    error::{client_error_code, gateway_error_contract},
    responses::{OpenAiResponsesEncoder, ProtocolError, ProtocolErrorBody, ResponseEncodeError},
};

#[derive(Clone, Default)]
pub(in crate::openai) enum HttpResponseFormat {
    #[default]
    Responses,
    Chat {
        include_usage: bool,
        identity: ChatIdentity,
    },
}

pub(in crate::openai) struct HttpResponseEncoder {
    responses: OpenAiResponsesEncoder,
    chat: Option<ChatStreamEncoder>,
    chat_identity: Option<ChatIdentity>,
    decoder: SseEventDecoder,
}

impl HttpResponseEncoder {
    pub(in crate::openai) fn new(format: &HttpResponseFormat) -> Self {
        Self {
            responses: OpenAiResponsesEncoder::new(),
            chat: match format {
                HttpResponseFormat::Responses => None,
                HttpResponseFormat::Chat {
                    include_usage,
                    identity,
                } => Some(ChatStreamEncoder::new(identity.clone(), *include_usage)),
            },
            chat_identity: match format {
                HttpResponseFormat::Chat { identity, .. } => Some(identity.clone()),
                HttpResponseFormat::Responses => None,
            },
            decoder: SseEventDecoder::default(),
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
        let mut output = Vec::new();
        for bytes in self.responses.push_sse(event) {
            for event in self
                .decoder
                .push(&bytes)
                .map_err(|_| invalid_chat_response())?
            {
                let data =
                    serde_json::from_str(&event.data).map_err(|_| invalid_chat_response())?;
                output.extend(
                    chat.push_event(event.event.as_deref(), &data)
                        .map_err(HttpEncodeError::Chat)?,
                );
            }
        }
        Ok(output)
    }

    pub(in crate::openai) fn is_completed(&self) -> bool {
        self.chat.as_ref().map_or_else(
            || self.responses.is_completed(),
            ChatStreamEncoder::is_completed,
        )
    }

    pub(in crate::openai) fn has_wire_failure(&self) -> bool {
        self.chat.as_ref().map_or_else(
            || self.responses.has_wire_failure(),
            ChatStreamEncoder::has_wire_failure,
        )
    }

    pub(in crate::openai) fn finish_sse(&mut self) -> Result<(), HttpEncodeError> {
        if let Some(chat) = self.chat.as_mut() {
            for event in self.decoder.finish().map_err(|_| invalid_chat_response())? {
                let data =
                    serde_json::from_str(&event.data).map_err(|_| invalid_chat_response())?;
                if !chat
                    .push_event(event.event.as_deref(), &data)
                    .map_err(HttpEncodeError::Chat)?
                    .is_empty()
                {
                    return Err(invalid_chat_response());
                }
            }
        }
        Ok(())
    }

    pub(in crate::openai) fn finish_collected(
        mut self,
        events: &[ProviderEvent],
    ) -> Result<Value, HttpEncodeError> {
        if let Some(identity) = &self.chat_identity {
            if let Some(error) = events.iter().find_map(chat_capacity_error) {
                return Err(error);
            }
            let response =
                super::responses::collect_response(events).map_err(HttpEncodeError::Responses)?;
            return project_response(&response, identity).map_err(HttpEncodeError::Chat);
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

fn invalid_chat_response() -> HttpEncodeError {
    HttpEncodeError::Chat(ProtocolErrorBody {
        error: ProtocolError {
            kind: "server_error",
            code: "invalid_upstream_response",
            message: "The gateway could not deliver the Chat response.".to_owned(),
            param: None,
        },
    })
}

fn chat_capacity_error(event: &ProviderEvent) -> Option<HttpEncodeError> {
    let wire = event
        .wire_event()
        .filter(|wire| wire.protocol() == "openai")?;
    if ![
        wire.event_type(),
        wire.data().get("type").and_then(Value::as_str),
    ]
    .into_iter()
    .any(|kind| matches!(kind, Some("error" | "response.failed")))
    {
        return None;
    }
    let error = ["/response/error", "/error"]
        .into_iter()
        .filter_map(|path| wire.data().pointer(path))
        .find(|error| {
            error
                .get("code")
                .and_then(Value::as_str)
                .is_some_and(|code| client_error_code(code) != code)
        })?;
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("upstream capacity is temporarily unavailable");
    Some(HttpEncodeError::Capacity(ClientVisibleUpstreamError::new(
        message,
        error.get("code").and_then(Value::as_str).map(str::to_owned),
        error.get("type").and_then(Value::as_str).map(str::to_owned),
    )))
}

#[derive(Debug)]
pub(in crate::openai) enum HttpEncodeError {
    Responses(ResponseEncodeError),
    Chat(ProtocolErrorBody),
    Capacity(ClientVisibleUpstreamError),
}

impl HttpEncodeError {
    pub(in crate::openai) fn protocol_body(&self) -> ProtocolErrorBody {
        match self {
            Self::Capacity(error) => ProtocolErrorBody {
                error: ProtocolError {
                    kind: "server_error",
                    code: "server_error",
                    message: error.message().to_owned(),
                    param: None,
                },
            },
            Self::Responses(error) => error.protocol_body(),
            Self::Chat(error) => error.clone(),
        }
    }

    pub(in crate::openai) fn gateway_error(&self) -> GatewayError {
        if let Self::Capacity(error) = self {
            return GatewayError::new(
                GatewayErrorKind::UpstreamUnavailable,
                "upstream capacity is temporarily unavailable",
            )
            .with_client_visible_upstream_error(error.clone());
        }
        GatewayError::new(
            GatewayErrorKind::UpstreamUnavailable,
            "upstream response could not be converted to the requested format",
        )
        .with_client_code(match self {
            Self::Chat(error) => error.error.code,
            _ => "incompatible_upstream_response",
        })
    }
}
