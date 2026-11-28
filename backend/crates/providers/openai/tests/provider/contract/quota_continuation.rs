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
                assert_eq!(
                    error.continuation_recovery_disposition(),
                    (code == "slow_down")
                        .then_some(ContinuationRecoveryDisposition::ClientReplayRequired)
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
                    assert!(
                        client_events
                            .iter()
                            .all(|event| event.is_replayable_prelude())
                    );
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
async fn quota_continuation_after_output_item_preserves_original_failure() {
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
        "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_quota\",\"model\":\"gpt-5.4\"}}\n\nevent: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"content\":[]}}\n\n".to_owned(),
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
            if event
                .unwrap()
                .wire_event()
                .is_some_and(|wire| wire.event_type() == Some("response.output_item.added"))
            {
                return;
            }
        }
        panic!("expected output item");
    })
    .await
    .expect("bounded delivery");
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

#[tokio::test]
async fn continuation_full_client_replay_selects_another_account() {
    for capacity in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let full_input = json!([
            {"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]},
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"first answer"}]},
            {"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]},
        ]);
        let expected_input = full_input.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = accept_codex_test_websocket(socket).await;
            ws.next().await.unwrap().unwrap();
            for event in [
                json!({"type":"response.created","response":{"id":"resp_previous","model":"gpt-5.4"}}),
                json!({"type":"response.completed","response":{"id":"resp_previous","model":"gpt-5.4","status":"completed","output":[]}}),
            ] {
                ws.send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
            }
            let delta: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(delta["previous_response_id"], "resp_previous");
            assert_eq!(
                delta["input"],
                json!([{"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}])
            );
            let failure = if capacity {
                json!({"type":"error","error":{"type":"service_unavailable_error","code":"server_is_overloaded","message":"Our servers are currently overloaded. Please try again later.","headers":{"retry-after":"309","retry-after-ms":"309000"}}})
            } else {
                json!({"type":"error","status":429,"error":{"type":"usage_limit_reached","code":"usage_limit_reached","message":"You have reached your usage limit."}})
            };
            ws.send(Message::Text(failure.to_string().into()))
                .await
                .unwrap();
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = accept_codex_test_websocket(socket).await;
            let replay: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert!(replay.get("previous_response_id").is_none());
            assert_eq!(replay["input"], expected_input);
            for event in [
                json!({"type":"response.created","response":{"id":"resp_replayed","model":"gpt-5.4"}}),
                json!({"type":"response.completed","response":{"id":"resp_replayed","model":"gpt-5.4","status":"completed","output":[]}}),
            ] {
                ws.send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
            }
        });
        let provider = provider_with_base_url(&store, base_url);
        let first = Operation::Generate(generate_with_persisted_session_context(
            "acct_provider_contract",
            "conversation-quota",
            "quota-replay",
            "turn",
        ));
        let mut stream = provider
            .execute(
                planned_request("openai", first),
                context("req_quota_first", CancellationToken::new()),
            )
            .await
            .unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_provider_contract"
        );
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        drop(stream);
        let mut stream = provider
            .execute(
                planned_request("openai", quota_continuation_operation(true)),
                context("req_quota_delta", CancellationToken::new())
                    .with_continuation_attempt(ContinuationAttempt::Native),
            )
            .await
            .unwrap();
        let mut error = loop {
            match stream.next().await {
                Some(Ok(event)) => assert!(!event.has_client_event()),
                Some(Err(error)) => break error,
                None => panic!("expected quota rejection"),
            }
        };
        drop(stream);
        assert_eq!(
            error.continuation_recovery_disposition(),
            Some(ContinuationRecoveryDisposition::ClientReplayRequired)
        );
        create_account(&store, "acct_affinity_switch_b").await;
        if capacity {
            assert_eq!(error.kind(), ProviderErrorKind::UpstreamCapacityUnavailable);
            assert_eq!(error.retry_after(), Some(Duration::from_secs(309)));
            assert!(!error.take_atomic_client_events().is_empty());
            let owner = ProviderAccountId::new("acct_provider_contract").unwrap();
            let kind = ProviderKind::new("openai").unwrap();
            let pin = NativeContinuationPin::new(
                PreviousResponseId::new("resp_previous"),
                PreviousResponseId::new("resp_previous"),
                ClientApiKeyId::new("key_openai_contract").unwrap(),
                kind.clone(),
                owner.clone(),
            )
            .with_scope(
                gateway_core::engine::continuation::NativeContinuationScope::ConnectionLocal,
            );
            let rotate = AttemptContext::new(
                RequestAttemptContext::new(
                    ModelRequestId::new("req_capacity_rotate").unwrap(),
                    ClientApiKeyId::new("key_openai_contract").unwrap(),
                ),
                NonZeroU32::new(2).unwrap(),
                SystemTime::now() + Duration::from_secs(5),
                account_policy(),
                AccountAttemptContext::new(
                    BTreeSet::from([owner.clone()]),
                    None,
                    Some(ProviderAccountStateOwner::new(kind, owner)),
                )
                .with_account_scope(contract_account_scope()),
                Some(ContinuationBinding::Pinned(pin)),
                CancellationToken::new(),
            )
            .with_continuation_attempt(ContinuationAttempt::RotateForClientReplay(
                ClientReplayTrigger::UpstreamCapacity,
            ));
            error = provider
                .execute(
                    planned_request("openai", quota_continuation_operation(true)),
                    rotate,
                )
                .await
                .err()
                .expect("rotation requests full client replay without sending delta");
            assert_eq!(
                error.continuation_unavailable_reason(),
                Some("account_rotated")
            );
            assert_ne!(
                store
                    .account("acct_provider_contract")
                    .unwrap()
                    .quota()
                    .access(),
                QuotaAccessState::Exhausted
            );
        } else {
            assert_eq!(
                store
                    .account("acct_provider_contract")
                    .unwrap()
                    .quota()
                    .access(),
                QuotaAccessState::Exhausted
            );
        }
        assert_eq!(
            error.client_visible_upstream_error().unwrap().code(),
            Some("previous_response_not_found")
        );
        assert!(error.take_atomic_client_events().is_empty());
        assert!(
            error
                .client_visible_upstream_response()
                .unwrap()
                .headers()
                .iter()
                .all(|header| !header.name().eq_ignore_ascii_case("retry-after")
                    && !header.name().eq_ignore_ascii_case("retry-after-ms"))
        );
        // 客户端重建完整历史，保留同一会话标识；选号必须使用替代账号
        let replay = Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", json!({"model":"gpt-5.4","session_id":"quota-replay","thread_id":"quota-replay","input":full_input}).as_object().unwrap().clone()).unwrap(),
    ));
        let mut stream = provider
            .execute(
                planned_request("openai", replay),
                context("req_quota_full", CancellationToken::new()),
            )
            .await
            .unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_affinity_switch_b"
        );
        let mut completed = false;
        while let Some(event) = stream.next().await {
            completed |= event
                .unwrap()
                .canonical_facts()
                .iter()
                .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
        }
        assert!(completed);
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn native_queue_recovery_checks_an_immediate_spare_in_each_affinity_mode() {
    use gateway_core::account::AccountAffinity::{Preferred, Relaxed, Strict};
    use std::sync::atomic::Ordering;
    let owner = ProviderAccountId::new("acct_scope_old").unwrap();
    let spare = ProviderAccountId::new("acct_scope_new").unwrap();
    let key = ClientApiKeyId::new("key_openai_contract").unwrap();
    let pin = NativeContinuationPin::new(
        PreviousResponseId::new("resp_previous"),
        PreviousResponseId::new("resp_previous"),
        key.clone(),
        ProviderKind::new("openai").unwrap(),
        owner.clone(),
    );
    let server = MockServer::start().await;
    for (mode, switch, child, previous_route) in [
        (Strict, false, false, "codex"),
        (Strict, true, false, "codex"),
        (Preferred, false, false, "codex"),
        (Preferred, true, false, "codex"),
        (Relaxed, false, false, "codex"),
        (Relaxed, true, false, "codex"),
        (Strict, true, true, "codex"),
        (Strict, true, false, "excel"),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, owner.as_str()).await;
        create_account(&store, spare.as_str()).await;
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        leases.capacity.set_load(owner.as_str(), 1);
        let provider = super::scheduling::waiting_provider_with_affinity_and_limit(
            &store,
            leases.clone(),
            server.uri(),
            Arc::default(),
            Arc::default(),
            NonZeroU32::MIN,
        );
        let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object("openai", json!({
                "model":"gpt-5.4", "input":"continue", "previous_response_id":"resp_previous",
                "session_id":"queue-root", "thread_id": if child { "queue-child" } else { "queue-root" },
            }).as_object().unwrap().clone()).unwrap(),
        ).with_provider_session_state(ProviderSessionState::new("openai", json!({
            "account_id":owner.as_str(), "continuation_scope":"persisted", "responses_upstream":previous_route,
        }).as_object().unwrap().clone()).unwrap()));
        let select = |attempt, excluded, timeout| {
            let ctx = AttemptContext::new(
                RequestAttemptContext::new(
                    ModelRequestId::new("req_queue_recovery").unwrap(),
                    key.clone(),
                ),
                NonZeroU32::MIN,
                SystemTime::now() + Duration::from_secs(10),
                account_policy()
                    .with_account_affinity(mode)
                    .with_switch_account_on_queue_failure(switch),
                AccountAttemptContext::new(excluded, None, None)
                    .with_account_scope(contract_account_scope()),
                Some(ContinuationBinding::Pinned(pin.clone())),
                CancellationToken::new(),
            )
            .with_request_tuning(gateway_core::routing::RequestTuning {
                account_busy_wait_enabled: true,
                account_concurrency_wait_timeout_seconds: timeout,
                ..Default::default()
            })
            .with_continuation_attempt(attempt);
            provider.execute(planned_request("openai", operation.clone()), ctx)
        };
        let native = select(ContinuationAttempt::Native, BTreeSet::new(), 1)
            .await
            .err()
            .unwrap();
        if previous_route == "excel" {
            assert_eq!(native.send_state(), UpstreamSendState::NotSent);
            assert_eq!(
                native
                    .client_visible_upstream_error()
                    .and_then(|e| e.code()),
                Some("previous_response_not_found")
            );
            assert!(leases.capacity.waits.lock().unwrap().is_empty());
            continue;
        }
        assert_eq!(
            native.kind(),
            ProviderErrorKind::ConcurrencyQueueTimeout,
            "{mode:?} {child} {previous_route}"
        );
        assert!(native.client_visible_upstream_error().is_none());
        let eligible = !child && previous_route == "codex";
        assert_eq!(
            native.continuation_recovery_disposition(),
            Some(if eligible {
                ContinuationRecoveryDisposition::ClientReplayRequired
            } else {
                ContinuationRecoveryDisposition::RetryExactConnection
            })
        );
        if !eligible {
            continue;
        }
        let waits = leases.capacity.waits.lock().unwrap().len();
        assert!(waits > 0);
        for busy in [true, false] {
            leases.capacity.set_load(spare.as_str(), u32::from(busy));
            let replacement = tokio::time::timeout(
                Duration::from_secs(1),
                select(
                    ContinuationAttempt::RotateForClientReplay(
                        ClientReplayTrigger::AccountQueueTimeout,
                    ),
                    BTreeSet::from([owner.clone()]),
                    5,
                ),
            )
            .await
            .expect("replacement must not queue")
            .err()
            .unwrap();
            assert_eq!(replacement.send_state(), UpstreamSendState::NotSent);
            assert_eq!(
                replacement
                    .client_visible_upstream_error()
                    .and_then(|e| e.code()),
                (!busy).then_some("previous_response_not_found")
            );
            assert_eq!(leases.capacity.waits.lock().unwrap().len(), waits);
            assert_eq!(
                leases.capacity.signals.lock().unwrap()[&spare].in_flight,
                u32::from(busy)
            );
        }
    }
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| !r.url.path().ends_with("/responses"))
    );
}
