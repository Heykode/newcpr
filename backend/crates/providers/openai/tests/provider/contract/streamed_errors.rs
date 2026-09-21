use super::*;
use gateway_core::error::ProviderError;

const ACCOUNT_ID: &str = "acct_provider_contract";
const MISSING_ITEM: &str = "Item with id 'rs_missing_fixture' not found. \
    Items are not persisted when `store` is set to false.";

#[tokio::test]
async fn repeated_streamed_404_errors_do_not_invalidate_accounts() {
    for websocket in [false, true] {
        for event in ["error", "response.failed"] {
            let failure = error_event(
                event,
                404,
                json!({
                    "type": "invalid_request_error",
                    "code": null,
                    "param": "input",
                    "message": MISSING_ITEM,
                }),
            );
            let (errors, store, revision) = exchange_errors(websocket, failure.clone(), 3).await;
            let account = store.account(ACCOUNT_ID).unwrap();
            assert_eq!(
                account.credential_state(),
                CredentialState::Ready,
                "{event}, websocket={websocket}: request errors must not disable the account",
            );
            assert_eq!(account.last_error_reason(), None);
            assert_eq!(account.revision().get(), revision);
            for error in errors {
                assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
                assert_eq!(error.upstream_status(), Some(404));
                assert_eq!(error.send_state(), UpstreamSendState::Sent);
                assert!(!error.replay_is_safe());
                assert!(!error.allows_pre_delivery_retry());
                assert!(!provider_openai::openai_failure_affects_account_score(
                    &error
                ));
                let visible = error.client_visible_upstream_error().unwrap();
                assert_eq!(visible.error_type(), Some("invalid_request_error"));
                assert_eq!(visible.code(), None);
                assert_eq!(visible.message(), MISSING_ITEM);
                assert_eq!(
                    serde_json::from_str::<Value>(error.raw_upstream_error().unwrap().as_str())
                        .unwrap(),
                    failure,
                );
            }
        }
    }
}

#[tokio::test]
async fn streamed_404_preserves_explicit_account_failure_signals() {
    for websocket in [false, true] {
        for (code, expected_state, expected_reason, expected_kind) in [
            (
                "token_expired",
                CredentialState::Expired,
                AccountErrorReason::AccessTokenExpired,
                ProviderErrorKind::Unauthorized,
            ),
            (
                "token_revoked",
                CredentialState::Expired,
                AccountErrorReason::CredentialExpired,
                ProviderErrorKind::Unauthorized,
            ),
            (
                "account_banned",
                CredentialState::Banned,
                AccountErrorReason::AccountBanned,
                ProviderErrorKind::PermissionDenied,
            ),
            (
                "deactivated_workspace",
                CredentialState::Banned,
                AccountErrorReason::AccountBanned,
                ProviderErrorKind::PermissionDenied,
            ),
            (
                "identity_verification_required",
                CredentialState::Invalid,
                AccountErrorReason::AccountUnverified,
                ProviderErrorKind::PermissionDenied,
            ),
        ] {
            let failure = error_event(
                "error",
                404,
                json!({
                    "type": "invalid_request_error",
                    "code": code,
                    "message": "synthetic account rejection",
                }),
            );
            let (errors, store, _) = exchange_errors(websocket, failure, 1).await;
            let account = store.account(ACCOUNT_ID).unwrap();
            assert_eq!(account.credential_state(), expected_state, "{code}");
            assert_eq!(account.last_error_reason(), Some(expected_reason), "{code}");
            assert_eq!(errors[0].kind(), expected_kind, "{code}");
            assert_eq!(errors[0].upstream_status(), Some(404));
            assert_eq!(
                errors[0].client_visible_upstream_error().unwrap().code(),
                Some(code),
            );
        }
    }
}

fn error_event(event: &str, status: u16, error: Value) -> Value {
    if event == "response.failed" {
        json!({
            "type": event,
            "status_code": status,
            "response": {
                "id": "resp_streamed_error",
                "status": "failed",
                "error": error,
            },
        })
    } else {
        json!({"type": event, "status": status, "error": error})
    }
}

async fn exchange_errors(
    websocket: bool,
    failure: Value,
    count: usize,
) -> (Vec<ProviderError>, Arc<MemoryAccountStore>, u64) {
    let http = MockServer::start().await;
    let (url, server) = if websocket {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for _ in 0..count {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = accept_codex_test_websocket(stream).await;
                socket.next().await.unwrap().unwrap();
                socket
                    .send(Message::Text(failure.to_string().into()))
                    .await
                    .unwrap();
                socket.close(None).await.unwrap();
            }
        });
        (url, Some(server))
    } else {
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    "event: {}\ndata: {failure}\n\n",
                    failure["type"].as_str().unwrap(),
                ),
                "text/event-stream",
            ))
            .expect(u64::try_from(count).unwrap())
            .mount(&http)
            .await;
        (http.uri(), None)
    };
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, ACCOUNT_ID).await;
    let revision = store.account(ACCOUNT_ID).unwrap().revision().get();
    let provider = provider_with_base_url_and_retry_budget(&store, url, 0);
    let mut errors = Vec::new();
    for index in 0..count {
        let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object(
                "openai",
                json!({"model": "gpt-5.4", "input": "hello"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap()
            .with_context(Map::from_iter([(
                "use_websocket".to_owned(),
                json!(websocket),
            )])),
        ));
        let mut stream = provider
            .execute(
                planned_request("openai", operation),
                context(
                    &format!("req_streamed_error_{index}"),
                    CancellationToken::new(),
                ),
            )
            .await
            .unwrap();
        let error = loop {
            match timeout(Duration::from_secs(5), stream.next())
                .await
                .unwrap()
            {
                Some(Ok(_)) => {}
                Some(Err(error)) => break error,
                None => panic!("upstream error must fail the request"),
            }
        };
        errors.push(error);
    }
    if let Some(server) = server {
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    } else {
        assert_eq!(http.received_requests().await.unwrap().len(), count);
    }
    (errors, store, revision)
}
