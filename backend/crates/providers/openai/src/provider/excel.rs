use sha2::{Digest, Sha256};

use super::*;
use crate::transport::excel::{
    ExcelPreparedRequest, ExcelRequestError, StructuredOutput, prepare_request,
};

pub(super) async fn prepare_excel(
    request: &mut CodexResponsesRequest,
    lease: &CodexCredentialLease,
    context: &AttemptContext,
    replay: Arc<dyn gateway_core::provider_ports::ProviderReplayPort>,
    image_relay: &Arc<crate::transport::excel::image_relay::ImageRelay>,
) -> Result<(), ProviderError> {
    if !matches!(
        lease.authentication(),
        crate::credential::CodexRuntimeAuthentication::OAuth(_)
    ) || lease.account().upstream_account_id().is_none()
    {
        return Err(provider_error(
            ProviderErrorKind::InvalidRequest,
            UpstreamSendState::NotSent,
        ));
    }
    let mut source = request.body().clone();
    if request.previous_response_id().is_some()
        && context
            .continuation()
            .and_then(ContinuationBinding::pinned)
            .is_none()
    {
        // The Core index publishes only delivered responses. A raw ID observed
        // before cancellation must not expose a provisional replay record.
        return Err(request_error(ExcelRequestError::History));
    }
    let cache_anchor = request
        .local_conversation_id
        .as_deref()
        .or_else(|| source.get("prompt_cache_key").and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_else(|| {
            source
                .get("input")
                .map(|value| {
                    value
                        .as_array()
                        .and_then(|items| items.first())
                        .unwrap_or(value)
                        .to_string()
                })
                .unwrap_or_default()
        });
    let scope = serde_json::json!([
        "cpr-excel-v1",
        context.client_api_key_ref().as_str(),
        lease.account_id().as_str(),
        lease.account().upstream_user_id(),
        lease.account().upstream_account_id(),
        request.model()
    ]);
    let owner = hex::encode(Sha256::digest(scope.to_string().as_bytes()));
    let trusted_session = request
        .local_conversation_id
        .as_deref()
        .or_else(|| source.get("prompt_cache_key").and_then(Value::as_str))
        .or_else(|| source.get("session_id").and_then(Value::as_str));
    let restored = crate::transport::excel::replay::restore_scoped(
        replay,
        owner.clone(),
        cache_anchor,
        request.previous_response_id(),
        &source,
        trusted_session,
    )
    .await
    .map_err(request_error)?;
    source.remove("previous_response_id");
    source.insert("input".into(), Value::Array(restored.input));
    source.insert(
        "prompt_cache_key".into(),
        hex::encode(Sha256::digest(
            serde_json::json!([owner, restored.conversation])
                .to_string()
                .as_bytes(),
        ))
        .into(),
    );
    let tools = restored.tools;
    let structured = StructuredOutput::parse(&source).map_err(request_error)?;
    let mut body = prepare_request(&source, &tools, &restored.native_calls, structured.as_ref())
        .map_err(request_error)?;
    let trace = context.trace();
    if trace.is_enabled() {
        let (requested, effective) =
            crate::transport::excel::reasoning_effort(&source).map_err(request_error)?;
        trace.record(
            "excel.compatibility",
            json!({
                "requestedReasoningEffort": requested,
                "effectiveReasoningEffort": effective,
                "unavailableHostedTools": tools.unavailable(),
            }),
        );
    }
    let image_tuning = image_relay.request_tuning();
    let image_limits = image_tuning.into();
    crate::transport::excel::images::validate_with_limits(&body, false, image_limits)
        .map_err(request_error)?;
    let image_lease = image_relay
        .stage_with_tuning(&mut body, image_tuning)
        .map_err(request_error)?;
    crate::transport::excel::images::validate_with_limits(&body, true, image_limits)
        .map_err(request_error)?;
    crate::transport::request::clear_turn_state(request);
    request.force_http_sse = true;
    request.use_websocket = false;
    request.excel = Some(ExcelPreparedRequest {
        body,
        tools,
        structured,
        _image_lease: image_lease,
        image_limits,
        completed: Default::default(),
        usage: crate::transport::excel::usage::ExcelUsagePolicy::new(
            lease.account().excel_cache_creation_as_input(),
        ),
        replay: Some(restored.capture),
        endpoint: crate::transport::excel::RESPONSES_URL.into(),
    });
    Ok(())
}

pub(super) fn request_error(error: ExcelRequestError) -> ProviderError {
    if let ExcelRequestError::ImageInput(message) = error {
        return provider_error(
            ProviderErrorKind::InvalidRequest,
            UpstreamSendState::NotSent,
        )
        .with_status(400)
        .with_upstream_code(OpaqueUpstreamValue::new("excel_image_input_invalid"))
        .with_client_visible_upstream_error(gateway_core::error::ClientVisibleUpstreamError::new(
            message,
            Some("excel_image_input_invalid".into()),
            Some("invalid_request_error".into()),
        ))
        .with_diagnostic(ProviderDiagnostic::new(error.to_string()));
    }
    if error == ExcelRequestError::ImageRelay {
        return provider_error(ProviderErrorKind::Unavailable, UpstreamSendState::NotSent)
            .with_status(503)
            .with_upstream_code(OpaqueUpstreamValue::new("excel_image_relay_unavailable"));
    }
    if matches!(
        error,
        ExcelRequestError::History | ExcelRequestError::HistoryInput { .. }
    ) {
        let mut failure = continuation_replay_required_error("excel_history_unavailable");
        if matches!(error, ExcelRequestError::HistoryInput { .. }) {
            failure = failure
                .with_diagnostic(ProviderDiagnostic::new(error.to_string()))
                .with_client_visible_upstream_error(
                    gateway_core::error::ClientVisibleUpstreamError::new(
                        error.to_string(),
                        Some("previous_response_not_found".into()),
                        Some("invalid_request_error".into()),
                    ),
                );
        }
        return failure;
    }
    if matches!(error, ExcelRequestError::EncryptedContent { .. }) {
        return provider_error(
            ProviderErrorKind::InvalidRequest,
            UpstreamSendState::NotSent,
        )
        .with_status(400)
        .with_upstream_code(OpaqueUpstreamValue::new("excel_unsupported_content"))
        .with_client_visible_upstream_error(gateway_core::error::ClientVisibleUpstreamError::new(
            error.to_string(),
            Some("excel_unsupported_content".into()),
            Some("invalid_request_error".into()),
        ))
        .with_diagnostic(ProviderDiagnostic::new(error.to_string()));
    }
    provider_error(
        ProviderErrorKind::InvalidRequest,
        UpstreamSendState::NotSent,
    )
    .with_status(400)
    .with_upstream_code(OpaqueUpstreamValue::new("excel_unsupported_request"))
    .with_diagnostic(ProviderDiagnostic::new(error.to_string()))
}

pub(super) fn map_repair_failure(error: CodexClientError) -> MappedProviderFailure {
    let status = match &error {
        CodexClientError::Upstream { status, .. } => Some(status.as_u16()),
        _ => None,
    };
    let mut failure = if let Some(upstream) = error.upstream_failure() {
        map_upstream_failure(upstream, None, ReplayBoundary::AfterSemanticOutput)
    } else {
        map_client_error(error, UpstreamSendState::Sent, false)
    };
    failure.http_rejection_status = status;
    classify_failure(failure, true)
}

pub(super) fn failed_repair_metering(request: &CodexResponsesRequest) -> Vec<ProviderEvent> {
    use crate::transport::usage::{OpenAiBillingUsage, openai_billing_breakdown};
    use gateway_protocol::openai::events::{billable_usage_is_complete, extract_usage};
    let Some(response) = request
        .excel
        .as_ref()
        .and_then(|prepared| prepared.usage.take_failed_repair_usage())
    else {
        return Vec::new();
    };
    let Some(raw) = extract_usage(&response) else {
        return Vec::new();
    };
    let mut usage = gateway_core::metering::Usage::new();
    usage.input_tokens = Some(raw.input_tokens);
    usage.output_tokens = Some(raw.output_tokens);
    usage.cached_tokens = Some(raw.cached_tokens);
    usage.cache_write_tokens = Some(raw.cache_write_tokens);
    usage.reasoning_tokens = Some(raw.reasoning_tokens);
    usage.total_tokens = Some(raw.total_tokens);
    let cost = billable_usage_is_complete(&response, raw)
        .then(|| {
            openai_billing_breakdown(
                request.model(),
                OpenAiBillingUsage::from(raw),
                request.service_tier(),
            )
        })
        .flatten()
        .map(|cost| cost.calculated_cost());
    vec![ProviderEvent::metering(
        gateway_core::event::ProviderMeteringCheckpoint::new(usage, cost),
    )]
}

fn should_disable_on_403(account: &ProviderAccount, failure: &MappedProviderFailure) -> bool {
    let error = &failure.error;
    account.excel_auto_disable_on_403()
        && account.responses_upstream() == gateway_core::account::ResponsesUpstream::Excel
        && failure.http_rejection_status == Some(403)
        && !error.upstream_code().is_some_and(|code| {
            code.as_str()
                .trim()
                .eq_ignore_ascii_case("basispoints_model_access_changed")
        })
        && !error.client_visible_upstream_error().is_some_and(|detail| {
            [detail.code(), detail.error_type()]
                .into_iter()
                .flatten()
                .any(|code| {
                    code.trim()
                        .eq_ignore_ascii_case("basispoints_model_access_changed")
                })
        })
}

pub(super) async fn observe_http_rejection(
    selector: &Arc<CodexCredentialSelector>,
    account: &ProviderAccount,
    failure: &mut MappedProviderFailure,
    excel: bool,
    allows_mutation: bool,
    diagnostic: bool,
) {
    if !excel || !allows_mutation {
        return;
    }
    if let Some(revoked) = isolate_http_authentication_failure(failure) {
        selector
            .record_excel_authentication_failure(account, revoked, diagnostic)
            .await;
        return;
    }
    if !should_disable_on_403(account, failure) {
        return;
    }
    suppress_rejection_recovery(failure);
    let selector = selector.clone();
    let account = account.clone();
    // Cancellation may drop the waiter, but the already-confirmed write stays bounded.
    let _ = tokio::spawn(async move {
        match tokio::time::timeout(Duration::from_secs(2), selector.disable_excel_on_403(&account)).await {
            Ok(Ok(true)) => tracing::info!(account_id = %account.id(), "Excel disabled after upstream HTTP 403; request not replayed"),
            Ok(Ok(false)) => {},
            _ => tracing::warn!(account_id = %account.id(), "Excel HTTP 403 auto-disable did not complete"),
        }
    }).await;
}

fn isolate_http_authentication_failure(failure: &mut MappedProviderFailure) -> Option<bool> {
    if failure.http_rejection_status != Some(401) {
        return None;
    }
    let revoked = failure.account_failure == Some(CodexAccountFailure::CredentialRevoked);
    // Preserve downstream evidence; do not replay or persist arbitrary upstream text.
    suppress_recovery_with_kind(failure, ProviderErrorKind::Unauthorized);
    failure.rate_limit_headers.clear();
    failure.error_message = Some("Excel upstream authentication failed".to_owned());
    Some(revoked)
}

fn suppress_rejection_recovery(failure: &mut MappedProviderFailure) {
    suppress_recovery_with_kind(failure, ProviderErrorKind::PermissionDenied);
    failure.rate_limit_headers.clear();
}

fn suppress_recovery_with_kind(failure: &mut MappedProviderFailure, kind: ProviderErrorKind) {
    let old = &failure.error;
    let mut error = provider_error(kind, old.send_state());
    if kind == ProviderErrorKind::RateLimited
        && let Some(delay) = old.retry_after()
    {
        error = error.with_retry_after(delay);
    }
    if let Some(status) = old.upstream_status() {
        error = error.with_status(status);
    }
    if let Some(code) = old.upstream_code() {
        error = error.with_upstream_code(code.clone());
    }
    if let Some(detail) = old.client_visible_upstream_error() {
        error = error.with_client_visible_upstream_error(detail.clone());
    }
    if let Some(raw) = old.raw_upstream_error() {
        error = error.with_raw_upstream_error(raw.clone());
    }
    if let Some(response) = old.client_visible_upstream_response() {
        error = error.with_client_visible_upstream_response(
            gateway_core::error::ClientVisibleUpstreamResponse::new(
                response.status(),
                response.content_type().map(<[u8]>::to_vec),
                response.body().clone(),
            )
            .with_headers(response.headers().to_vec()),
        );
    }
    if let Some(id) = old.upstream_request_id() {
        error = error.with_upstream_request_id(id.clone());
    }
    failure.error = error;
    failure.account_failure = None;
    failure.websocket_transport_retryable = false;
    failure.cyber_policy_failure = false;
    failure.capture_response_cookies = false;
}

pub(super) fn classify_failure(
    mut failure: MappedProviderFailure,
    excel: bool,
) -> MappedProviderFailure {
    if !excel {
        return failure;
    }
    let Some(detail) = failure.error.client_visible_upstream_error() else {
        isolate_endpoint_rate_limit(&mut failure);
        return failure;
    };
    let classified = [detail.code(), detail.error_type()]
        .into_iter()
        .flatten()
        .find_map(|code| match code.trim().to_ascii_lowercase().as_str() {
            "workspace_not_allowed" => Some((
                "workspace_not_allowed",
                ProviderErrorKind::PermissionDenied,
                None,
            )),
            "basispoints_model_access_changed" => Some((
                "basispoints_model_access_changed",
                ProviderErrorKind::Unsupported,
                None,
            )),
            "token_revoked" | "token_invalidated" | "refresh_token_invalidated" => Some((
                "token_revoked",
                ProviderErrorKind::Unauthorized,
                Some(CodexAccountFailure::CredentialRevoked),
            )),
            "access_token_expired" | "token_expired" | "token_invalid" | "invalid_api_key" => {
                Some((
                    "access_token_expired",
                    ProviderErrorKind::Unauthorized,
                    Some(CodexAccountFailure::CredentialExpired),
                ))
            }
            _ => None,
        });
    let Some((code, kind, account_failure)) = classified else {
        isolate_endpoint_rate_limit(&mut failure);
        return failure;
    };
    // Preserve the upstream evidence, but not Codex's status-only retry/cooldown policy.
    let old = &failure.error;
    let mut error = provider_error(kind, old.send_state())
        .with_upstream_code(OpaqueUpstreamValue::new(code))
        .with_client_visible_upstream_error(detail.clone())
        .with_diagnostic(
            ProviderDiagnostic::new(format!("Excel upstream rejected request: {code}"))
                .with_classification("upstream", "excel_rejected"),
        );
    if let Some(status) = old.upstream_status() {
        error = error.with_status(status);
    }
    if let Some(raw) = old.raw_upstream_error() {
        error = error.with_raw_upstream_error(raw.clone());
    }
    if let Some(response) = old.client_visible_upstream_response() {
        error = error.with_client_visible_upstream_response(
            gateway_core::error::ClientVisibleUpstreamResponse::new(
                response.status(),
                response.content_type().map(<[u8]>::to_vec),
                response.body().clone(),
            )
            .with_headers(response.headers().to_vec()),
        );
    }
    if let Some(id) = old.upstream_request_id() {
        error = error.with_upstream_request_id(id.clone());
    }
    failure.error = error;
    failure.account_failure = account_failure;
    failure.websocket_transport_retryable = false;
    failure.cyber_policy_failure = false;
    failure.capture_response_cookies = false;
    failure
}

fn isolate_endpoint_rate_limit(failure: &mut MappedProviderFailure) {
    if failure.error.kind() == ProviderErrorKind::RateLimited
        && matches!(
            failure.account_failure,
            None | Some(CodexAccountFailure::RateLimited { .. })
        )
    {
        suppress_recovery_with_kind(failure, ProviderErrorKind::RateLimited);
        failure.error = failure.error.clone().with_diagnostic(
            ProviderDiagnostic::new("Excel endpoint rate limited this request")
                .with_classification("upstream", "excel_rate_limited"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_core::error::{ClientVisibleUpstreamError, ClientVisibleUpstreamResponse};

    #[tokio::test]
    async fn excel_repair_checkpoint_reaches_second_http_request_through_core_stream() {
        use crate::transport::excel::ClientTools;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        for unknown in [false, true] {
            let server = MockServer::start().await;
            let calls = Arc::new(AtomicUsize::new(0));
            let recorded = calls.clone();
            Mock::given(path("/fixture"))
                .respond_with(move |_: &wiremock::Request| {
                    let attempt = recorded.fetch_add(1, Ordering::SeqCst);
                    let id = if attempt == 0 { "resp_original" } else { "resp_corrected" };
                    let arguments = if attempt > 0 {
                        json!({"summary":"cpr.custom/exec","code":"text(1)"})
                    } else if unknown {
                        json!({"code":json!({"name":"absent","arguments":{}}).to_string()})
                    } else {
                        json!({"summary":"Run","code":"text(1)"})
                    };
                    let created = json!({"type":"response.created","response":{"id":id,"model":"gpt-5.6-sol","status":"in_progress","output":[]}});
                    let completed = json!({"type":"response.completed","response":{"id":id,"model":"gpt-5.6-sol","status":"completed",
                        "output":[{"type":"function_call","id":format!("fc_{attempt}"),"call_id":format!("call_{attempt}"),"name":"run_officejs","arguments":arguments.to_string()}],
                        "usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12,"input_tokens_details":{"cached_tokens":0}}}});
                    ResponseTemplate::new(200).set_body_raw(format!("data: {created}\n\ndata: {completed}\n\n"), "text/event-stream")
                }).mount(&server).await;
            let mut request = CodexResponsesRequest::from_body(
                json!({"model":"gpt-5.6-sol","input":"run",
                "tools":[{"type":"custom","name":"exec"}]})
                .as_object()
                .unwrap()
                .clone(),
            );
            let tools = ClientTools::parse(request.body()).unwrap();
            request.excel = Some(ExcelPreparedRequest {
                body: prepare_request(request.body(), &tools, &Default::default(), None).unwrap(),
                tools,
                structured: None,
                _image_lease: None,
                image_limits: Default::default(),
                completed: Default::default(),
                usage: Default::default(),
                replay: None,
                endpoint: format!("{}/fixture", server.uri()),
            });
            let client = CodexBackendClient::new(
                reqwest::Client::builder().no_proxy().build().unwrap(),
                server.uri(),
                crate::config::OpenAiConfig::default().wire_profile_state(),
            );
            let mut body = client
                .create_response_stream_http_sse(
                    &request,
                    CodexRequestContext::auxiliary(
                        "Bearer fixture",
                        Some("workspace"),
                        "request",
                        None,
                    ),
                )
                .await
                .unwrap()
                .body;
            let events: EventStream = Box::pin(async_stream::stream! {
                let mut decoder = CodexCanonicalDecoder::new("gpt-5.6-sol").with_raw_sse_passthrough();
                while let Some(chunk) = body.next().await {
                    let chunk = chunk.expect("successful repair transport");
                    if chunk.is_empty() {
                        for event in failed_repair_metering(&request) { yield Ok(event); }
                        continue;
                    }
                    match decoder.push(&chunk) {
                        CodexCanonicalOutcome::Events(events) => for event in events { yield Ok(event); },
                        CodexCanonicalOutcome::Failed(failure) => panic!("unexpected decode failure: {failure:?}"),
                    }
                }
                assert!(matches!(decoder.finish(), CodexCanonicalOutcome::Events(_)));
            });
            let metadata = ProviderCallMetadata::new(
                ProviderKind::new("openai").unwrap(),
                UpstreamModelId::new("gpt-5.6-sol").unwrap(),
                gateway_core::account::ProviderAccountId::new("acct_fixture").unwrap(),
                UpstreamTransport::new("excel_http_sse").unwrap(),
            );
            let mut stream = ProviderStream::new(metadata, events, ());
            let mut checkpoints = Vec::new();
            let mut final_usage = None;
            let mut completed = false;
            while let Some(event) = stream.next().await {
                let mut event = event.expect("internal metering must not trigger MissingStarted");
                if let Some(checkpoint) = event.take_metering() {
                    assert!(!event.has_client_event());
                    checkpoints.push(checkpoint.usage().total_tokens.unwrap());
                }
                for fact in event.canonical_facts() {
                    if let GatewayEvent::Usage(usage) = fact {
                        final_usage = usage.total_tokens;
                    }
                    if let GatewayEvent::Completed(meta) = fact {
                        assert_eq!(meta.response_id(), "resp_original");
                        completed = true;
                    }
                }
            }
            assert!(completed);
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(checkpoints.first(), Some(&12));
            assert_eq!(checkpoints.last(), Some(&24));
            assert_eq!(final_usage, Some(24));
        }
    }

    #[test]
    fn excel_http_401_preserves_evidence_without_replay_or_sensitive_state_reason() {
        for (code, revoked) in [
            ("access_token_expired", false),
            ("token_revoked", true),
            ("token_invalidated", true),
            ("unknown", false),
        ] {
            let mut failure = classify_failure(rejection(code), true);
            failure.http_rejection_status = Some(401);
            failure.error = failure
                .error
                .with_status(401)
                .with_client_visible_upstream_response(ClientVisibleUpstreamResponse::new(
                    401,
                    Some(b"application/json".to_vec()),
                    bytes::Bytes::from_static(b"test body"),
                ))
                .with_replay_safe()
                .with_same_account_retry();
            failure.error_message = Some("Bearer private-fixture; request text".into());
            let raw = failure
                .error
                .client_visible_upstream_response()
                .unwrap()
                .body()
                .clone();
            assert_eq!(
                isolate_http_authentication_failure(&mut failure),
                Some(revoked)
            );
            assert_eq!(failure.error.kind(), ProviderErrorKind::Unauthorized);
            assert_eq!(failure.error.upstream_status(), Some(401));
            assert_eq!(
                failure
                    .error
                    .client_visible_upstream_response()
                    .unwrap()
                    .body(),
                &raw
            );
            assert_eq!(
                failure.error_message.as_deref(),
                Some("Excel upstream authentication failed")
            );
            assert!(failure.account_failure.is_none());
            assert!(!failure.error.replay_is_safe());
            assert!(failure.error.pre_delivery_retry().is_none());
        }
        for status in [None, Some(200), Some(403), Some(429), Some(500)] {
            let mut failure = rejection("forbidden");
            failure.http_rejection_status = status;
            failure.error_message = Some("fixture".into());
            assert_eq!(isolate_http_authentication_failure(&mut failure), None);
            assert_eq!(failure.error_message.as_deref(), Some("fixture"));
        }
    }

    #[test]
    fn excel_tool_file_id_error_is_actionable_and_not_retried() {
        let error = request_error(ExcelRequestError::ImageInput(
            "tool image file_id is unsupported; return the original image as Base64 or HTTPS image_url",
        ));
        assert_eq!(error.upstream_status(), Some(400));
        assert!(
            error
                .client_visible_upstream_error()
                .unwrap()
                .message()
                .contains("tool image file_id")
        );
        assert_eq!(error.send_state(), UpstreamSendState::NotSent);
        assert!(error.pre_delivery_retry().is_none());
        assert!(!error.replay_is_safe());
    }

    #[test]
    fn excel_auto_disable_requires_opt_in_and_actual_http_403() {
        let account = ProviderAccount::new(
            gateway_core::account::ProviderAccountId::new("acct_excel403").unwrap(),
            gateway_core::identity::ProviderKind::new("openai").unwrap(),
            "fixture".into(),
            None,
            "oauth".into(),
            gateway_core::account::CredentialRevision::new(1).unwrap(),
            None,
        )
        .with_responses_upstream(gateway_core::account::ResponsesUpstream::Excel);
        let mut failure = rejection("forbidden");
        failure.error = failure.error.with_status(403);
        failure.http_rejection_status = Some(403);
        assert!(!should_disable_on_403(&account, &failure));
        let account = account.with_excel_auto_disable_on_403(true);
        assert!(should_disable_on_403(&account, &failure));
        for status in [None, Some(200), Some(401), Some(429), Some(500)] {
            failure.http_rejection_status = status;
            assert!(!should_disable_on_403(&account, &failure));
        }
        failure.http_rejection_status = Some(403);
        let codex = account
            .clone()
            .with_responses_upstream(gateway_core::account::ResponsesUpstream::Codex);
        assert!(!should_disable_on_403(&codex, &failure));
        for code in [
            "basispoints_model_access_changed",
            " BASISPOINTS_MODEL_ACCESS_CHANGED ",
        ] {
            let mut excluded = rejection(code);
            excluded.http_rejection_status = Some(403);
            assert!(!should_disable_on_403(&account, &excluded));
        }
        failure.error = failure
            .error
            .with_upstream_code(OpaqueUpstreamValue::new("basispoints_model_access_changed"));
        assert!(!should_disable_on_403(&account, &failure));
    }

    #[test]
    fn excel_auto_disable_keeps_error_evidence_without_retry_or_account_mutation() {
        let mut failure = rejection("forbidden");
        failure.error = failure
            .error
            .with_status(403)
            .with_replay_safe()
            .with_same_account_retry()
            .with_upstream_code(OpaqueUpstreamValue::new("forbidden"))
            .with_upstream_request_id(OpaqueUpstreamValue::new("fixture-request"));
        failure.account_failure = Some(CodexAccountFailure::CredentialRevoked);
        failure.cyber_policy_failure = true;
        failure.capture_response_cookies = true;
        failure.websocket_transport_retryable = true;
        suppress_rejection_recovery(&mut failure);
        assert_eq!(failure.error.upstream_status(), Some(403));
        assert_eq!(failure.error.upstream_code().unwrap().as_str(), "forbidden");
        assert_eq!(
            failure.error.upstream_request_id().unwrap().as_str(),
            "fixture-request"
        );
        assert_eq!(
            failure
                .error
                .client_visible_upstream_response()
                .unwrap()
                .body()
                .as_ref(),
            b"test body"
        );
        assert!(failure.error.retry_after().is_none());
        assert!(failure.error.pre_delivery_retry().is_none());
        assert!(!failure.error.replay_is_safe());
        assert!(failure.account_failure.is_none());
        assert!(!failure.cyber_policy_failure);
        assert!(!failure.capture_response_cookies);
        assert!(!failure.websocket_transport_retryable);
    }

    #[tokio::test]
    async fn excel_auto_disable_http_provenance_does_not_trust_sse_error_status() {
        use crate::transport::excel::{ClientTools, ExcelPreparedRequest};
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        for status in [403, 401, 429, 500, 200] {
            server.reset().await;
            let body = if status == 200 {
                "data: {\"type\":\"error\",\"status\":403,\"error\":{\"code\":\"forbidden\",\"message\":\"fixture\"}}\n\n"
            } else {
                "{\"error\":{\"code\":\"forbidden\",\"message\":\"fixture\"}}"
            };
            Mock::given(method("POST"))
                .and(path("/fixture"))
                .respond_with(ResponseTemplate::new(status).set_body_raw(
                    body,
                    if status == 200 {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                ))
                .expect(1)
                .mount(&server)
                .await;
            let mut request = CodexResponsesRequest::from_body(
                json!({"model":"gpt-5.6-sol","input":[]})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            request.excel = Some(ExcelPreparedRequest {
                body: request.body().clone(),
                tools: ClientTools::default(),
                structured: None,
                _image_lease: None,
                image_limits: Default::default(),
                completed: Default::default(),
                usage: Default::default(),
                replay: None,
                endpoint: format!("{}/fixture", server.uri()),
            });
            let client = CodexBackendClient::new(
                reqwest::Client::builder().no_proxy().build().unwrap(),
                server.uri(),
                crate::config::OpenAiConfig::default().wire_profile_state(),
            );
            let failure = super::super::compact::collect_excel_compact(
                &client,
                &request,
                CodexRequestContext::auxiliary(
                    "Bearer fixture",
                    Some("workspace"),
                    "request",
                    None,
                ),
            )
            .await
            .err()
            .expect("fixture rejects");
            assert_eq!(
                failure.http_rejection_status,
                (status != 200).then_some(status)
            );
            let mut failure = failure;
            assert_eq!(
                isolate_http_authentication_failure(&mut failure).is_some(),
                status == 401
            );
        }
    }

    #[tokio::test]
    async fn excel_compact_requires_real_completion_and_encrypted_output() {
        use crate::transport::excel::{ClientTools, ExcelPreparedRequest};
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };

        let complete = json!({
            "id":"resp_compact_fixture","status":"completed",
            "output":[{"type":"compaction","encrypted_content":"opaque-fixture"}],
            "usage":{"input_tokens":12,"output_tokens":3,"total_tokens":15}
        });
        let created = json!({"type":"response.created","response":{
            "id":"resp_compact_fixture","status":"in_progress","output":[]
        }});
        let terminal = json!({"type":"response.completed","response":complete});
        for (body, content_type, valid) in [
            (
                format!("data: {created}\n\ndata: {terminal}\n\n"),
                "text/event-stream",
                true,
            ),
            (complete.to_string(), "application/json", false),
            (format!("data: {created}\n\n"), "text/event-stream", false),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/fixture"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(body, content_type))
                .expect(1)
                .mount(&server)
                .await;
            let mut request = CodexResponsesRequest::from_body(
                json!({"model":"gpt-5.6-sol","input":[]})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            request.excel = Some(ExcelPreparedRequest {
                body: request.body().clone(),
                tools: ClientTools::default(),
                structured: None,
                _image_lease: None,
                image_limits: Default::default(),
                completed: Default::default(),
                usage: Default::default(),
                replay: None,
                endpoint: format!("{}/fixture", server.uri()),
            });
            let client = CodexBackendClient::new(
                reqwest::Client::builder().no_proxy().build().unwrap(),
                server.uri(),
                crate::config::OpenAiConfig::default().wire_profile_state(),
            );
            let result = super::super::compact::collect_excel_compact(
                &client,
                &request,
                CodexRequestContext::auxiliary(
                    "Bearer fixture",
                    Some("workspace"),
                    "request",
                    None,
                ),
            )
            .await;
            if !valid {
                assert!(result.is_err());
                continue;
            }
            let response = result.unwrap_or_else(|_| panic!("valid compact fixture rejected"));
            let value: Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(value["object"], "response.compaction");
            assert_eq!(value["output"], complete["output"]);
            let (usage, _) =
                super::super::compact::compact_response_metering(b"{}", &response.body, None)
                    .unwrap();
            assert_eq!(usage.unwrap().total_tokens, Some(15));
        }
        for output in [
            json!([]),
            json!([{"type":"compaction","encrypted_content":""}]),
        ] {
            assert!(
                super::super::compact::compact_response_metering(
                    b"{}",
                    &serde_json::to_vec(&json!({"output":output})).unwrap(),
                    None,
                )
                .is_err()
            );
        }
    }

    fn rejection(code: &str) -> MappedProviderFailure {
        MappedProviderFailure::plain(
            provider_error(ProviderErrorKind::RateLimited, UpstreamSendState::Ambiguous)
                .with_status(429)
                .with_retry_after(Duration::from_secs(60))
                .with_pre_delivery_retry()
                .with_client_visible_upstream_error(ClientVisibleUpstreamError::new(
                    "test rejection",
                    Some(code.into()),
                    None,
                ))
                .with_client_visible_upstream_response(ClientVisibleUpstreamResponse::new(
                    429,
                    Some(b"application/json".to_vec()),
                    bytes::Bytes::from_static(b"test body"),
                )),
        )
    }

    #[test]
    fn excel_permissions_do_not_cool_or_revoke_the_account() {
        for (code, kind) in [
            ("workspace_not_allowed", ProviderErrorKind::PermissionDenied),
            (
                "basispoints_model_access_changed",
                ProviderErrorKind::Unsupported,
            ),
        ] {
            let failure = classify_failure(rejection(code), true);
            assert_eq!(failure.error.kind(), kind);
            assert_eq!(failure.error.upstream_status(), Some(429));
            assert!(failure.error.retry_after().is_none());
            assert!(failure.error.pre_delivery_retry().is_none());
            assert!(failure.account_failure.is_none());
            assert_eq!(
                failure
                    .error
                    .client_visible_upstream_response()
                    .unwrap()
                    .body()
                    .as_ref(),
                b"test body"
            );
        }
        for (excel, code) in [
            (false, "workspace_not_allowed"),
            (true, "rate_limit_exceeded"),
        ] {
            let failure = classify_failure(rejection(code), excel);
            assert_eq!(failure.error.kind(), ProviderErrorKind::RateLimited);
            assert_eq!(failure.error.retry_after(), Some(Duration::from_secs(60)));
        }
    }

    #[test]
    fn excel_explicit_authentication_failures_keep_account_recovery() {
        for code in ["token_expired", "token_revoked"] {
            let failure = classify_failure(rejection(code), true);
            assert_eq!(failure.error.kind(), ProviderErrorKind::Unauthorized);
            assert!(matches!(
                (code, failure.account_failure),
                (
                    "token_expired",
                    Some(CodexAccountFailure::CredentialExpired)
                ) | (
                    "token_revoked",
                    Some(CodexAccountFailure::CredentialRevoked)
                )
            ));
        }
    }

    #[test]
    fn excel_rate_limit_isolated_but_shared_quota_and_native_route_unchanged() {
        let mut rate = rejection("rate_limit_exceeded");
        rate.account_failure = Some(CodexAccountFailure::RateLimited {
            retry_after: Some(Duration::from_secs(60)),
        });
        let excel = classify_failure(rate, true);
        assert!(excel.account_failure.is_none());
        assert!(excel.error.pre_delivery_retry().is_none());
        assert_eq!(excel.error.upstream_status(), Some(429));
        assert!(
            classify_failure(rejection("rate_limit_exceeded"), false)
                .error
                .pre_delivery_retry()
                .is_some()
        );
        let mut quota = rejection("usage_limit_reached");
        quota.account_failure = Some(CodexAccountFailure::QuotaExhausted);
        assert!(matches!(
            classify_failure(quota, true).account_failure,
            Some(CodexAccountFailure::QuotaExhausted)
        ));
    }
}
