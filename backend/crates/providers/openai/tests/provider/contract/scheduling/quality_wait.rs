use super::*;
use async_trait::async_trait;
use futures::future::BoxFuture;
use gateway_core::account::{AccountSelectionPolicy, RotationStrategy};
use gateway_core::engine::admission::*;
use gateway_core::engine::continuation::{NativeContinuationPort, NativeContinuationStoreError};
use gateway_core::engine::execution::{
    ClientApiKeyUsageSink, DefaultExecutionService, ProviderCircuitDecision, ProviderCircuitError,
    ProviderCircuitPort,
};
use gateway_core::engine::probe::{AccountProbe, AccountProbeRequest};
use gateway_core::engine::provider::ProviderRegistry;
use gateway_core::engine::{
    AttemptRecord, ExecutionStore, IntermediateFailure, ModelRequestFinalization, NewModelRequest,
    RecoveryReport,
};
use gateway_core::error::StoreError;
use gateway_core::runtime::RuntimeSnapshotHandle;
use std::sync::atomic::Ordering;

// Exercise the trusted Core quality entry point; client JSON cannot set its marker.
#[derive(Default)]
struct QualityPorts {
    requests: Mutex<Vec<NewModelRequest>>,
    finalizations: Mutex<Vec<ModelRequestFinalization>>,
}

#[async_trait]
impl ExecutionStore for QualityPorts {
    async fn create_model_request(&self, request: NewModelRequest) -> Result<(), StoreError> {
        self.requests.lock().unwrap().push(request);
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
    async fn finalize_model_request(
        &self,
        result: ModelRequestFinalization,
    ) -> Result<(), StoreError> {
        self.finalizations.lock().unwrap().push(result);
        Ok(())
    }
    async fn recover_expired(&self, _: SystemTime) -> Result<RecoveryReport, StoreError> {
        Ok(RecoveryReport::default())
    }
}

impl ClientAdmissionPort for QualityPorts {
    fn admit(
        &self,
        _: ClientAdmissionRequest,
    ) -> BoxFuture<'_, Result<ClientAdmissionDecision, ClientAdmissionError>> {
        panic!("quality must not use user admission")
    }
    fn release<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a ModelRequestId,
    ) -> BoxFuture<'a, Result<bool, ClientAdmissionError>> {
        panic!("quality must not use user admission")
    }
    fn restore(
        &self,
        _: ClientAdmissionRecovery,
    ) -> BoxFuture<'_, Result<ClientAdmissionRestoreResult, ClientAdmissionError>> {
        panic!("quality must not use user admission")
    }
}

impl ProviderCircuitPort for QualityPorts {
    fn decision<'a>(
        &'a self,
        _: &'a ProviderKind,
    ) -> BoxFuture<'a, Result<ProviderCircuitDecision, ProviderCircuitError>> {
        Box::pin(async { Ok(ProviderCircuitDecision::Allow) })
    }
    fn observe_failure<'a>(
        &'a self,
        _: &'a ProviderKind,
    ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
        Box::pin(async { Ok(()) })
    }
    fn observe_success<'a>(
        &'a self,
        _: &'a ProviderKind,
    ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
        Box::pin(async { Ok(()) })
    }
}

impl NativeContinuationPort for QualityPorts {
    fn resolve<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a PreviousResponseId,
    ) -> BoxFuture<'a, Result<Option<NativeContinuationPin>, NativeContinuationStoreError>> {
        panic!("quality must not resolve user continuation")
    }
    fn record<'a>(
        &'a self,
        _: NativeContinuationPin,
    ) -> BoxFuture<'a, Result<(), NativeContinuationStoreError>> {
        Box::pin(async { Ok(()) })
    }
}

impl ClientApiKeyUsageSink for QualityPorts {
    fn record_used(&self, _: &ClientApiKeyId) {
        panic!("quality must not charge a user key")
    }
}

fn quality_service(
    provider: CodexProvider,
    interval: Duration,
    waiting: bool,
    ports: Arc<QualityPorts>,
) -> DefaultExecutionService {
    let kind = ProviderKind::new("openai").unwrap();
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        AccountSelectionPolicy::new(RotationStrategy::Sticky, NonZeroU32::MIN, interval),
        vec![kind.clone()],
        vec![ProviderModel::new(
            kind.clone(),
            UpstreamModelId::new("gpt-5.4").unwrap(),
            ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), None),
        )],
        Vec::new(),
    )
    .unwrap()
    .with_request_tuning(gateway_core::routing::RequestTuning {
        account_busy_wait_enabled: waiting,
        account_busy_wait_sticky_max_waiting: 3,
        account_busy_wait_fallback_max_waiting: 3,
        account_busy_wait_sticky_timeout_seconds: 1,
        account_busy_wait_fallback_timeout_seconds: 1,
        ..Default::default()
    })
    .with_account_directory(Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
        (
            ProviderAccountId::new("acct_scope_old").unwrap(),
            RuntimeAccount::new(kind.clone(), BTreeSet::new()),
        ),
        (
            ProviderAccountId::new("acct_scope_new").unwrap(),
            RuntimeAccount::new(kind, BTreeSet::new()),
        ),
    ]))));
    DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(snapshot),
        ports.clone(),
        ProviderRegistry::new([
            Arc::new(provider) as Arc<dyn gateway_core::engine::provider::Provider>
        ])
        .unwrap(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports,
    )
}

#[tokio::test]
async fn quality_interval_wait_bypasses_business_queue_and_rechecks_safety() {
    for scenario in [
        "open-queue",
        "full",
        "disabled-wait",
        "contention",
        "lease-race",
        "cancel",
        "disabled-account",
    ] {
        let accounts = Arc::new(MemoryAccountStore::default());
        create_account(&accounts, "acct_scope_old").await;
        create_account(&accounts, "acct_scope_new").await;
        let id = ProviderAccountId::new("acct_scope_old").unwrap();
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        let waiting = if scenario == "open-queue" { 0 } else { 3 };
        leases.capacity.full.store(waiting == 3, Ordering::SeqCst);
        leases.capacity.waiting.store(waiting, Ordering::SeqCst);
        leases.capacity.set_load(id.as_str(), 30);
        let server = local_server().await;
        let ports = Arc::new(QualityPorts::default());
        let service = quality_service(
            waiting_provider(&accounts, leases.clone(), server.uri()),
            Duration::from_millis(250),
            scenario != "disabled-wait",
            ports.clone(),
        );
        let started = SystemTime::now();
        if scenario == "lease-race" {
            let capacity = leases.capacity.clone();
            *leases.capacity.after_state_load.lock().unwrap() = Some(Box::new(move || {
                capacity
                    .signals
                    .lock()
                    .unwrap()
                    .get_mut(&ProviderAccountId::new("acct_scope_old").unwrap())
                    .unwrap()
                    .last_started_at = Some(SystemTime::now());
            }));
        } else {
            leases
                .capacity
                .signals
                .lock()
                .unwrap()
                .get_mut(&id)
                .unwrap()
                .last_started_at = Some(started);
        }
        let cancellation = CancellationToken::new();
        let check = service.quality_check(
            AccountProbeRequest {
                account_id: id.clone(),
                provider_kind: ProviderKind::new("openai").unwrap(),
                upstream_model: UpstreamModelId::new("gpt-5.4").unwrap(),
                operation: http_generate_operation(),
            },
            cancellation.clone(),
        );
        tokio::pin!(check);
        tokio::select! {
            result = &mut check => panic!("quality must wait without business admission ({scenario}): {result:?}"),
            () = tokio::time::sleep(Duration::from_millis(60)) => {}
        }
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(
            leases.capacity.waits.lock().unwrap().is_empty(),
            "{scenario}"
        );
        let earliest = if scenario == "contention" {
            let moved = SystemTime::now();
            leases
                .capacity
                .signals
                .lock()
                .unwrap()
                .get_mut(&id)
                .unwrap()
                .last_started_at = Some(moved);
            moved + Duration::from_millis(250)
        } else {
            started + Duration::from_millis(250)
        };
        if scenario == "cancel" {
            cancellation.cancel();
        }
        if scenario == "disabled-account" {
            accounts.set_enabled(&id, false).await.unwrap();
        }
        let result = timeout(Duration::from_secs(3), check).await.unwrap();
        if matches!(scenario, "cancel" | "disabled-account") {
            assert!(result.is_err(), "{scenario}");
            assert!(server.received_requests().await.unwrap().is_empty());
        } else {
            result.unwrap();
            assert!(
                SystemTime::now() >= earliest,
                "interval must remain enforced"
            );
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
            let requests = leases.requests.lock().unwrap();
            assert!(!requests.is_empty());
            assert!(
                requests
                    .iter()
                    .all(|r| r.account_id() == &id && r.is_quality_check())
            );
            let recorded = ports.requests.lock().unwrap();
            assert_eq!(
                requests.last().unwrap().deadline(),
                recorded[0].deadline_at,
                "selection cannot restart the request budget"
            );
        }
        assert!(
            leases.capacity.waits.lock().unwrap().is_empty(),
            "{scenario}"
        );
        assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), waiting);
        assert_eq!(leases.capacity.promotions.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn state_probe_wait_uses_each_shots_total_budget_not_business_wait_window() {
    use gateway_core::operation::quality_probe::StateProbeVerdict;

    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_scope_old").await;
    let id = ProviderAccountId::new("acct_scope_old").unwrap();
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    leases.capacity.full.store(true, Ordering::SeqCst);
    leases.capacity.set_load(id.as_str(), 30);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("x-codex-turn-state", "synthetic-quality-state")
                .set_body_string(format!(
                    "event: response.created\ndata: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_scope_capture\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\"}}}}\n\n{CAPTURE_COMPLETED_SSE}"
                )),
        )
        .mount(&server)
        .await;
    let ports = Arc::new(QualityPorts::default());
    let interval = Duration::from_millis(1200);
    let service = quality_service(
        waiting_provider(&accounts, leases.clone(), server.uri()),
        interval,
        true,
        ports.clone(),
    );
    leases
        .capacity
        .signals
        .lock()
        .unwrap()
        .get_mut(&id)
        .unwrap()
        .last_started_at = Some(SystemTime::now());
    let report = timeout(
        Duration::from_secs(5),
        service.state_probe(
            AccountProbeRequest {
                account_id: id.clone(),
                provider_kind: ProviderKind::new("openai").unwrap(),
                upstream_model: UpstreamModelId::new("gpt-5.4").unwrap(),
                operation: http_generate_operation(),
            },
            CancellationToken::new(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        report.verdict,
        StateProbeVerdict::Healthy,
        "{report:?}, finalizations: {:?}",
        ports.finalizations.lock().unwrap()
    );
    assert_eq!(report.shots.len(), 2);
    assert!(leases.capacity.waits.lock().unwrap().is_empty());
    let sent = server.received_requests().await.unwrap();
    let requests = ports.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let acquired = leases.requests.lock().unwrap();
    assert_eq!(acquired.len(), 2);
    for (shot, lease) in requests.iter().zip(acquired.iter()) {
        assert_eq!(
            shot.deadline_at
                .at()
                .unwrap()
                .duration_since(shot.started_at)
                .unwrap(),
            Duration::from_secs(60)
        );
        assert_eq!(lease.deadline(), shot.deadline_at);
        assert!(lease.is_quality_check());
        assert_eq!(lease.account_id(), &id);
    }
    assert!(
        requests[1]
            .started_at
            .duration_since(requests[0].started_at)
            .unwrap()
            >= Duration::from_secs(1)
    );
    assert_eq!(sent.len(), 2);
    assert!(!sent[0].headers.contains_key("x-codex-turn-state"));
    assert_eq!(
        sent[1].headers["x-codex-turn-state"],
        "synthetic-quality-state"
    );
}

#[tokio::test]
async fn state_probe_wait_and_response_share_sixty_second_timeout() {
    use gateway_core::operation::quality_probe::StateProbeVerdict;

    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_scope_old").await;
    let id = ProviderAccountId::new("acct_scope_old").unwrap();
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    leases.capacity.full.store(true, Ordering::SeqCst);
    leases.capacity.set_load(id.as_str(), 30);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("x-codex-turn-state", "synthetic-quality-state")
                .set_delay(Duration::from_secs(50))
                .set_body_string(format!(
                    "event: response.created\ndata: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_scope_capture\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\"}}}}\n\n{CAPTURE_COMPLETED_SSE}"
                )),
        )
        .mount(&server)
        .await;
    let ports = Arc::new(QualityPorts::default());
    let service = quality_service(
        waiting_provider(&accounts, leases.clone(), server.uri()),
        Duration::from_secs(20),
        true,
        ports.clone(),
    );
    leases
        .capacity
        .signals
        .lock()
        .unwrap()
        .get_mut(&id)
        .unwrap()
        .last_started_at = Some(SystemTime::now());
    let started = std::time::Instant::now();
    let probe = service.state_probe(
        AccountProbeRequest {
            account_id: id.clone(),
            provider_kind: ProviderKind::new("openai").unwrap(),
            upstream_model: UpstreamModelId::new("gpt-5.4").unwrap(),
            operation: http_generate_operation(),
        },
        CancellationToken::new(),
    );
    tokio::pin!(probe);
    tokio::select! {
        result = &mut probe => panic!("old 45-second or business wait deadline ended the shot: {result:?}"),
        () = tokio::time::sleep(Duration::from_secs(47)) => {}
    }
    let report = timeout(Duration::from_secs(18), probe)
        .await
        .expect("waiting must not add another timeout window");
    assert_eq!(report.verdict, StateProbeVerdict::Inconclusive);
    assert!(started.elapsed() >= Duration::from_secs(59));
    assert!(started.elapsed() < Duration::from_secs(65));
    assert_eq!(
        ports.requests.lock().unwrap().len(),
        1,
        "failed first shot must stop the round"
    );
    assert_eq!(
        leases.capacity.signals.lock().unwrap()[&id].in_flight,
        30,
        "quality execution lease must be released"
    );
    assert!(leases.capacity.waits.lock().unwrap().is_empty());
}
