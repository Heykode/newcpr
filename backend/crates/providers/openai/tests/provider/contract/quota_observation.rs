use super::*;

const ACCOUNT: &str = "acct_quota_observation";
const RESET: u64 = 2_000_000_000;

#[tokio::test]
async fn failed_response_headers_do_not_restore_exhausted_access() {
    for streaming in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, ACCOUNT).await;
        let account = store.account(ACCOUNT).unwrap();
        store
            .apply_quota_access(QuotaAccessChange {
                account_id: account.id().clone(),
                expected_revision: account.revision(),
                state: QuotaState::exhausted(
                    QuotaEvidence::ProviderDenied,
                    SystemTime::now(),
                    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(RESET)),
                ),
            })
            .await
            .unwrap();
        let server = MockServer::start().await;
        let error = json!({
            "type": "error",
            "error": {"type": "invalid_request_error", "message": "invalid request"}
        });
        let response = if streaming {
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("event: error\ndata: {error}\n\n"))
        } else {
            ResponseTemplate::new(400).set_body_json(&error)
        };
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(response.insert_header("x-codex-primary-used-percent", "8"))
            .expect(1)
            .mount(&server)
            .await;
        let provider = provider_with_base_url(&store, server.uri());
        let mut stream = provider
            .execute(
                planned_request("openai", http_generate_operation()),
                diagnostic_context("req_failed_quota_observation", ACCOUNT),
            )
            .await
            .unwrap();
        let mut failed = false;
        while let Some(event) = stream.next().await {
            failed |= event.is_err();
        }
        assert!(failed);
        assert_eq!(
            store.account(ACCOUNT).unwrap().quota().access(),
            QuotaAccessState::Exhausted,
            "streaming={streaming}: observing headers is not inference success"
        );
        server.verify().await;
    }
}

#[tokio::test]
async fn quota_reset_survives_http_and_stream_error_envelopes() {
    for streaming in [false, true] {
        for reset in [json!(RESET), json!(RESET.to_string())] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, ACCOUNT).await;
            let server = MockServer::start().await;
            let error = json!({
                "type": "usage_limit_reached",
                "message": "quota exhausted",
                "resets_at": reset
            });
            let body = if streaming {
                json!({"type":"response.failed", "response":{"id":"resp_quota","error":error}})
            } else {
                json!({"error":error})
            };
            let response = if streaming {
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(format!("event: response.failed\ndata: {body}\n\n"))
            } else {
                ResponseTemplate::new(429).set_body_json(&body)
            };
            Mock::given(method("POST"))
                .and(path("/codex/responses"))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
            let provider = provider_with_base_url(&store, server.uri());
            let mut stream = provider
                .execute(
                    planned_request("openai", http_generate_operation()),
                    diagnostic_context("req_quota_reset", ACCOUNT),
                )
                .await
                .unwrap();
            while let Some(event) = stream.next().await {
                if let Err(error) = event {
                    assert_eq!(error.kind(), ProviderErrorKind::QuotaExhausted);
                }
            }
            let account = store.account(ACCOUNT).unwrap();
            assert_eq!(account.quota().access(), QuotaAccessState::Exhausted);
            assert_eq!(
                account.quota().reset_at(),
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(RESET)),
                "streaming={streaming}, reset={reset}"
            );
            server.verify().await;
        }
    }
}

#[tokio::test]
async fn current_error_quota_headers_win_without_changing_plan_or_wire_error() {
    for websocket in [false, true] {
        for envelope in ["error", "response.failed", "sse-error"] {
            if websocket && envelope == "sse-error" {
                continue;
            }
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, ACCOUNT).await;
            let before = store.account(ACCOUNT).unwrap();
            let quota_error = json!({
                "type":"usage_limit_reached", "message":"quota exhausted",
                "resets_at":RESET.to_string()
            });
            let mut body = if envelope != "response.failed" {
                json!({"type":"error", "status_code":429, "error":quota_error})
            } else {
                json!({"type":"response.failed", "response":{"id":"resp_quota","error":quota_error}})
            };
            body["headers"] = json!({
                "X-Codex-Primary-Used-Percent":"100",
                "X-Codex-Primary-Window-Minutes":"10080",
                "X-Codex-Primary-Reset-At":RESET.to_string(),
                "X-Codex-Plan-Type":"team",
                "x-codex-turn-state":"synthetic-not-a-state",
                "set-cookie":"synthetic-not-a-cookie"
            });
            let event_type = if envelope == "sse-error" {
                body.as_object_mut().unwrap().remove("type");
                "error"
            } else {
                envelope
            };
            let http = MockServer::start().await;
            let (url, server) = if websocket {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let url = format!("http://{}", listener.local_addr().unwrap());
                let error = body.clone();
                let server = tokio::spawn(async move {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut socket = accept_codex_test_websocket(socket).await;
                    socket.next().await.unwrap().unwrap();
                    socket
                        .send(Message::Text(error.to_string().into()))
                        .await
                        .unwrap();
                    socket.close(None).await.unwrap();
                });
                (url, Some(server))
            } else {
                Mock::given(method("POST"))
                    .and(path("/codex/responses"))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .insert_header("x-codex-primary-used-percent", "8")
                            .set_body_raw(
                                format!("event: {event_type}\ndata: {body}\n\n"),
                                "text/event-stream",
                            ),
                    )
                    .expect(1)
                    .mount(&http)
                    .await;
                (http.uri(), None)
            };
            let (provider, quota) = provider_and_quota_with_affinity_and_base_url_and_leases(
                &store,
                Arc::new(MemorySessionAffinity::default()),
                url,
                Arc::new(TestLeaseCoordinator::default()),
                0,
            );
            let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object(
                    "openai",
                    json!({
                        "model":"gpt-5.4", "input":"hello"
                    })
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
                    diagnostic_context("req_current_error_quota", ACCOUNT),
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
                    None => panic!("missing quota failure"),
                }
            };
            assert_eq!(error.kind(), ProviderErrorKind::QuotaExhausted);
            assert_eq!(
                serde_json::from_str::<Value>(error.raw_upstream_error().unwrap().as_str())
                    .unwrap(),
                body
            );
            let snapshot = quota.read_account(before.id()).await.unwrap().unwrap();
            assert_eq!(
                snapshot.fact().remaining_percent(),
                Some(0),
                "{envelope}, websocket={websocket}"
            );
            let after = store.account(ACCOUNT).unwrap();
            assert_eq!(after.quota().access(), QuotaAccessState::Exhausted);
            assert_eq!(
                after.quota().reset_at(),
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(RESET))
            );
            assert_eq!(after.plan_type(), before.plan_type());
            assert_eq!(after.revision(), before.revision());
            assert_eq!(
                after.turn_state_binding_revision(),
                before.turn_state_binding_revision()
            );
            if let Some(server) = server {
                timeout(Duration::from_secs(5), server)
                    .await
                    .unwrap()
                    .unwrap();
            } else {
                http.verify().await;
            }
        }
    }
}

#[tokio::test]
async fn eof_and_malformed_terminals_do_not_restore_exhausted_access() {
    for body in [
        "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_incomplete\"}}\n\n",
        "event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
        "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_failed\",\"status\":\"failed\"}}\n\n",
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, ACCOUNT).await;
        let account = store.account(ACCOUNT).unwrap();
        store
            .apply_quota_access(QuotaAccessChange {
                account_id: account.id().clone(),
                expected_revision: account.revision(),
                state: QuotaState::exhausted(
                    QuotaEvidence::UsageLimitReached,
                    SystemTime::now(),
                    None,
                ),
            })
            .await
            .unwrap();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-codex-primary-used-percent", "8")
                    .set_body_raw(body, "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let provider = provider_with_base_url(&store, server.uri());
        let mut stream = provider
            .execute(
                planned_request("openai", http_generate_operation()),
                diagnostic_context("req_incomplete_quota", ACCOUNT),
            )
            .await
            .unwrap();
        while stream.next().await.is_some() {}
        assert_eq!(
            store.account(ACCOUNT).unwrap().quota().access(),
            QuotaAccessState::Exhausted
        );
        server.verify().await;
    }
}

#[tokio::test]
async fn quota_relative_reset_uses_valid_fallback_without_reusing_retry_after() {
    for (timing, relative) in [
        (json!({"resets_in_seconds":"3600"}), true),
        (json!({"resets_at":1, "resets_in_seconds":3600}), true),
        (
            json!({"resets_at":"9223372036854775807", "resets_in_seconds":"3600"}),
            true,
        ),
        (json!({"resets_at":-1, "resets_in_seconds":-1}), false),
        (json!({"resets_in_seconds":"not-a-number"}), false),
        (json!({"retry_after":3600}), false),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, ACCOUNT).await;
        let server = MockServer::start().await;
        let mut error = timing;
        error["type"] = json!("usage_limit_reached");
        error["message"] = json!("quota exhausted");
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({"error":error})))
            .mount(&server)
            .await;
        let provider = provider_with_base_url(&store, server.uri());
        let earliest = SystemTime::now() + Duration::from_secs(3599);
        let mut stream = provider
            .execute(
                planned_request("openai", http_generate_operation()),
                diagnostic_context("req_relative_quota_reset", ACCOUNT),
            )
            .await
            .unwrap();
        while let Some(event) = stream.next().await {
            if let Err(error) = event {
                assert_eq!(error.kind(), ProviderErrorKind::QuotaExhausted);
            }
        }
        let state = store.account(ACCOUNT).unwrap().quota();
        assert_eq!(state.access(), QuotaAccessState::Exhausted);
        if relative {
            let reset = state.reset_at().expect("relative quota reset");
            assert!(reset >= earliest && reset <= SystemTime::now() + Duration::from_secs(3600));
        } else {
            assert_eq!(state.reset_at(), None);
        }
    }
}
