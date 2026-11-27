//! Chat 在选择 OAuth 账号后直接转成 Responses，保留原始模型名交付客户端。
use super::*;
use gateway_core::engine::provider::NativeResponseTranslator;
use gateway_protocol::chat_completions::{CHAT_PROTOCOL, ChatIdentity, responses};

pub(super) fn request_error() -> ProviderError {
    provider_error(
        ProviderErrorKind::InvalidRequest,
        UpstreamSendState::NotSent,
    )
}
fn response_error() -> ProviderError {
    provider_error(ProviderErrorKind::Protocol, UpstreamSendState::Sent)
}
pub(super) fn prepare(
    generate: &GenerateRequest,
    model: &UpstreamModelId,
    context: &AttemptContext,
) -> Result<(Operation, Translator), ProviderError> {
    let original = generate.protocol_payload().body();
    let mut body = responses::convert_request(original).map_err(|error| {
        ProviderError::invalid_request(error.safe_message())
            .with_request_details(error.code(), error.parameter())
            .with_source(error)
    })?;
    if matches!(
        body.get("service_tier").and_then(Value::as_str),
        Some("auto" | "default")
    ) {
        body.shift_remove("service_tier");
    }
    let operation = Operation::Generate(generate.clone())
        .translate_protocol_wire(
            OPENAI_PROTOCOL,
            serde_json::to_vec(&body)
                .map_err(|_| request_error())?
                .into(),
        )
        .map_err(|_| request_error())?;
    let identity = ChatIdentity {
        id: format!("chatcmpl-{}", context.request_id().as_str()),
        created: SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        model: original
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(model.as_str())
            .to_owned(),
    };
    let streaming = original.get("stream").and_then(Value::as_bool) == Some(true);
    let include_usage = !streaming
        || original
            .get("stream_options")
            .and_then(|v| v.get("include_usage"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
    Ok((
        operation,
        Translator {
            encoder: responses::ChatStreamEncoder::new(identity.clone(), include_usage),
            identity,
            streaming,
            include_usage,
            complete: false,
        },
    ))
}
pub(super) struct Translator {
    encoder: responses::ChatStreamEncoder,
    identity: ChatIdentity,
    streaming: bool,
    include_usage: bool,
    complete: bool,
}
fn chunks(values: Vec<Value>) -> Result<Vec<ProtocolWireEvent>, ProviderError> {
    values
        .into_iter()
        .map(|value| {
            ProtocolWireEvent::json(CHAT_PROTOCOL, None, value).map_err(|_| response_error())
        })
        .collect()
}
impl NativeResponseTranslator for Translator {
    fn source_protocol(&self) -> &str {
        OPENAI_PROTOCOL
    }
    fn target_protocol(&self) -> &str {
        CHAT_PROTOCOL
    }
    fn translate(
        &mut self,
        event: &ProtocolWireEvent,
    ) -> Result<Vec<ProtocolWireEvent>, ProviderError> {
        if self.complete {
            return Err(response_error());
        }
        if let Some(raw) = event.raw_json_body() {
            let body = serde_json::from_slice(raw).map_err(|_| response_error())?;
            let response = responses::project_response(&body, &self.identity)
                .map_err(|e| response_error().with_source(e))?;
            self.complete = true;
            if self.streaming {
                return chunks(
                    gateway_protocol::chat_completions::completion_chunks(
                        &response,
                        self.include_usage,
                    )
                    .map_err(|e| response_error().with_source(e))?,
                );
            }
            return ProtocolWireEvent::raw_json(
                CHAT_PROTOCOL,
                serde_json::to_vec(&response)
                    .map_err(|_| response_error())?
                    .into(),
            )
            .map(|wire| vec![wire])
            .map_err(|_| response_error());
        }
        if !event.has_json_data() {
            return Ok(vec![]);
        }
        chunks(
            self.encoder
                .push_event(event.event_type(), event.data())
                .map_err(|e| response_error().with_source(e))?,
        )
    }
    fn finish(&mut self) -> Result<Vec<ProtocolWireEvent>, ProviderError> {
        if self.complete {
            return Ok(vec![]);
        }
        chunks(
            self.encoder
                .finish()
                .map_err(|e| response_error().with_source(e))?,
        )
    }
}
