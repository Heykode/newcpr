//! Official OAuth Messages input-token estimation, bounded by CPU capacity and request lifetime.
use super::*;
use gateway_core::operation::TokenCountRequest;
use gateway_protocol::anthropic::ANTHROPIC_PROTOCOL;
use std::sync::OnceLock;

impl CodexProvider {
    pub(super) async fn execute_count_tokens(
        &self,
        request: &TokenCountRequest,
        candidate: &ProviderCandidate,
        context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        if request.payload().protocol() != ANTHROPIC_PROTOCOL {
            return Err(invalid_request());
        }
        let model = candidate
            .upstream_model()
            .cloned()
            .ok_or_else(invalid_request)?;
        let started = Instant::now();
        let lease = self
            .selector
            .select(&SelectCodexCredential {
                upstream_model: model.as_str(),
                request_url: &self.responses_url,
                attempt: &context,
                session_affinity_key: None,
                guardian: false,
            })
            .await
            .map_err(map_selection_error)?;
        let account_selection_wait_ms =
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut body: Map<String, Value> =
            serde_json::from_slice(request.payload().body()).map_err(|_| invalid_request())?;
        body.insert("model".into(), Value::String(model.as_str().into()));
        gateway_protocol::anthropic::validate_count_request(&body)
            .map_err(|_| invalid_request())?;
        self.local_count_stream(
            body,
            model,
            context,
            lease,
            account_selection_wait_ms,
            ANTHROPIC_PROTOCOL,
        )
    }
    fn local_count_stream(
        &self,
        body: Map<String, Value>,
        model: UpstreamModelId,
        context: AttemptContext,
        lease: CodexCredentialLease,
        account_selection_wait_ms: u64,
        response_protocol: &'static str,
    ) -> Result<ProviderStream, ProviderError> {
        let metadata = ProviderCallMetadata::new(
            ProviderKind::new(PROVIDER_NAME).map_err(|_| invalid_request())?,
            model.clone(),
            lease.account_id().clone(),
            UpstreamTransport::new("local_tokenizer").map_err(|_| invalid_request())?,
        )
        .with_credential_revision(lease.account().revision())
        .with_selection_observation(ProviderSelectionObservation::new(
            account_selection_wait_ms,
            lease.capacity_snapshot(),
        ));
        let tokenizer_model = model.as_str().to_ascii_lowercase();
        let events: EventStream = Box::pin(async_stream::try_stream! {
            // BPE 是 CPU 密集任务，按可用核数限制进程内并发；断开后由工作线程持有名额直到退出
            static WORKERS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
            let workers = WORKERS.get_or_init(|| tokio::sync::Semaphore::new(
                std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
            ));
            let permit = tokio::select! {
                biased;
                _ = context.cancellation().cancelled() => Err(provider_error(ProviderErrorKind::Cancelled, UpstreamSendState::NotSent)),
                _ = context.deadline().wait() => Err(provider_error(ProviderErrorKind::Timeout, UpstreamSendState::NotSent)),
                result = workers.acquire() => result.map_err(|_| provider_error(ProviderErrorKind::Protocol, UpstreamSendState::NotSent)),
            }?;
            let cancellation = context.cancellation().clone();
            let deadline = context.deadline();
            let count = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let check = || {
                    if cancellation.is_cancelled() { Err(provider_error(ProviderErrorKind::Cancelled, UpstreamSendState::NotSent)) }
                    else if deadline.is_elapsed() { Err(provider_error(ProviderErrorKind::Timeout, UpstreamSendState::NotSent)) }
                    else { Ok(()) }
                };
                check()?;
                let translated = gateway_protocol::anthropic::messages_to_responses_count_request(&body).map_err(|_| invalid_request())?;
                let segments = responses_segments(&translated);
                static O200K: OnceLock<Result<tiktoken_rs::CoreBPE, ()>> = OnceLock::new();
                static CL100K: OnceLock<Result<tiktoken_rs::CoreBPE, ()>> = OnceLock::new();
                // 与 CPA 的 Codex 映射一致，未知 Responses 模型使用 cl100k
                let modern = ["gpt-5", "gpt-4.1", "gpt-4o"].iter().any(|prefix| tokenizer_model.starts_with(prefix));
                let encoder = if modern {
                    O200K.get_or_init(|| tiktoken_rs::o200k_base().map_err(|_| ()))
                } else {
                    CL100K.get_or_init(|| tiktoken_rs::cl100k_base().map_err(|_| ()))
                }.as_ref().map_err(|_| {
                    provider_error(ProviderErrorKind::Protocol, UpstreamSendState::NotSent)
                })?;
                check()?;
                Ok::<u64, ProviderError>(u64::try_from(encoder.encode_ordinary(&segments.join("\n")).len()).unwrap_or(u64::MAX))
            });
            let count = tokio::select! {
                biased;
                _ = context.cancellation().cancelled() => Err(provider_error(ProviderErrorKind::Cancelled, UpstreamSendState::NotSent)),
                _ = context.deadline().wait() => Err(provider_error(ProviderErrorKind::Timeout, UpstreamSendState::NotSent)),
                result = count => result.map_err(|_| provider_error(ProviderErrorKind::Protocol, UpstreamSendState::NotSent)).and_then(std::convert::identity),
            }?;
            let meta = ResponseMeta::new(context.request_id().as_str(), model.as_str());
            let wire = ProtocolWireEvent::raw_json(response_protocol, serde_json::to_vec(&serde_json::json!({"input_tokens":count})).map_err(|_| provider_error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?.into()).map_err(|_| provider_error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
            yield ProviderEvent::canonical_with_wire(vec![GatewayEvent::Started(meta.clone()), GatewayEvent::Completed(meta.with_finish_reason(FinishReason::Stop))], wire);
        });
        // 估算没有发送上游请求，不能计入账号响应成功率或上游用量
        Ok(ProviderStream::new(metadata, events, lease))
    }
}

fn push(segments: &mut Vec<String>, value: &Value) {
    if value.is_null() {
        return;
    }
    let text = value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string());
    let text = text.trim();
    if !text.is_empty() {
        segments.push(text.to_owned());
    }
}

fn fields(segments: &mut Vec<String>, value: &Value, names: &[&str]) {
    for name in names {
        push(segments, &value[name]);
    }
}

fn responses_segments(body: &Map<String, Value>) -> Vec<String> {
    let mut segments = Vec::new();
    if let Some(instructions) = body.get("instructions") {
        push(&mut segments, instructions);
    }
    for item in body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("message") | None => {
                if item["content"].is_string() {
                    push(&mut segments, &item["content"]);
                }
                for part in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    push(&mut segments, &part["text"]);
                }
            }
            Some("function_call") => fields(&mut segments, item, &["name", "arguments"]),
            Some("function_call_output") => push(&mut segments, &item["output"]),
            _ => push(&mut segments, &item["text"]),
        }
    }
    for tool in body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        fields(&mut segments, tool, &["name", "description", "parameters"]);
    }
    if let Some(format) = body.get("text").and_then(|value| value.get("format")) {
        fields(&mut segments, format, &["name", "schema"]);
    }
    segments
}

fn invalid_request() -> ProviderError {
    provider_error(
        ProviderErrorKind::InvalidRequest,
        UpstreamSendState::NotSent,
    )
}
