//! 验证真实 OpenAI Provider 经 Core 重试收尾时保留上游错误与本地会话边界

use super::*;
use async_trait::async_trait;
use gateway_core::engine::provider::ProviderRegistry;
use gateway_core::engine::{
    AttemptCoordinator, AttemptRecord, EngineError, ExecutionStore, GatewayEngine,
    IntermediateFailure, ModelRequestFinalization, NewModelRequest, RecoveryReport,
};
use gateway_core::error::StoreError;
use gateway_core::routing::AccountRoutingSnapshot;

struct NoopExecutionStore;

#[async_trait]
impl ExecutionStore for NoopExecutionStore {
    async fn create_model_request(&self, _: NewModelRequest) -> Result<(), StoreError> {
        Ok(())
    }

    async fn record_attempt(&self, _: AttemptRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn mark_send_state(
        &self,
        _: &ModelRequestId,
        _: UpstreamSendState,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn mark_downstream_committed(
        &self,
        _: &ModelRequestId,
        _: SystemTime,
        _: Option<u16>,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn record_client_status(&self, _: &ModelRequestId, _: u16) -> Result<(), StoreError> {
        Ok(())
    }

    async fn record_intermediate_failure(&self, _: IntermediateFailure) -> Result<(), StoreError> {
        Ok(())
    }

    async fn finalize_model_request(&self, _: ModelRequestFinalization) -> Result<(), StoreError> {
        Ok(())
    }

    async fn recover_expired(&self, _: SystemTime) -> Result<RecoveryReport, StoreError> {
        Ok(RecoveryReport::default())
    }
}

#[tokio::test]
async fn refused_connections_respect_the_same_rotation_limit_over_http_and_websocket() {
    let upstream = MockServer::start().await;
    for websocket in [false, true] {
        for (rotations, child) in [(0, false), (2, false), (2, true)] {
            if child && websocket {
                continue;
            }
            let store = Arc::new(MemoryAccountStore::default());
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = listener.local_addr().unwrap();
            drop(listener);
            let proxy =
                gateway_core::account::OutboundProxy::parse(&format!("socks5h://{endpoint}"))
                    .unwrap();
            for id in ["acct_provider_contract", "acct_subagent_b"] {
                create_account(&store, id).await;
                store.set_outbound_proxy(id, proxy.clone());
            }
            let operation = if child {
                Operation::Generate(GenerateRequest::from_protocol_payload(
                    generate_with_session_context(
                        "root",
                        Some("child"),
                        Some(r#"{"subagent_kind":"thread_spawn"}"#),
                    )
                    .protocol_payload()
                    .clone()
                    .with_context(Map::from_iter([("use_websocket".into(), json!(false))])),
                ))
            } else if websocket {
                generate_operation()
            } else {
                http_generate_operation()
            };
            let leases = Arc::new(TestLeaseCoordinator::default());
            let provider = provider_with_affinity_and_base_url_and_leases(
                &store,
                Arc::new(MemorySessionAffinity::default()),
                upstream.uri(),
                Arc::clone(&leases),
            );
            let providers = ProviderRegistry::new([
                Arc::new(provider) as Arc<dyn gateway_core::engine::provider::Provider>
            ])
            .unwrap();
            let coordinator = AttemptCoordinator::new(GatewayEngine::new(
                Arc::new(NoopExecutionStore),
                providers,
            ));
            let settings = gateway_core::routing::RequestTuning {
                max_account_switches: rotations,
                ..Default::default()
            };
            let plan = execution_plan_with_settings(&operation, settings);
            let mut session = coordinator
                .start(
                    execution_request("req_refused", &operation, Duration::from_secs(15)),
                    operation,
                    plan,
                    None,
                    None,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            let result = session.collect_uncommitted().await;
            assert!(
                matches!(result, Err(EngineError::Provider(_))),
                "ws={websocket}, rotations={rotations}: {result:?}"
            );
            let requests = leases.requests.lock().unwrap();
            let accounts: Vec<_> = requests.iter().map(|r| r.account_id()).collect();
            if rotations == 0 {
                assert!((3..=4).contains(&accounts.len()), "{accounts:?}");
            } else {
                assert!(
                    matches!(&result, Err(EngineError::Provider(error)) if error.unreachable_proxy().is_some()),
                    "{result:?}"
                );
                assert_eq!(
                    accounts.len(),
                    2,
                    "ws={websocket}: {accounts:?}, {result:?}"
                );
            }
            assert!(
                accounts.iter().all(|a| *a == accounts[0]),
                "ws={websocket}: {accounts:?}"
            );
        }
    }
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn unreachable_proxy_is_skipped_across_rotations_and_reconsidered_on_the_next_request() {
    use gateway_core::account::OutboundProxy;

    const A: &str = "acct_provider_contract";
    const B: &str = "acct_subagent_a";
    const C: &str = "acct_subagent_b";
    const D: &str = "acct_affinity";
    let store = Arc::new(MemoryAccountStore::default());
    let upstream = MockServer::start().await;
    // 关闭监听以产生真实的连接拒绝，下一请求让同一入口恢复服务
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    for (id, weight) in [(A, 100), (B, 90), (C, 80), (D, 70)] {
        create_account(&store, id).await;
        store.set_scheduling(id, None, AccountWeight::new(weight).unwrap());
        if matches!(id, A | B) {
            let proxy = OutboundProxy::parse(&format!("http://{id}@{addr}")).unwrap();
            store.set_outbound_proxy(id, proxy);
        }
    }
    let rate_limit_then_success = || {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        Mock::given(method("POST"))
            .respond_with(move |_: &wiremock::Request| {
                if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(429).set_body_json(json!({
                        "error":{"type":"rate_limit_error","message":"fixture rate limit"}
                    }))
                } else {
                    ResponseTemplate::new(200)
                        .set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
                }
            })
            .expect(2)
    };
    rate_limit_then_success().mount(&upstream).await;
    let leases = Arc::new(TestLeaseCoordinator::default());
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        Arc::new(MemorySessionAffinity::default()),
        upstream.uri(),
        Arc::clone(&leases),
    );
    let providers = ProviderRegistry::new([
        Arc::new(provider) as Arc<dyn gateway_core::engine::provider::Provider>
    ])
    .unwrap();
    let coordinator =
        AttemptCoordinator::new(GatewayEngine::new(Arc::new(NoopExecutionStore), providers));
    let run = async |id: &'static str| {
        // 每个请求启动独立会话，避免已有的会话亲和优先于测试设置的账号权重
        let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object(
                "openai",
                Map::from_iter([
                    ("model".into(), json!("gpt-5.4")),
                    ("input".into(), json!("hello")),
                    ("prompt_cache_key".into(), json!(id)),
                ]),
            )
            .unwrap()
            .with_context(Map::from_iter([("use_websocket".into(), json!(false))])),
        ));
        let plan = execution_plan_with_settings(
            &operation,
            gateway_core::routing::RequestTuning {
                max_account_switches: 2,
                ..Default::default()
            },
        );
        let mut session = coordinator
            .start(
                execution_request(id, &operation, Duration::from_secs(15)),
                operation,
                plan,
                None,
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let result = session.collect_uncommitted().await;
        let accounts = std::mem::take(&mut *leases.requests.lock().unwrap())
            .iter()
            .map(|r| r.account_id().as_str().to_owned())
            .collect::<Vec<_>>();
        assert!(result.is_ok(), "{id}: {result:?}, accounts={accounts:?}");
        accounts
    };
    // C 的限流再次触发换号，B 权重更高也不能换回已失败的入口
    assert_eq!(run("req_proxy_skip").await, [A, A, C, D]);
    let recovered = MockServer::builder()
        .listener(std::net::TcpListener::bind(addr).unwrap())
        .start()
        .await;
    rate_limit_then_success().mount(&recovered).await;
    // 新请求可重新使用 A；这次 A 只被限流，同一入口的 B 仍是最优账号
    assert_eq!(run("req_proxy_recovered").await, [A, B]);
}

fn execution_plan_with_settings(
    operation: &Operation,
    settings: gateway_core::routing::RequestTuning,
) -> gateway_core::routing::RoutingPlan {
    execution_plan_with_policy(operation, settings, account_policy())
}

fn execution_plan_with_policy(
    operation: &Operation,
    settings: gateway_core::routing::RequestTuning,
    policy: gateway_core::account::AccountSelectionPolicy,
) -> gateway_core::routing::RoutingPlan {
    let kind = ProviderKind::new("openai").unwrap();
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        policy,
        vec![kind.clone()],
        vec![ProviderModel::new(
            kind,
            UpstreamModelId::new("gpt-5.4").unwrap(),
            ModelCapabilities::new(BTreeSet::from([operation.kind()]), Some(32_000))
                .with_upstream_feature_validation(),
        )],
        Vec::new(),
    )
    .unwrap();
    snapshot
        .with_request_tuning(settings)
        .plan(
            &PublicModelId::new("gpt-5.4").unwrap(),
            operation,
            contract_account_scope(),
            &RoutingContext::default(),
        )
        .unwrap()
}

#[tokio::test]
async fn native_queue_timeout_recovers_through_core_and_full_client_replay() {
    use gateway_core::account::{AccountAffinity, AccountSelectionPolicy, RotationStrategy};
    use gateway_core::provider_ports::{ProviderSessionAffinityKey, ProviderSessionAffinityPort};
    use std::sync::atomic::Ordering;

    let owner = ProviderAccountId::new("acct_scope_old").unwrap();
    let spare = ProviderAccountId::new("acct_scope_new").unwrap();
    let kind = ProviderKind::new("openai").unwrap();
    for mode in [
        AccountAffinity::Strict,
        AccountAffinity::Preferred,
        AccountAffinity::Relaxed,
    ] {
        for busy in [true, false] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, owner.as_str()).await;
            let upstream = MockServer::start().await;
            let affinity = Arc::new(MemorySessionAffinity::default());
            let leases = Arc::new(TestLeaseCoordinator::default());
            leases.capacity.enabled.store(true, Ordering::SeqCst);
            let provider = super::scheduling::waiting_provider_with_affinity_and_limit(
                &store,
                leases.clone(),
                upstream.uri(),
                Arc::default(),
                affinity.clone(),
                NonZeroU32::MIN,
            );
            let full_input = json!([
                {"role":"user","content":"first turn"},
                {"role":"assistant","content":"first answer"},
                {"role":"user","content":"continue"},
            ]);
            let replay = Operation::Generate(GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object("openai", json!({
                    "model":"gpt-5.4", "session_id":"queue-replay", "thread_id":"queue-replay", "input":full_input,
                }).as_object().unwrap().clone()).unwrap()
                .with_context(Map::from_iter([("use_websocket".into(), json!(false))])),
            ));
            // Establish a real primary binding without polling the upstream stream.
            drop(
                provider
                    .execute(
                        planned_request("openai", replay.clone()),
                        context("req_queue_seed", CancellationToken::new()),
                    )
                    .await
                    .unwrap(),
            );
            let affinity_key =
                ProviderSessionAffinityKey::try_new(affinity.lookup_keys()[0].clone()).unwrap();
            assert_eq!(
                affinity.load(&kind, &affinity_key).await.unwrap(),
                Some(owner.clone())
            );
            create_account(&store, spare.as_str()).await;
            leases.capacity.set_load(owner.as_str(), 1);
            leases.capacity.set_load(spare.as_str(), u32::from(busy));
            let Operation::Generate(generate) = &replay else {
                unreachable!()
            };
            let mut body = generate.protocol_payload().body().clone();
            body.insert("input".into(), json!("continue"));
            body.insert("previous_response_id".into(), json!("resp_previous"));
            let native = Operation::Generate(GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object("openai", body).unwrap()
                    .with_context(generate.protocol_payload().context().clone()),
            ).with_provider_session_state(ProviderSessionState::new("openai", json!({
                "account_id":owner.as_str(), "continuation_scope":"persisted", "responses_upstream":"codex",
            }).as_object().unwrap().clone()).unwrap()));
            let pin = NativeContinuationPin::new(
                PreviousResponseId::new("resp_previous"),
                PreviousResponseId::new("resp_previous"),
                ClientApiKeyId::new("key_openai_contract").unwrap(),
                kind.clone(),
                owner.clone(),
            );
            let policy = AccountSelectionPolicy::new(
                RotationStrategy::Smart,
                NonZeroU32::MIN,
                Duration::ZERO,
            )
            .with_account_affinity(mode)
            .with_switch_account_on_queue_failure(false);
            let settings = gateway_core::routing::RequestTuning {
                account_busy_wait_enabled: true,
                account_concurrency_wait_timeout_seconds: 1,
                max_account_switches: 1,
                ..Default::default()
            };
            let providers = ProviderRegistry::new([
                Arc::new(provider) as Arc<dyn gateway_core::engine::provider::Provider>
            ])
            .unwrap();
            let coordinator = AttemptCoordinator::new(GatewayEngine::new(
                Arc::new(NoopExecutionStore),
                providers,
            ));
            let plan = execution_plan_with_policy(&native, settings, policy);
            let mut session = coordinator
                .start(
                    execution_request("req_queue_core", &native, Duration::from_secs(10)),
                    native,
                    plan,
                    None,
                    Some(ContinuationBinding::Pinned(pin)),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            // Core, rather than the test, creates the recovery attempt with the same exhausted queue budget.
            let Err(EngineError::Provider(error)) = session.collect_uncommitted().await else {
                panic!("expected a terminal local selection error: {mode:?} busy={busy}");
            };
            assert_eq!(error.kind(), ProviderErrorKind::ConcurrencyQueueTimeout);
            assert_eq!(error.send_state(), UpstreamSendState::NotSent);
            assert_eq!(
                error.client_visible_upstream_error().and_then(|e| e.code()),
                (!busy).then_some("previous_response_not_found")
            );
            assert_eq!(leases.capacity.waits.lock().unwrap().len(), 1);
            assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
            assert_eq!(
                leases.capacity.signals.lock().unwrap()[&spare].in_flight,
                u32::from(busy)
            );
            assert!(upstream.received_requests().await.unwrap().is_empty());
            let expected_primary = if busy || mode == AccountAffinity::Preferred {
                &owner
            } else {
                &spare
            };
            assert_eq!(
                affinity.load(&kind, &affinity_key).await.unwrap().as_ref(),
                Some(expected_primary)
            );
            if busy {
                continue;
            }

            Mock::given(method("POST"))
                .and(path("/codex/responses"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
                )
                .expect(1)
                .mount(&upstream)
                .await;
            leases.requests.lock().unwrap().clear();
            let replay_plan = execution_plan_with_policy(&replay, settings, policy);
            let mut replay_session = coordinator
                .start(
                    execution_request("req_queue_replay", &replay, Duration::from_secs(10)),
                    replay,
                    replay_plan,
                    None,
                    None,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            assert!(replay_session.collect_uncommitted().await.is_ok());
            assert_eq!(
                leases.requests.lock().unwrap().last().unwrap().account_id(),
                &spare
            );
            let requests = upstream.received_requests().await.unwrap();
            assert_eq!(requests.len(), 1);
            let sent = captured_request_body(&requests[0]);
            assert!(sent.get("previous_response_id").is_none());
            assert_eq!(sent["input"], full_input);
        }
    }
}
fn execution_request(id: &str, operation: &Operation, timeout: Duration) -> NewModelRequest {
    NewModelRequest {
        admission_decision_ms: None,
        id: ModelRequestId::new(id).unwrap(),
        client_api_key_id: None,
        client_api_key_ref: ClientApiKeyId::new("key_openai_contract").unwrap(),
        config_revision: ConfigRevision::new(1).unwrap(),
        routing: AccountRoutingSnapshot::all(),
        protocol: "openai".to_owned(),
        operation: operation.kind(),
        endpoint: "responses".to_owned(),
        client_transport: "http_sse".to_owned(),
        requested_model: Some(PublicModelId::new("gpt-5.4").unwrap()),
        client_ip: None,
        user_agent: None,
        reasoning_effort: None,
        reasoning_preset: None,
        request_kind: None,
        subagent_kind: None,
        compact: false,
        continuation: Default::default(),
        image_generation_requested: false,
        started_at: SystemTime::now(),
        deadline_at: (SystemTime::now() + timeout).into(),
    }
}
