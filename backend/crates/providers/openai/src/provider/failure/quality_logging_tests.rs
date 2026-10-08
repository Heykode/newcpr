use super::*;
use crate::transport::protocol::responses::ResponsesSseFailure;
use crate::transport::websocket::CodexWebSocketUpstreamError;
use std::{io, sync::Mutex};

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn logged(run: impl FnOnce()) -> String {
    let logs = Logs::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    let bytes = logs.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn quality_logging_suppresses_only_explicit_limits_across_http_and_websocket() {
    let cases = [
        (429, json!({"type":"usage_limit_reached"}), true),
        (429, json!({"code":"rate_limit_exceeded"}), true),
        (429, json!({"type":"rate_limit_error"}), true),
        (429, json!({"code":"insufficient_quota"}), true),
        (
            429,
            json!({"code":"workspace_member_credits_depleted"}),
            true,
        ),
        (429, json!({"code":"unknown_rejection"}), false),
        (
            429,
            json!({"code":"websocket_connection_limit_reached"}),
            false,
        ),
        (429, json!({"code":"server_is_overloaded"}), false),
        (401, json!({"code":"invalid_api_key"}), false),
        (403, json!({"code":"permission_denied"}), false),
        (500, json!({"code":"server_error"}), false),
    ];
    for (status, mut error, expected_limit) in cases {
        error["message"] = json!("fixture error detail must stay redacted");
        let body = json!({"type":"error","status_code":status,"error":error});
        for quality_check in [false, true] {
            let context = UpstreamErrorLogContext {
                request_id: "req_quality_logging",
                account_id: "acct_quality_logging",
                attempt_index: 1,
                websocket_connection_id: None,
                quality_check,
            };
            for transport in [
                CodexBackendTransport::HttpSse,
                CodexBackendTransport::HttpJson,
            ] {
                let error = CodexClientError::Upstream {
                    status: reqwest::StatusCode::from_u16(status).unwrap(),
                    body: body.to_string(),
                    client_response: None,
                    retry_after_seconds: Some(60),
                    diagnostics: Box::default(),
                    set_cookie_headers: Vec::new(),
                    rate_limit_headers: Vec::new(),
                    transport,
                    transport_metrics: Box::default(),
                    send_phase: CodexUpstreamSendPhase::AfterPayload,
                };
                let before = error.upstream_failure().unwrap();
                let logs = logged(|| log_client_upstream_error(context, &error));
                assert_log(&logs, quality_check && expected_limit);
                let after = error.upstream_failure().unwrap();
                assert_eq!(before.category(), after.category());
                assert_eq!(before.raw_body, after.raw_body);
            }
            let opening = CodexClientError::WebSocket(CodexWebSocketExchangeError::Upstream(
                Box::new(CodexWebSocketUpstreamError {
                    status_code: status,
                    retry_after_seconds: Some(60),
                    body: body.to_string(),
                    client_response: None,
                    set_cookie_headers: Vec::new(),
                    diagnostics: CodexUpstreamDiagnostics::default(),
                    send_phase: CodexUpstreamSendPhase::BeforePayload,
                }),
            ));
            assert_log(
                &logged(|| log_client_upstream_error(context, &opening)),
                quality_check && expected_limit,
            );
            for transport in [
                CodexBackendTransport::HttpSse,
                CodexBackendTransport::WebSocket,
            ] {
                let failure = CodexCanonicalError::Upstream(Box::new(
                    ResponsesSseFailure::from_event("error", &body),
                ));
                assert_log(
                    &logged(|| log_canonical_upstream_error(context, transport, &failure)),
                    quality_check && expected_limit,
                );
            }
        }
    }
}

fn assert_log(logs: &str, suppressed: bool) {
    assert_eq!(logs.is_empty(), suppressed, "unexpected logging: {logs}");
    if !suppressed {
        assert!(logs.contains("OpenAI upstream returned an error payload"));
        assert!(logs.contains("req_quality_logging"));
    }
    assert!(!logs.contains("fixture error detail must stay redacted"));
}
