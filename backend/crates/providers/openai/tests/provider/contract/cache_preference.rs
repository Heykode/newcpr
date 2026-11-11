//! 无显式会话时，选号提示的优先级、隔离边界与完整历史要求

use super::*;

#[tokio::test]
async fn cache_preference_keys_follow_client_hints_or_the_opening_messages() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_affinity").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let provider = provider_with_affinity(&store, affinity.clone());

    let select = async |client: &str, model: &str, body: serde_json::Value| {
        let before = affinity.lookup_keys().len();
        let attempt = AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_cache_key").unwrap(),
                ClientApiKeyId::new(client).unwrap(),
            ),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(5),
            account_policy(),
            AccountAttemptContext::new(BTreeSet::new(), None, None)
                .with_account_scope(contract_account_scope()),
            None,
            CancellationToken::new(),
        );
        let payload =
            ProtocolPayload::json_object("openai", body.as_object().unwrap().clone()).unwrap();
        drop(
            provider
                .execute(
                    planned_request_with_model(
                        "openai",
                        Operation::Generate(GenerateRequest::from_protocol_payload(payload)),
                        model,
                    ),
                    attempt,
                )
                .await
                .unwrap(),
        );
        affinity.lookup_keys().get(before).cloned()
    };

    let model = "gpt-5.4";
    let opening = json!({"instructions":"be brief", "input":[{"role":"developer","content":"review"}, {"role":"user","content":"hello"}]});
    let key = select("client_a", model, opening.clone()).await.unwrap();
    let mut next = opening.clone();
    next["tools"] = json!([{"type":"function","name":"new_tool"}]);
    next["input"][1]["content"] =
        json!([{"type":"input_text","text":"hel"},{"type":"input_text","text":"lo"}]);
    next["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role":"user","content":"next turn"}));
    assert_eq!(select("client_a", model, next).await.as_ref(), Some(&key));
    assert_ne!(
        select("client_b", model, opening.clone()).await.unwrap(),
        key
    );
    assert_ne!(
        select("client_a", "gpt-5.5", opening.clone())
            .await
            .unwrap(),
        key
    );
    for path in ["/instructions", "/input/0/content", "/input/1/content"] {
        let mut changed = opening.clone();
        *changed.pointer_mut(path).unwrap() = json!("different");
        assert_ne!(select("client_a", model, changed).await.unwrap(), key);
    }

    let chat_opening = json!({"input":[{"role":"system","content":"be brief"},{"role":"developer","content":"review"},{"role":"user","content":"hello"}]});
    assert_eq!(
        select("client_a", model, chat_opening).await.as_ref(),
        Some(&key)
    );

    let hinted = json!({"prompt_cache_key":"client-hint","input":"hello"});
    let cache = select("client_a", model, hinted.clone()).await.unwrap();
    assert_ne!(key, cache);
    assert_ne!(select("client_b", model, hinted).await.unwrap(), cache);
    assert_eq!(
        select(
            "client_a",
            model,
            json!({"prompt_cache_key":"client-hint", "input":"different"})
        )
        .await,
        Some(cache)
    );
    for input in [
        json!(" "),
        json!([{"role":"assistant","content":"partial history"},{"role":"user","content":"next"}]),
        json!([{"role":"user","content":[{"type":"input_image","image_url":"https://invalid.test/x"}]}]),
    ] {
        assert_eq!(
            select(
                "client_a",
                model,
                json!({"instructions":"shared", "input":input})
            )
            .await,
            None
        );
    }
}

#[tokio::test]
async fn cache_preference_storage_timeouts_follow_the_affinity_mode() {
    use gateway_core::account::AccountAffinity;

    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    for waiting in [false, true] {
        for mode in [AccountAffinity::Preferred, AccountAffinity::Strict] {
            for affinity in [
                MemorySessionAffinity::with_operation_delay(Duration::from_secs(1)),
                MemorySessionAffinity::with_claim_barrier(2),
            ] {
                let affinity = Arc::new(affinity);
                let server = MockServer::start().await;
                let leases = Arc::new(TestLeaseCoordinator::default());
                leases
                    .capacity
                    .enabled
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                let provider = super::scheduling::waiting_provider_with_affinity_and_limit(
                    &store,
                    leases,
                    server.uri(),
                    Arc::new(MemorySessionExclusions::default()),
                    affinity.clone(),
                    NonZeroU32::MIN,
                );
                let result = provider
                    .execute(
                        planned_request("openai", generate_operation()),
                        context_with_policy(
                            "req_cache_store_timeout",
                            CancellationToken::new(),
                            contract_account_scope(),
                            account_policy().with_openai_account_affinity(mode),
                        )
                        .with_request_tuning(
                            gateway_core::routing::RequestTuning {
                                account_busy_wait_enabled: waiting,
                                ..Default::default()
                            },
                        ),
                    )
                    .await;
                if mode == AccountAffinity::Preferred {
                    assert_eq!(
                        result.unwrap().metadata().provider_account_id().as_str(),
                        "acct_scope_old"
                    );
                } else {
                    let error = result
                        .err()
                        .expect("strict must not bypass the binding store");
                    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
                }
                assert_eq!(affinity.binding_count(), 0);
                assert!(server.received_requests().await.unwrap().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn strict_cache_preference_waits_and_preserves_binding_on_rejected_waits() {
    use gateway_core::account::{AccountSelectionPolicy, RotationStrategy};
    use std::sync::atomic::Ordering;

    for cache_hint in [false, true] {
        for outcome in [
            "released",
            "timeout",
            "cancelled",
            "disabled",
            "queue_disabled",
        ] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_scope_old").await;
            let affinity = Arc::new(MemorySessionAffinity::default());
            let leases = Arc::new(TestLeaseCoordinator::default());
            let server = MockServer::start().await;
            leases.capacity.enabled.store(true, Ordering::SeqCst);
            let provider = super::scheduling::waiting_provider_with_affinity_and_limit(
                &store,
                leases.clone(),
                server.uri(),
                Arc::new(MemorySessionExclusions::default()),
                affinity.clone(),
                NonZeroU32::MIN,
            );
            let policy = AccountSelectionPolicy::new(
                RotationStrategy::Smart,
                NonZeroU32::MIN,
                Duration::ZERO,
            );
            let select = || {
                let mut body = json!({"input":"same opening"});
                if cache_hint {
                    body["prompt_cache_key"] = json!("client-cache-key");
                }
                let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
                    ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
                        .unwrap(),
                ));
                provider.execute(
                    planned_request("openai", operation),
                    context_with_policy(
                        "req_strict_cache",
                        CancellationToken::new(),
                        contract_account_scope(),
                        policy,
                    )
                    .with_request_tuning(
                        gateway_core::routing::RequestTuning {
                            account_busy_wait_enabled: outcome != "queue_disabled",
                            max_waiting_per_account: 1,
                            account_concurrency_wait_timeout_seconds: 1,
                            ..Default::default()
                        },
                    ),
                )
            };
            drop(select().await.unwrap());
            create_account(&store, "acct_scope_new").await;
            let primary = ProviderAccountId::new("acct_scope_old").unwrap();
            leases.capacity.set_load("acct_scope_old", 1);
            leases.busy_accounts.lock().unwrap().insert(primary.clone());

            let mut waiting = Box::pin(select());
            if outcome != "queue_disabled" {
                assert!(
                    timeout(Duration::from_millis(30), waiting.as_mut())
                        .await
                        .is_err()
                );
                // 原账号占满时，即使另一个账号空闲也不分流，排队满后仍保留绑定
                let full = select()
                    .await
                    .err()
                    .expect("one waiting slot is already occupied");
                assert_eq!(full.send_state(), UpstreamSendState::NotSent);
            }
            match outcome {
                "released" => {
                    leases.capacity.set_load("acct_scope_old", 0);
                    leases.busy_accounts.lock().unwrap().clear();
                }
                "disabled" => store.set_enabled(&primary, false).await.unwrap(),
                "timeout" => {
                    let error = waiting.await.err().expect("strict queue timeout");
                    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
                    waiting = Box::pin(select());
                    {
                        leases.capacity.set_load("acct_scope_old", 0);
                        leases.busy_accounts.lock().unwrap().clear();
                    };
                }
                "cancelled" => {
                    // 丢弃请求 future 必须释放等待位置，后续请求仍能等原账号
                    drop(waiting);
                    waiting = Box::pin(select());
                    assert!(
                        timeout(Duration::from_millis(30), waiting.as_mut())
                            .await
                            .is_err()
                    );
                    {
                        leases.capacity.set_load("acct_scope_old", 0);
                        leases.busy_accounts.lock().unwrap().clear();
                    };
                }
                "queue_disabled" => {}
                _ => unreachable!(),
            }
            let expected = if matches!(outcome, "disabled" | "queue_disabled") {
                "acct_scope_new"
            } else {
                "acct_scope_old"
            };
            assert_eq!(
                waiting
                    .await
                    .unwrap_or_else(|error| panic!(
                        "cache_hint={cache_hint}, outcome={outcome}: {error:?}"
                    ))
                    .metadata()
                    .provider_account_id()
                    .as_str(),
                expected,
                "cache_hint={cache_hint}, outcome={outcome}"
            );
            {
                leases.capacity.set_load("acct_scope_old", 0);
                leases.busy_accounts.lock().unwrap().clear();
            };
            assert_eq!(
                select()
                    .await
                    .unwrap()
                    .metadata()
                    .provider_account_id()
                    .as_str(),
                expected,
                "cache_hint={cache_hint}, outcome={outcome}; subsequent selection"
            );
            assert_eq!(affinity.binding_count(), 1);
            assert_eq!(affinity.alias_count(), 0);
            assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
            assert!(server.received_requests().await.unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn relaxed_mode_skips_cache_preferences_and_sessionless_turn_aliases() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_affinity").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let provider = provider_with_affinity(&store, affinity.clone());
    for hint in [false, true] {
        let mut body = json!({"input":"hello", "turn_id":"unowned-turn"});
        if hint {
            body["prompt_cache_key"] = json!("client-hint");
        }
        let payload =
            ProtocolPayload::json_object("openai", body.as_object().unwrap().clone()).unwrap();
        drop(
            provider
                .execute(
                    planned_request(
                        "openai",
                        Operation::Generate(GenerateRequest::from_protocol_payload(payload)),
                    ),
                    relaxed_context("req_relaxed_cache", CancellationToken::new()),
                )
                .await
                .unwrap(),
        );
    }
    assert!(affinity.lookup_keys().is_empty());
    assert_eq!(affinity.binding_count(), 0);
    assert_eq!(affinity.alias_count(), 0);
}

#[tokio::test]
async fn content_preference_spills_over_in_preferred_mode_and_preserves_turn_state() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(CAPTURE_COMPLETED_SSE),
        )
        .expect(3)
        .mount(&server)
        .await;
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        affinity.clone(),
        server.uri(),
        leases.clone(),
    );
    for (turn, busy) in [("first", false), ("spillover", true), ("recovered", false)] {
        if busy {
            create_account(&store, "acct_subagent_b").await;
            leases
                .busy_accounts
                .lock()
                .unwrap()
                .insert(ProviderAccountId::new("acct_subagent_a").unwrap());
        } else {
            leases.busy_accounts.lock().unwrap().clear();
        }
        let (operation, attempt) = {
            let mut input = vec![
                json!({"role":"developer", "content":"review"}),
                json!({"role":"user","content":"check this patch"}),
            ];
            if turn != "first" {
                input.push(json!({"role":"assistant","content":"ready"}));
                input.push(json!({"role":"user","content":turn}));
            }
            let payload = ProtocolPayload::json_object(
                "openai",
                Map::from_iter([("input".into(), json!(input))]),
            )
            .unwrap()
            .with_context(Map::from_iter([
                ("use_websocket".to_owned(), json!(false)),
                ("turn_id".to_owned(), json!(turn)),
                ("turn_state".to_owned(), json!("unowned-turn-state")),
            ]));
            (
                Operation::Generate(GenerateRequest::from_protocol_payload(payload)),
                context_with_policy(
                    "req_preference_wire",
                    CancellationToken::new(),
                    contract_account_scope(),
                    account_policy().with_openai_account_affinity(
                        gateway_core::account::AccountAffinity::Preferred,
                    ),
                ),
            )
        };
        let mut stream = provider
            .execute(planned_request("openai", operation), attempt)
            .await
            .unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
    }
    let sent = server.received_requests().await.unwrap();
    let accounts = sent
        .iter()
        .map(|request| request.headers["chatgpt-account-id"].to_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        accounts,
        [
            "chatgpt-acct_subagent_a",
            "chatgpt-acct_subagent_b",
            "chatgpt-acct_subagent_a"
        ]
    );
    assert_eq!(affinity.binding_count(), 1);
    {
        assert_eq!(affinity.alias_count(), 0);
        for request in &sent {
            let body = captured_request_body(request);
            assert!(body.get("client_account_session_id").is_none());
            assert!(body.get("cache_preference").is_none());
            assert_eq!(
                captured_header_values(request, "x-codex-turn-state"),
                vec![b"unowned-turn-state".to_vec()]
            );
        }
    }
    server.verify().await;
}

#[tokio::test]
async fn preferred_queued_cache_read_failure_discards_preference_without_rebinding() {
    use gateway_core::account::{AccountAffinity, AccountSelectionPolicy, RotationStrategy};
    use gateway_core::provider_ports::{ProviderSessionAffinityKey, ProviderSessionAffinityPort};
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    let server = MockServer::start().await;
    let provider = super::scheduling::waiting_provider_with_affinity_and_limit(
        &store,
        leases.clone(),
        server.uri(),
        Arc::new(MemorySessionExclusions::default()),
        affinity.clone(),
        NonZeroU32::MIN,
    );
    let select = || {
        provider.execute(
            planned_request("openai", generate_operation()),
            context_with_policy(
                "req_optional_queue",
                CancellationToken::new(),
                contract_account_scope(),
                AccountSelectionPolicy::new(
                    RotationStrategy::Smart,
                    NonZeroU32::new(2).unwrap(),
                    Duration::ZERO,
                )
                .with_openai_account_affinity(AccountAffinity::Preferred),
            )
            .with_request_tuning(gateway_core::routing::RequestTuning {
                account_busy_wait_enabled: true,
                ..Default::default()
            }),
        )
    };
    drop(select().await.unwrap());
    let key = ProviderSessionAffinityKey::try_new(affinity.lookup_keys()[0].clone()).unwrap();
    create_account(&store, "acct_scope_new").await;
    leases.capacity.set_load("acct_scope_old", 1);
    leases.capacity.set_load("acct_scope_new", 1);
    let mut pending = Box::pin(select());
    assert!(
        timeout(Duration::from_millis(30), pending.as_mut())
            .await
            .is_err()
    );
    assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 1);
    affinity.fail_next_lookup();
    leases.capacity.set_load("acct_scope_new", 0);
    let selected = timeout(Duration::from_secs(3), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        selected.metadata().provider_account_id().as_str(),
        "acct_scope_new"
    );
    assert_eq!(
        affinity
            .load(&ProviderKind::new("openai").unwrap(), &key)
            .await
            .unwrap()
            .unwrap()
            .as_str(),
        "acct_scope_old"
    );
    assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn round_robin_retains_the_assignment_when_a_cache_preference_cannot_be_saved() {
    use gateway_core::account::{AccountAffinity, AccountSelectionPolicy, RotationStrategy};
    use gateway_core::provider_ports::ProviderSessionAffinityPort;

    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_affinity").await;
    let affinity = Arc::new(MemorySessionAffinity::with_claim_barrier(2));
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    let attempt = context_with_policy(
        "req_cache_rr",
        CancellationToken::new(),
        contract_account_scope(),
        AccountSelectionPolicy::new(
            RotationStrategy::RoundRobin,
            NonZeroU32::MIN,
            Duration::ZERO,
        )
        .with_openai_account_affinity(AccountAffinity::Preferred),
    );
    let key = attempt.client_api_key_ref().clone();
    let result = provider
        .execute(planned_request("openai", generate_operation()), attempt)
        .await
        .unwrap();
    assert_eq!(
        result.metadata().provider_account_id().as_str(),
        "acct_affinity"
    );
    assert_eq!(affinity.binding_count(), 0);
    assert_eq!(
        affinity
            .round_robin_cursor(&ProviderKind::new("openai").unwrap(), &key)
            .await
            .unwrap(),
        1
    );
}
