//! 验证推理失败与真实使用在额度恢复和预热中的不同作用

use super::*;

#[tokio::test]
async fn stream_quota_facts_skip_warmup_only_after_usage() {
    for ending in ["failed", "error", "eof", "malformed", "completed"] {
        for output_seen in [false, true] {
            let store = Arc::new(MemoryAccountStore::default());
            let account_id = "acct_provider_contract";
            create_account(&store, account_id).await;
            store.set_quota_settings(
                account_id,
                serde_json::from_value(json!({
                    "codex:18000s": {"warmup": {"model": "gpt-5.4"}}
                }))
                .unwrap(),
            );
            let now = Utc::now();
            let reset = now.timestamp() - 600;
            let observed_at = SystemTime::from(now - chrono::Duration::seconds(660));
            let account = store.account(account_id).unwrap();
            let usage = |reset| {
                json!({"rate_limit": {"allowed": true, "primary_window": {
                    "used_percent": 0, "limit_window_seconds": 18000, "reset_at": reset
                }}})
            };
            store
                .compare_and_swap_quota(QuotaObservation {
                    account_id: account.id().clone(),
                    expected_revision: account.revision(),
                    quota: OpaqueProviderData::new(usage(reset).as_object().unwrap().clone()),
                    observed_at,
                    state: QuotaState::allowed(observed_at),
                    quota_pause: None,
                    plan_type: None,
                })
                .await
                .unwrap();
            let server = MockServer::start().await;
            let (provider, quota) = provider_and_quota_with_affinity_and_base_url_and_leases(
                &store,
                Arc::new(MemorySessionAffinity::default()),
                server.uri(),
                Arc::new(TestLeaseCoordinator::default()),
                0,
            );
            quota
                .execute_bucket_warmup_at(now - chrono::Duration::seconds(601))
                .await
                .unwrap();
            let rate_limit = json!({"type": "codex.rate_limits", "rate_limits": {
                "allowed": true, "primary": {"used_percent": 0, "window_minutes": 300,
                "reset_at": reset + 18000}
            }});
            let mut body = sse_event(json!({"type": "response.created", "response": {
                "id": "resp_quota_usage", "model": "gpt-5.4"
            }}));
            body.push_str(&sse_event(rate_limit));
            if output_seen {
                body.push_str(&sse_event(json!({"type": "response.output_text.delta",
                    "item_id": "msg_1", "output_index": 0, "content_index": 0, "delta": "hello"
                })));
            }
            body.push_str(&match ending {
                "failed" => sse_event(json!({"type": "response.failed", "status_code": 400,
                    "response": {"id": "resp_quota_usage", "status": "failed", "error": {
                        "code": "invalid_request_error", "message": "fixture rejection"
                    }}
                })),
                "error" => sse_event(json!({"type": "error", "status": 400, "error": {
                    "type": "invalid_request_error", "code": "invalid_request_error",
                    "message": "fixture rejection"
                }})),
                "malformed" => "event: response.completed\ndata: {invalid json}\n\n".to_owned(),
                "completed" => completed_response(),
                _ => String::new(),
            });
            Mock::given(method("POST"))
                .and(path("/codex/responses"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .insert_header("x-codex-primary-used-percent", "0")
                        .insert_header("x-codex-primary-window-minutes", "300")
                        .insert_header("x-codex-primary-reset-at", (reset + 18000).to_string())
                        .set_body_string(body),
                )
                .up_to_n_times(1)
                .with_priority(1)
                .expect(1)
                .mount(&server)
                .await;
            let mut stream = provider
                .execute(
                    planned_request("openai", http_generate_operation()),
                    context("req_quota_usage", CancellationToken::new()),
                )
                .await
                .unwrap();
            while let Some(event) = stream.next().await {
                if let Err(error) = event {
                    assert_ne!(ending, "completed", "successful fixture failed: {error}");
                    break;
                }
            }
            drop(stream);
            let saved = store.quota_json(account_id).unwrap();
            assert_eq!(
                saved.pointer("/rate_limits_by_limit_id/codex/primary_window/used_percent"),
                Some(&json!(0.0))
            );
            let used_quota = output_seen || ending == "completed";
            assert_eq!(
                saved.pointer("/_cpr_bucket_last_used_at/codex").is_some(),
                used_quota,
                "{ending}"
            );

            let expected_warmups = u64::from(!used_quota);
            Mock::given(method("GET"))
                .and(path("/api/codex/usage"))
                .respond_with(ResponseTemplate::new(200).set_body_json(usage(reset + 18000)))
                .expect(expected_warmups)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/codex/responses"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(completed_response()),
                )
                .expect(expected_warmups)
                .mount(&server)
                .await;
            assert_eq!(
                quota
                    .execute_bucket_warmup_at(Utc::now())
                    .await
                    .unwrap()
                    .warmed_up,
                expected_warmups,
                "{ending}, output={output_seen}"
            );
            server.verify().await;
        }
    }
}

fn sse_event(value: serde_json::Value) -> String {
    format!("data: {value}\n\n")
}

fn completed_response() -> String {
    sse_event(json!({"type": "response.completed", "response": {
        "id": "resp_quota_usage", "model": "gpt-5.4", "status": "completed", "output": [],
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
    }}))
}

#[tokio::test]
async fn disconnected_stream_preserves_quota_facts_without_marking_usage() {
    let store = Arc::new(MemoryAccountStore::default());
    let account_id = "acct_provider_contract";
    create_account(&store, account_id).await;
    let body = sse_event(json!({"type": "codex.rate_limits", "rate_limits": {
        "primary": {"used_percent": 0, "window_minutes": 300,
            "reset_at": Utc::now().timestamp() + 18000}
    }})) + &sse_event(json!({"type": "response.created", "response": {
        "id": "resp_disconnect", "model": "gpt-5.4"
    }}));
    let (base_url, _release, _first_chunk_sent, server) =
        paused_chunked_sse_server(body, String::new()).await;
    let (provider, _) = provider_and_quota_with_affinity_and_base_url_and_leases(
        &store,
        Arc::new(MemorySessionAffinity::default()),
        base_url,
        Arc::new(TestLeaseCoordinator::default()),
        0,
    );
    let mut stream = provider
        .execute(
            planned_request("openai", http_generate_operation()),
            context("req_quota_disconnect", CancellationToken::new()),
        )
        .await
        .unwrap();
    stream.next().await.unwrap().unwrap();
    server.abort();
    let mut failed = false;
    while let Some(event) = stream.next().await {
        if event.is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed, "the fixture must fail while reading the stream");
    let saved = store.quota_json(account_id).unwrap();
    assert_eq!(
        saved.pointer("/rate_limits_by_limit_id/codex/primary_window/used_percent"),
        Some(&json!(0.0))
    );
    assert!(saved.get("_cpr_bucket_last_used_at").is_none());
}

#[tokio::test]
async fn duration_only_usage_limit_tracks_the_window_named_by_the_denial() {
    for (minutes, other_minutes, other_used, active_limit, recovers) in [
        (300, 10080, 45, "codex", true),
        (10080, 300, 45, "", true),
        (43200, 10080, 45, "", true),
        (10080, 300, 100, "", false),
        (10080, 10080, 45, "", false),
        (10080, 300, 45, "spark", false),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        let id = "acct_provider_contract";
        create_account(&store, id).await;
        let account = store.account(id).unwrap();
        let now = Utc::now().timestamp();
        let usage = |used, reset| {
            json!({"rate_limit": {
                "allowed": true,
                "primary_window": {"used_percent": other_used, "limit_window_seconds": i64::from(other_minutes) * 60, "reset_at": now + 7200},
                "secondary_window": {"used_percent": used, "limit_window_seconds": i64::from(minutes) * 60, "reset_at": reset}
            }})
        };
        let observed_at = SystemTime::now() - Duration::from_secs(120);
        store
            .compare_and_swap_quota(QuotaObservation {
                account_id: account.id().clone(),
                expected_revision: account.revision(),
                quota: OpaqueProviderData::new(usage(99, now - 60).as_object().unwrap().clone()),
                observed_at,
                state: QuotaState::allowed(observed_at),
                quota_pause: None,
                plan_type: None,
            })
            .await
            .unwrap();

        let server = MockServer::start().await;
        let (provider, quota) = provider_and_quota_with_affinity_and_base_url_and_leases(
            &store,
            Arc::new(MemorySessionAffinity::default()),
            server.uri(),
            Arc::new(TestLeaseCoordinator::default()),
            0,
        );
        let error = json!({"type": "usage_limit_reached", "code": "usage_limit_reached",
            "limit_window_minutes": minutes});
        let mut refusal = if minutes == 43200 {
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_event(json!({"type": "response.failed",
                    "response": {"id": "resp_limit", "status": "failed", "error": error}})))
        } else {
            ResponseTemplate::new(429).set_body_json(json!({"error": error}))
        };
        if !active_limit.is_empty() {
            refusal = refusal.insert_header("x-codex-active-limit", active_limit);
        }
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(refusal)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/codex/usage"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(usage(0, now + i64::from(minutes) * 60)),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut stream = provider
            .execute(
                planned_request("openai", http_generate_operation()),
                context("req_usage_window", CancellationToken::new()),
            )
            .await
            .unwrap();
        let failure = loop {
            if let Err(error) = stream.next().await.expect("quota refusal") {
                break error;
            }
        };
        assert_eq!(failure.kind(), ProviderErrorKind::QuotaExhausted);
        let denied = store.account(id).unwrap().quota();
        assert_eq!(denied.evidence(), Some(QuotaEvidence::UsageLimitReached));
        assert_eq!(denied.reset_at(), None);
        assert_eq!(
            denied.limit_window_minutes(),
            (active_limit != "spark").then_some(minutes)
        );

        let summary = quota.synchronize().await.unwrap();
        assert_eq!(
            summary.updated,
            u64::from(recovers),
            "{minutes}/{other_minutes} {other_used} {active_limit}"
        );
        assert_eq!(store.account(id).unwrap().quota().is_exhausted(), !recovers);
    }
}

#[tokio::test]
async fn spend_limit_http_and_sse_freeze_quota_and_prohibit_retries() {
    for code in [
        "api_key_spend_limit_exceeded",
        "project_spend_limit_exceeded",
        "organization_spend_limit_exceeded",
    ] {
        for sse in [false, true] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            let server = MockServer::start().await;
            let error = json!({"code":code,"type":"insufficient_quota","message":"budget reached"});
            let response = if sse {
                ResponseTemplate::new(200).set_body_raw(sse_event(json!({"type":"response.failed","response":{"id":"resp_spend","status":"failed","error":error}})), "text/event-stream")
            } else {
                ResponseTemplate::new(429).set_body_json(json!({"error":error}))
            };
            Mock::given(method("POST"))
                .and(path("/codex/responses"))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
            let provider = provider_with_base_url(&store, server.uri());
            let result = provider
                .execute(
                    planned_request(
                        "openai",
                        super::image_account_eligibility::borrow_text(
                            "spend-session",
                            "spend-turn",
                        ),
                    ),
                    context("req_spend_limit", CancellationToken::new()),
                )
                .await;
            let error = match result {
                Err(error) => error,
                Ok(mut stream) => loop {
                    match stream.next().await {
                        Some(Err(error)) => break error,
                        Some(Ok(_)) => {}
                        None => panic!("spend rejection must fail"),
                    }
                },
            };
            assert_eq!(error.kind(), ProviderErrorKind::QuotaExhausted);
            assert!(error.retry_is_prohibited());
            assert_eq!(error.pre_delivery_retry(), None);
            assert_eq!(
                error.client_visible_upstream_error().unwrap().code(),
                Some(code)
            );
            assert_eq!(error.upstream_status(), if sse { None } else { Some(429) });
            assert_eq!(
                store
                    .account("acct_provider_contract")
                    .unwrap()
                    .quota()
                    .evidence(),
                Some(QuotaEvidence::ProviderDenied)
            );
            assert_eq!(
                store
                    .account("acct_provider_contract")
                    .unwrap()
                    .credential_state(),
                CredentialState::Ready
            );
        }
    }
}

#[tokio::test]
async fn spend_limit_websocket_opening_stream_and_continuation_prohibit_replay() {
    for (opening, continuation) in [(false, false), (true, false), (false, true)] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let error = json!({"code":"project_spend_limit_exceeded","type":"insufficient_quota","message":"budget reached"});
            if opening {
                crate::transport::read_http_request_with_body(&mut socket).await;
                let body = json!({"error":error}).to_string();
                socket.write_all(format!("HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nretry-after: 600\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
            } else {
                let mut socket = accept_codex_test_websocket(socket).await;
                socket.next().await.unwrap().unwrap();
                if continuation {
                    generate_compat::send_completion(&mut socket, "resp_spend_seed").await;
                    let request = socket.next().await.unwrap().unwrap();
                    let body: Value = serde_json::from_str(request.to_text().unwrap()).unwrap();
                    assert_eq!(body["previous_response_id"], "resp_spend_seed");
                }
                socket
                    .send(Message::Text(
                        json!({"type":"error","status":429,"error":error})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
            }
            assert!(
                timeout(Duration::from_millis(120), listener.accept())
                    .await
                    .is_err(),
                "no retry or HTTP fallback"
            );
        });
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let provider = provider_with_base_url(&store, base);
        let mut request = websocket_fixture_request(generate_with_persisted_session_context(
            "acct_provider_contract",
            "spend-conversation",
            "spend-root",
            "spend-turn",
        ));
        let mut attempt = context("req_spend_ws", CancellationToken::new());
        if continuation {
            let mut seed = provider
                .execute(
                    planned_request("openai", Operation::Generate(request.clone())),
                    context("req_spend_seed", CancellationToken::new()),
                )
                .await
                .unwrap();
            let mut state = None;
            while let Some(event) = seed.next().await {
                if let Some(update) = event.unwrap().session_update() {
                    state = Some(update.clone());
                }
            }
            drop(seed);
            let state = state.expect("seed continuation owner");
            assert_eq!(state.payload()["continuation_scope"], "connection_local");
            let mut body = request.protocol_payload().body().clone();
            body.insert("previous_response_id".to_owned(), json!("resp_spend_seed"));
            request = GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object("openai", body).unwrap(),
            )
            .with_provider_session_state(state);
            attempt = pinned_continuation_context(
                "req_spend_exact",
                "acct_provider_contract",
                "resp_spend_seed",
                "resp_spend_seed",
                1,
                ContinuationAttempt::Native,
            );
        }
        let result = provider
            .execute(
                planned_request("openai", Operation::Generate(request)),
                attempt,
            )
            .await;
        let error = match result {
            Err(error) => error,
            Ok(mut stream) => loop {
                match stream.next().await {
                    Some(Err(error)) => break error,
                    Some(Ok(_)) => {}
                    None => panic!("spend rejection must fail"),
                }
            },
        };
        assert_eq!(error.kind(), ProviderErrorKind::QuotaExhausted);
        assert_eq!(error.upstream_status(), Some(429));
        assert!(error.retry_is_prohibited());
        assert!(!error.replay_is_safe());
        assert_eq!(error.pre_delivery_retry(), None);
        assert_eq!(error.continuation_recovery_disposition(), None);
        assert_eq!(
            error.client_visible_upstream_error().unwrap().code(),
            Some("project_spend_limit_exceeded")
        );
        assert_eq!(
            store
                .account("acct_provider_contract")
                .unwrap()
                .quota()
                .evidence(),
            Some(QuotaEvidence::ProviderDenied)
        );
        server.await.unwrap();
    }
}
