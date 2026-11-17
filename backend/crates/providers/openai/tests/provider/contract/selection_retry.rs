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
    let kind = ProviderKind::new("openai").unwrap();
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        account_policy(),
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
