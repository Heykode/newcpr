use super::*;

fn quota_continuation_operation(use_websocket: bool) -> Operation {
    Operation::Generate(
        GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object(
                "openai",
                json!({
                    "model": "gpt-5.4",
                    "input": "continue",
                    "previous_response_id": "resp_previous",
                    "session_id": "quota-replay",
                    "thread_id": "turn",
                })
                .as_object()
                .unwrap()
                .clone(),
            )
            .unwrap()
            .with_context(Map::from_iter([(
                "use_websocket".to_owned(),
                json!(use_websocket),
            )])),
        )
        .with_provider_session_state(
            generate_with_persisted_session_context(
                "acct_provider_contract",
                "conversation-quota",
                "quota-replay",
                "turn",
            )
            .provider_session_state("openai")
            .unwrap()
            .clone(),
        ),
    )
}

#[tokio::test]
async fn quota_continuation_opening_rejection_projects_replay_without_losing_upstream_facts() {
    for use_websocket in [false, true] {
        for (status, code, projects_replay) in [
            (429, "usage_limit_reached", true),
            (402, "insufficient_quota", true),
            (429, "quota_exceeded", false),
            (429, "rate_limit_exceeded", false),
            (429, "slow_down", false),
            (429, "unknown_error", false),
        ] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            let server = MockServer::start().await;
            let original =
                json!({"error": {"type": code, "code": code, "message": "upstream rejection"}});
            Mock::given(method(if use_websocket { "GET" } else { "POST" }))
                .and(path("/codex/responses"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .insert_header("retry-after", "129600")
                        .insert_header("x-request-id", "req-upstream-quota")
                        .set_body_json(&original),
                )
                .expect(1)
                .mount(&server)
                .await;
            let mut stream = provider_with_base_url_and_retry_budget(&store, server.uri(), 0)
                .execute(
                    planned_request("openai", quota_continuation_operation(use_websocket)),
                    context("req_quota_replay", CancellationToken::new())
                        .with_continuation_attempt(ContinuationAttempt::Native),
                )
                .await
                .expect("prepare continuation");
            let mut error = loop {
                match stream.next().await {
                    Some(Ok(event)) => assert!(!event.has_client_event()),
                    Some(Err(error)) => break error,
                    None => panic!("expected rejection"),
                }
            };
            assert_eq!(error.upstream_status(), Some(status));
            assert_eq!(error.retry_after(), Some(Duration::from_secs(129_600)));
            assert_eq!(
                serde_json::from_str::<Value>(error.raw_upstream_error().unwrap().as_str())
                    .unwrap(),
                original
            );
            assert!(error.take_atomic_client_events().is_empty());
            let response = error.client_visible_upstream_response().unwrap();
            let client: Value = serde_json::from_slice(response.body()).unwrap();
            if projects_replay {
                assert_eq!(error.upstream_code().map(|code| code.as_str()), Some(code));
                assert_eq!(error.kind(), ProviderErrorKind::QuotaExhausted);
                assert_eq!(
                    error.continuation_recovery_disposition(),
                    Some(ContinuationRecoveryDisposition::ClientReplayRequired)
                );
                assert_eq!(response.status(), 400);
                assert_eq!(client["error"]["code"], "previous_response_not_found");
                assert!(
                    !response
                        .headers()
                        .iter()
                        .any(|header| header.name() == "retry-after")
                );
                assert!(response.headers().iter().any(|header| {
                    header.name() == "x-request-id"
                        && header.value().as_ref() == b"req-upstream-quota"
                }));
                assert_eq!(
                    store
                        .account("acct_provider_contract")
                        .unwrap()
                        .quota()
                        .access(),
                    QuotaAccessState::Exhausted
                );
            } else {
                assert_ne!(
                    error.continuation_recovery_disposition(),
                    Some(ContinuationRecoveryDisposition::ClientReplayRequired)
                );
                assert_eq!(response.status(), 429);
                assert_eq!(client, original);
            }
        }
    }
}

#[tokio::test]
async fn quota_continuation_stream_rejection_only_projects_before_delivery() {
    for use_websocket in [false, true] {
        for semantic_output in [false, true] {
            for native_continuation in [false, true] {
                let store = Arc::new(MemoryAccountStore::default());
                create_account(&store, "acct_provider_contract").await;
                let mut events = vec![json!({
                    "type":"response.created",
                    "response":{"id":"resp_quota","model":"gpt-5.4"}
                })];
                if semantic_output {
                    events.push(json!({
                        "type":"response.output_text.delta",
                        "output_index":0,
                        "content_index":0,
                        "delta":"hello"
                    }));
                }
                let original = json!({
                    "type":"response.failed",
                    "response":{
                        "id":"resp_quota",
                        "error":{
                            "code":"usage_limit_reached",
                            "message":"You have reached your usage limit."
                        }
                    }
                });
                events.push(original.clone());
                let (base_url, _http_server, websocket_server) = if use_websocket {
                    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let base_url = format!("http://{}", listener.local_addr().unwrap());
                    let server = tokio::spawn(async move {
                        let (socket, _) = listener.accept().await.unwrap();
                        let mut ws = accept_codex_test_websocket(socket).await;
                        ws.next().await.unwrap().unwrap();
                        for event in events {
                            ws.send(Message::Text(event.to_string().into()))
                                .await
                                .unwrap();
                        }
                    });
                    (base_url, None, Some(server))
                } else {
                    let server = MockServer::start().await;
                    let frames = events
                        .iter()
                        .map(|event| {
                            format!(
                                "event: {}\ndata: {event}\n\n",
                                event["type"].as_str().unwrap()
                            )
                        })
                        .collect::<String>();
                    Mock::given(method("POST"))
                        .and(path("/codex/responses"))
                        .respond_with(
                            ResponseTemplate::new(200)
                                .insert_header("x-request-id", "req-quota-stream")
                                .set_body_raw(frames, "text/event-stream"),
                        )
                        .expect(1)
                        .mount(&server)
                        .await;
                    (server.uri(), Some(server), None)
                };
                let operation = if native_continuation {
                    quota_continuation_operation(use_websocket)
                } else if use_websocket {
                    generate_operation()
                } else {
                    http_generate_operation()
                };
                let attempt = context("req_quota_stream", CancellationToken::new())
                    .with_continuation_attempt(if native_continuation {
                        ContinuationAttempt::Native
                    } else {
                        ContinuationAttempt::None
                    });
                let mut stream = provider_with_base_url(&store, base_url)
                    .execute(planned_request("openai", operation), attempt)
                    .await
                    .unwrap();
                let mut client_events = Vec::new();
                let mut error = loop {
                    match stream.next().await {
                        Some(Ok(event)) => {
                            if event.has_client_event() {
                                client_events.push(event);
                            }
                        }
                        Some(Err(error)) => break error,
                        None => panic!("expected quota failure"),
                    }
                };
                assert_eq!(error.kind(), ProviderErrorKind::QuotaExhausted);
                assert_eq!(error.send_state(), UpstreamSendState::Sent);
                assert_eq!(error.replay_is_safe(), !semantic_output);
                assert_eq!(
                    serde_json::from_str::<Value>(error.raw_upstream_error().unwrap().as_str())
                        .unwrap(),
                    original
                );
                client_events.extend(error.take_atomic_client_events());
                if native_continuation && !semantic_output {
                    assert!(client_events.is_empty());
                    assert_eq!(
                        error.continuation_recovery_disposition(),
                        Some(ContinuationRecoveryDisposition::ClientReplayRequired)
                    );
                    assert_eq!(
                        error.client_visible_upstream_error().unwrap().code(),
                        Some("previous_response_not_found")
                    );
                } else {
                    assert_ne!(
                        error.continuation_recovery_disposition(),
                        Some(ContinuationRecoveryDisposition::ClientReplayRequired)
                    );
                    assert_eq!(
                        client_events.last().unwrap().wire_event().unwrap().data(),
                        &original
                    );
                }
                if let Some(server) = websocket_server {
                    server.await.unwrap();
                }
            }
        }
    }
}

#[tokio::test]
async fn quota_continuation_after_structural_commit_preserves_original_failure() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let original = json!({
        "type":"response.failed",
        "response":{
            "id":"resp_quota",
            "error":{"code":"usage_limit_reached","message":"limit reached"}
        }
    });
    let (base_url, release, _, server) = paused_chunked_sse_server(
        "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_quota\",\"model\":\"gpt-5.4\"}}\n\n".to_owned(),
        format!("event: response.failed\ndata: {original}\n\n"),
    )
    .await;
    let mut stream = provider_with_base_url(&store, base_url)
        .execute(
            planned_request("openai", quota_continuation_operation(false)),
            context("req_quota_commit", CancellationToken::new())
                .with_continuation_attempt(ContinuationAttempt::Native),
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(3), async {
        while let Some(event) = stream.next().await {
            if event.unwrap().has_client_event() {
                return;
            }
        }
        panic!("expected grace commit");
    })
    .await
    .expect("bounded grace");
    release.send(()).unwrap();
    let mut delivered_failure = false;
    let error = loop {
        match stream.next().await {
            Some(Ok(event)) => {
                delivered_failure |= event
                    .wire_event()
                    .is_some_and(|wire| wire.data() == &original);
            }
            Some(Err(error)) => break error,
            None => panic!("expected failure"),
        }
    };
    assert!(delivered_failure);
    assert!(!error.replay_is_safe());
    assert_ne!(
        error.continuation_recovery_disposition(),
        Some(ContinuationRecoveryDisposition::ClientReplayRequired)
    );
    server.await.unwrap();
}
