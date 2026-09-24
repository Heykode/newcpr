use super::*;
use gateway_core::error::ProviderError;

const ACCOUNT_ID: &str = "acct_provider_contract";
const MISSING_ITEM: &str = "Item with id 'rs_missing_fixture' not found. \
    Items are not persisted when `store` is set to false.";

#[tokio::test]
async fn websocket_pong_exit_diagnosis_preserves_ambiguous_send_and_account_health() {
    for reuse in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, ACCOUNT_ID).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (ping_seen_tx, ping_seen_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = accept_codex_test_websocket(stream).await;
            websocket.next().await.unwrap().unwrap();
            if reuse {
                for event in [
                    json!({"type":"response.created","response":{"id":"resp_warmup","model":"gpt-5.4"}}),
                    json!({"type":"response.completed","response":{"id":"resp_warmup","model":"gpt-5.4","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}),
                ] {
                    websocket
                        .send(Message::Text(event.to_string().into()))
                        .await
                        .unwrap();
                }
                websocket.next().await.unwrap().unwrap();
            }
            for event in [
                json!({"type":"response.created","response":{"id":"resp_pong","model":"gpt-5.4"}}),
                json!({"type":"response.output_text.delta","delta":"private-response-body"}),
            ] {
                websocket
                    .send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
            }
            assert!(matches!(
                websocket.next().await.unwrap().unwrap(),
                Message::Ping(_)
            ));
            ping_seen_tx.send(()).unwrap();
            // Do not poll again: tungstenite would flush its automatic Pong.
            futures::future::pending::<()>().await;
        });
        let operation = Operation::Generate(websocket_fixture_request(
            generate_with_persisted_session_context(
                ACCOUNT_ID,
                "conversation-pong-exit",
                "session-pong-exit",
                "turn-pong-exit",
            ),
        ));
        let provider = provider_with_base_url(&store, base_url);
        if reuse {
            let mut warmup = provider
                .execute(
                    planned_request("openai", operation.clone()),
                    context("req_pong_warmup", CancellationToken::new()),
                )
                .await
                .unwrap();
            while let Some(event) = warmup.next().await {
                event.unwrap();
            }
        }
        let mut stream = provider
            .execute(
                planned_request("openai", operation),
                context("req_pong_exit", CancellationToken::new()),
            )
            .await
            .unwrap();
        let mut pool_kind = None;
        loop {
            let event = stream.next().await.unwrap().unwrap();
            if let Some(observation) = event.response_observation() {
                pool_kind = observation.websocket_pool().or(pool_kind);
            }
            if event.has_client_event() {
                break;
            }
        }
        assert_eq!(
            pool_kind,
            Some(if reuse {
                WebSocketPoolKind::Reuse
            } else {
                WebSocketPoolKind::New
            })
        );
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(25)).await;
        // A real I/O barrier must not auto-advance the paused keepalive clock.
        tokio::time::resume();
        timeout(Duration::from_secs(5), ping_seen_rx)
            .await
            .expect("server observes Ping")
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(31)).await;
        let error = loop {
            match stream.next().await {
                Some(Ok(_)) => {}
                Some(Err(error)) => break error,
                None => panic!("pump timeout must remain a provider failure"),
            }
        };
        tokio::time::resume();
        server.abort();
        let _ = server.await;
        assert_eq!(error.send_state(), UpstreamSendState::Ambiguous);
        assert!(!error.replay_is_safe());
        assert_eq!(error.pre_delivery_retry(), None);
        assert_eq!(error.kind(), ProviderErrorKind::Transport);
        assert_eq!(
            error.connection_observation().unwrap().exit_reason(),
            "pong_timeout"
        );
        let diagnostic = error.diagnostic().unwrap();
        assert_eq!(diagnostic.stage(), Some("receive"));
        assert_eq!(diagnostic.code(), Some("pong_timeout"));
        assert_eq!(
            diagnostic.as_str(),
            "OpenAI WebSocket stream ended before a terminal response (pong_timeout); local keepalive timeout after 30s; last event type: response.output_text.delta"
        );
        assert!(error.raw_upstream_error().is_none());
        assert!(error.client_visible_upstream_error().is_none());
        assert!(!provider_openai::openai_failure_affects_account_score(
            &error
        ));
        let account = store.account(ACCOUNT_ID).unwrap();
        assert!(account.enabled());
        assert_eq!(account.credential_state(), CredentialState::Ready);
        assert_eq!(account.last_error_reason(), None);
    }
}

#[tokio::test]
async fn websocket_close_diagnosis_requires_an_actual_close_frame() {
    for code in [None, Some(CloseCode::Normal), Some(CloseCode::Size)] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, ACCOUNT_ID).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = accept_codex_test_websocket(stream).await;
            websocket.next().await.unwrap().unwrap();
            websocket
                .send(Message::Close(code.map(|code| CloseFrame {
                    code,
                    reason: "private-close-detail".into(),
                })))
                .await
                .unwrap();
            let _ = websocket.next().await;
        });
        let operation = Operation::Generate(websocket_fixture_request(
            generate_with_persisted_session_context(
                ACCOUNT_ID,
                "conversation-close-exit",
                "session-close-exit",
                "turn-close-exit",
            ),
        ));
        let result = provider_with_base_url(&store, base_url)
            .execute(
                planned_request("openai", operation),
                context("req_close_exit", CancellationToken::new()),
            )
            .await;
        let error = match result {
            Err(error) => error,
            Ok(mut stream) => loop {
                match stream.next().await {
                    Some(Ok(_)) => {}
                    Some(Err(error)) => break error,
                    None => panic!("Close before terminal must fail"),
                }
            },
        };
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(error.send_state(), UpstreamSendState::Ambiguous);
        assert!(!error.replay_is_safe());
        assert_eq!(error.pre_delivery_retry(), None);
        let diagnostic = error.diagnostic().unwrap();
        assert_eq!(diagnostic.stage(), Some("receive"));
        assert_eq!(
            diagnostic.code(),
            Some(if code == Some(CloseCode::Size) {
                "message_too_big"
            } else {
                "upstream_close"
            })
        );
        assert!(!diagnostic.as_str().contains("private-close-detail"));
        let raw: Value =
            serde_json::from_str(error.raw_upstream_error().unwrap().as_str()).unwrap();
        assert_eq!(raw["type"], "websocket.close");
        assert_eq!(
            raw["code"],
            code.map(u16::from).map_or(Value::Null, Value::from)
        );
        assert!(!provider_openai::openai_failure_affects_account_score(
            &error
        ));
    }
}

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
