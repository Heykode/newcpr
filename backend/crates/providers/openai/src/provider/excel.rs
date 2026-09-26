use sha2::{Digest, Sha256};

use super::*;
use crate::transport::excel::{
    ClientTools, ExcelPreparedRequest, ExcelRequestError, StructuredOutput, prepare_request,
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
    let restored = crate::transport::excel::replay::restore(
        replay,
        owner.clone(),
        cache_anchor,
        request.previous_response_id(),
        source
            .get("input")
            .ok_or_else(|| request_error(ExcelRequestError::Input))?,
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
    let tools = ClientTools::parse(&source).map_err(request_error)?;
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
    let image_lease = image_relay.stage(&mut body).map_err(request_error)?;
    crate::transport::request::clear_turn_state(request);
    request.force_http_sse = true;
    request.use_websocket = false;
    request.excel = Some(ExcelPreparedRequest {
        body,
        tools,
        structured,
        _image_lease: image_lease,
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
    if error == ExcelRequestError::ImageRelay {
        return provider_error(ProviderErrorKind::Unavailable, UpstreamSendState::NotSent)
            .with_status(503)
            .with_upstream_code(OpaqueUpstreamValue::new("excel_image_relay_unavailable"));
    }
    if error == ExcelRequestError::History {
        return continuation_replay_required_error("excel_history_unavailable");
    }
    provider_error(
        ProviderErrorKind::InvalidRequest,
        UpstreamSendState::NotSent,
    )
    .with_status(400)
    .with_upstream_code(OpaqueUpstreamValue::new("excel_unsupported_request"))
    .with_diagnostic(ProviderDiagnostic::new(error.to_string()))
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
) {
    if !excel || !allows_mutation || !should_disable_on_403(account, failure) {
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

fn suppress_rejection_recovery(failure: &mut MappedProviderFailure) {
    let old = &failure.error;
    let mut error = provider_error(ProviderErrorKind::PermissionDenied, old.send_state());
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
    failure.rate_limit_headers.clear();
}

pub(super) fn classify_failure(
    mut failure: MappedProviderFailure,
    excel: bool,
) -> MappedProviderFailure {
    if !excel {
        return failure;
    }
    let Some(detail) = failure.error.client_visible_upstream_error() else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_core::error::{ClientVisibleUpstreamError, ClientVisibleUpstreamResponse};

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
}
