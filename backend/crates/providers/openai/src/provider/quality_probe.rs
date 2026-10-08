//! Local state comparison on the normal quality request path; no collector or shared cookies.

use gateway_core::operation::quality_probe::{QualityProbeStep, StateProbeReason, StateProbeShot};

use super::*;

fn rejected(step: &QualityProbeStep, reason: StateProbeReason) -> ProviderError {
    step.observe(
        StateProbeShot {
            reason: Some(reason),
            ..Default::default()
        },
        None,
    );
    let error = provider_error(ProviderErrorKind::Unsupported, UpstreamSendState::NotSent);
    if reason == StateProbeReason::AccountChanged {
        error
            .with_diagnostic(
                ProviderDiagnostic::new("检查期间账号或出口状态发生变化，本轮无结论")
                    .with_classification("quality_probe", "account_changed"),
            )
            .with_client_visible_upstream_error(ClientVisibleUpstreamError::new(
                "检查期间账号或出口状态发生变化，本轮无结论",
                Some("account_changed".to_owned()),
                None,
            ))
    } else {
        error
    }
}

pub(super) fn initialize(
    request: &mut CodexResponsesRequest,
    is_quality_check: bool,
    request_id: &str,
) -> Result<(), ProviderError> {
    let Some(step) = &request.quality_probe else {
        return Ok(());
    };
    if !is_quality_check {
        return Err(rejected(step, StateProbeReason::UnsupportedAccount));
    }
    // Each shot is a fresh session, but the normal account-scoped QX projection
    // still owns its outbound IDs. Do this before identity/affinity derivation.
    request.client_session_id = Some(request_id.to_owned());
    Ok(())
}

pub(super) fn prepare(
    request: &mut CodexResponsesRequest,
    context: &AttemptContext,
    account: &ProviderAccount,
) -> Result<(), ProviderError> {
    let Some(step) = request.quality_probe.clone() else {
        return Ok(());
    };
    if !context.is_native_quality_probe() || !step.begin() {
        return Err(rejected(&step, StateProbeReason::RepeatedAttempt));
    }
    if account.authentication_kind() != "oauth" {
        return Err(rejected(&step, StateProbeReason::UnsupportedAccount));
    }
    prepare_payload(request);
    request.turn_state = None;
    request.responses_lite = None;
    if step.is_continuation() {
        let input = step
            .input()
            .ok_or_else(|| rejected(&step, StateProbeReason::MissingEvidence))?;
        if input.provider() != PROVIDER_NAME
            || input.payload().get("owner") != Some(&owner(account))
        {
            return Err(rejected(&step, StateProbeReason::AccountChanged));
        }
        request.turn_state = input
            .payload()
            .get("ticket")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if request.turn_state.is_none() {
            return Err(rejected(&step, StateProbeReason::MissingTicket));
        }
    }
    Ok(())
}

fn prepare_payload(request: &mut CodexResponsesRequest) {
    // State evidence is response-local HTTP metadata. Do not open a WebSocket
    // only to fall back; keep the account identity and egress preparation intact.
    request.use_websocket = false;
    request.force_http_sse = true;
    let body = request.body_mut();
    body.insert("instructions".to_owned(), json!("Reply with OK."));
    body.insert("parallel_tool_calls".to_owned(), json!(true));
    body.insert("include".to_owned(), json!(["reasoning.encrypted_content"]));
}

fn owner(account: &ProviderAccount) -> Value {
    json!({
        "id": account.id().as_str(),
        "revision": account.revision().get(),
        "user": account.upstream_user_id(),
        "workspace": account.upstream_account_id(),
    })
}

pub(super) fn cookie_header(
    request: &CodexResponsesRequest,
    client: &CodexBackendClient,
    account: &ProviderAccount,
) -> Result<Option<SecretString>, ProviderError> {
    let Some(step) = &request.quality_probe else {
        return Ok(None);
    };
    let scope = client
        .quality_probe_scope(account)
        .map_err(|reason| rejected(step, reason))?;
    let Some(input) = step.input() else {
        return Ok(None);
    };
    if input.payload().get("scope") != Some(&scope) {
        return Err(rejected(step, StateProbeReason::AccountChanged));
    }
    Ok(input
        .payload()
        .get("cookies")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(|value| SecretString::from(value.to_owned())))
}

pub(super) fn observe(
    request: &CodexResponsesRequest,
    client: &CodexBackendClient,
    account: &ProviderAccount,
    response: &CodexBackendStreamingResponse,
) {
    let Some(step) = &request.quality_probe else {
        return;
    };
    let ticket = response.turn_state.as_deref().unwrap_or("").trim();
    let mut shot = classify(
        step.is_continuation(),
        request.turn_state.as_deref(),
        ticket,
        response.transport,
        response.diagnostics.status_code,
    );
    let mut state = None;
    if !step.is_continuation() && shot.reason.is_none() {
        let cookies = route_cookies(&response.set_cookie_headers);
        match client.quality_probe_scope(account) {
            Ok(scope) => {
                let payload = json!({
                    "owner": owner(account), "scope": scope, "ticket": ticket, "cookies": cookies,
                });
                state = payload.as_object().and_then(|payload| {
                    ProviderSessionState::new(PROVIDER_NAME, payload.clone()).ok()
                });
                if state.is_none() {
                    shot.reason = Some(StateProbeReason::MissingEvidence);
                }
            }
            Err(reason) => shot.reason = Some(reason),
        }
    }
    step.observe(shot, state);
}

fn classify(
    continuation: bool,
    previous: Option<&str>,
    ticket: &str,
    transport: CodexBackendTransport,
    status: Option<u16>,
) -> StateProbeShot {
    let mut shot = StateProbeShot {
        transport: Some(actual_transport_name(transport).to_owned()),
        status,
        ticket_length: ticket.len(),
        ..Default::default()
    };
    if transport != CodexBackendTransport::HttpSse {
        // A pooled WS opening is not evidence for the current response. The HTTP
        // criterion has not been validated against response-scoped WS metadata.
        shot.reason = Some(StateProbeReason::UnsupportedTransport);
    } else if shot.status != Some(200) {
        shot.reason = Some(StateProbeReason::RequestFailed);
    } else if continuation {
        if previous.is_none_or(str::is_empty) {
            shot.reason = Some(StateProbeReason::MissingTicket);
        } else {
            shot.changed = Some(!ticket.is_empty() && previous != Some(ticket));
        }
    } else if ticket.is_empty() {
        shot.reason = Some(StateProbeReason::MissingTicket);
    }
    shot
}

fn route_cookies(headers: &[String]) -> String {
    let mut values = std::collections::BTreeMap::new();
    for header in headers {
        let Ok(cookie) = cookie::Cookie::parse(header.as_str()) else {
            continue;
        };
        if !matches!(cookie.name(), "__cflb" | "__oailb") {
            continue;
        }
        if cookie.value().is_empty()
            || cookie.value().len() > 4096
            || cookie
                .value()
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte == b';')
            || cookie.max_age().map_or_else(
                || {
                    cookie
                        .expires_datetime()
                        .is_some_and(|time| time.unix_timestamp() <= chrono::Utc::now().timestamp())
                },
                |age| age.whole_seconds() <= 0,
            )
        {
            values.remove(cookie.name());
        } else {
            values.insert(cookie.name().to_owned(), cookie.value().to_owned());
        }
    }
    // The reference does not require either cookie to exist. Never borrow unrelated
    // account cookies to fill a missing route cookie in this isolated exchange.
    values
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_probe_explains_inconclusive_result_without_enabling_retries() {
        use gateway_core::operation::quality_probe::{QualityProbeExchange, StateProbeVerdict};
        let exchange = QualityProbeExchange::default();
        let error = rejected(&exchange.step(true), StateProbeReason::AccountChanged);
        assert_eq!(exchange.report().verdict, StateProbeVerdict::Inconclusive);
        assert_eq!(exchange.report().reason, StateProbeReason::AccountChanged);
        assert_eq!(error.kind(), ProviderErrorKind::Unsupported);
        assert_eq!(error.send_state(), UpstreamSendState::NotSent);
        assert!(!error.allows_pre_delivery_retry());
        assert_eq!(error.diagnostic().unwrap().code(), Some("account_changed"));
        assert!(error.diagnostic().unwrap().as_str().contains("本轮无结论"));
        assert_eq!(
            error.client_visible_upstream_error().unwrap().code(),
            Some("account_changed")
        );
    }

    #[test]
    fn quality_probe_sessions_are_unique_with_normal_account_identity_projection() {
        use crate::transport::qx_application::{apply_response_headers, identity_seed};
        use gateway_core::operation::quality_probe::QualityProbeExchange;
        use reqwest::header::HeaderMap;

        let body = json!({"model": "probe-model", "input": "Reply with OK."});
        let mut ordinary = CodexResponsesRequest::from_body(body.as_object().unwrap().clone());
        ordinary.client_session_id = Some("ordinary-session".into());
        initialize(&mut ordinary, true, "must-not-replace").unwrap();
        assert_eq!(
            ordinary.client_session_id.as_deref(),
            Some("ordinary-session")
        );
        assert_eq!(serde_json::to_value(&ordinary).unwrap(), body);

        let exchange = QualityProbeExchange::default();
        let mut sessions = Vec::new();
        for (continuation, id) in [(false, "shot-one"), (true, "shot-two")] {
            let mut request = CodexResponsesRequest::from_body(body.as_object().unwrap().clone());
            request.quality_probe = Some(exchange.step(continuation));
            request.use_websocket = true;
            request.client_api_key_id = Some("admin_quality_check".into());
            initialize(&mut request, true, id).unwrap();
            request.identity_seed = Some(identity_seed(&request, id));
            let mut headers = HeaderMap::new();
            apply_response_headers(
                &mut headers,
                &request,
                CodexRequestContext::auxiliary(
                    "Bearer synthetic-token",
                    Some("fixture-account"),
                    id,
                    Some("fixture-installation"),
                ),
            )
            .unwrap();
            let session = headers["session_id"].to_str().unwrap().to_owned();
            assert_ne!(session, id);
            assert_eq!(headers["session-id"], session);
            sessions.push(session);
            assert!(request.use_websocket);
            assert!(!request.force_http_sse);
            assert_eq!(serde_json::to_value(&request).unwrap(), body);
        }
        assert_ne!(sessions[0], sessions[1]);

        let mut untrusted = ordinary.clone();
        untrusted.quality_probe = Some(exchange.step(false));
        assert!(initialize(&mut untrusted, false, "untrusted").is_err());
        assert_eq!(untrusted.client_session_id, ordinary.client_session_id);
    }

    #[test]
    fn quality_probe_payload_matches_reference_and_forces_http_without_changing_identity() {
        let body = json!({
            "model": "probe-model",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Reply with OK."}],
            }],
            "stream": true,
            "store": false,
        });
        let original = CodexResponsesRequest::from_body(body.as_object().unwrap().clone());
        let mut request = original.clone();
        request.use_websocket = true;
        request.client_session_id = Some("account-scoped-session".into());
        prepare_payload(&mut request);
        let mut expected = body;
        expected["instructions"] = json!("Reply with OK.");
        expected["parallel_tool_calls"] = json!(true);
        expected["include"] = json!(["reasoning.encrypted_content"]);
        assert_eq!(serde_json::to_value(&request).unwrap(), expected);
        assert!(!request.use_websocket);
        assert!(request.force_http_sse);
        assert_eq!(
            request.client_session_id.as_deref(),
            Some("account-scoped-session")
        );
        assert!(!original.body().contains_key("instructions"));
        assert!(!original.body().contains_key("include"));
    }

    #[test]
    fn quality_probe_matches_reference_comparison_without_length_or_cookie_guesses() {
        let http = CodexBackendTransport::HttpSse;
        assert_eq!(
            classify(false, None, "", http, Some(200)).reason,
            Some(StateProbeReason::MissingTicket)
        );
        for length in [32, 332, 780] {
            let ticket = "a".repeat(length);
            assert!(
                classify(false, None, &ticket, http, Some(200))
                    .reason
                    .is_none()
            );
            assert_eq!(
                classify(true, Some(&ticket), &ticket, http, Some(200)).changed,
                Some(false)
            );
            assert_eq!(
                classify(true, Some(&ticket), "", http, Some(200)).changed,
                Some(false)
            );
            assert_eq!(
                classify(true, Some(&ticket), "different", http, Some(200)).changed,
                Some(true)
            );
        }
        for status in [None, Some(401), Some(429), Some(500)] {
            let shot = classify(true, Some("first"), "different", http, status);
            assert_eq!(shot.reason, Some(StateProbeReason::RequestFailed));
            assert!(shot.changed.is_none());
        }
        assert_eq!(
            classify(
                true,
                Some("first"),
                "different",
                CodexBackendTransport::WebSocket,
                Some(200)
            )
            .reason,
            Some(StateProbeReason::UnsupportedTransport)
        );
    }

    #[test]
    fn quality_probe_route_cookies_are_optional_isolated_and_respect_deletion() {
        assert_eq!(route_cookies(&[]), "");
        assert_eq!(
            route_cookies(&[
                "__cflb=one; Path=/; HttpOnly".into(),
                "session=private; Path=/".into(),
                "__oailb=two; Secure".into(),
                "__cflb=; Max-Age=0".into(),
            ]),
            "__oailb=two"
        );
        assert_eq!(route_cookies(&["__oailb=bad\rvalue".into()]), "");
    }
}
