use super::*;
use gateway_core::{
    account::{AccountErrorReason, CredentialState},
    task::{ScheduledTask, WorkerTaskError},
};

struct Fixture {
    accounts: Arc<MemoryAccountStore>,
    states: Arc<ProbeStates>,
    leases: Arc<ReadOnlyLeases>,
    server: MockServer,
    discovery: Box<dyn ScheduledTask>,
    cycle: WorkerCycleContext,
    cancellation: CancellationToken,
    worker: tokio::task::JoinHandle<Result<(), WorkerTaskError>>,
}

impl Fixture {
    async fn new() -> Self {
        let accounts = Arc::new(MemoryAccountStore::default());
        let mut imported = CodexCredentialAdmin
            .prepare_import(ImportCodexOAuthCredential {
                account_id: "acct_recovery_probe".into(),
                name: "Maintenance recovery fixture".into(),
                secret: secret("synthetic-recovery"),
                verified_account: profile("synthetic-recovery-owner"),
                next_refresh_at: None,
                enabled: true,
            })
            .unwrap();
        imported.account = imported.account.with_turn_state_injection_enabled(true);
        accounts.create_account(imported).await.unwrap();
        let base = provider_ports_with_accounts(accounts.clone());
        let states = Arc::new(ProbeStates::default());
        let leases = Arc::new(ReadOnlyLeases::default());
        let ports = ProviderStorePorts::new(
            base.accounts(),
            leases.clone(),
            base.session_affinity(),
            base.session_exclusions(),
            base.catalog_cache(),
            base.artifact_profiles(),
            base.credential_state(),
            base.cooldowns(),
            base.runtime_policy(),
            base.oauth_pending(),
        )
        .with_turn_states(states.clone())
        .with_egress(Arc::new(LoopbackEgress(ProviderEgressConfig {
            revision: 1,
            addresses: vec![ProviderEgressAddress {
                id: "loopback".into(),
                address: Ipv6Addr::LOCALHOST,
                enabled: true,
            }],
            ..ProviderEgressConfig::default()
        })));
        let server = MockServer::builder()
            .listener(std::net::TcpListener::bind("[::1]:0").unwrap())
            .start()
            .await;
        let mut config = valid_config();
        config.config.api.base_url = server.uri();
        let tuning = RequestTuningHandle::default();
        tuning.publish_openai_turn_state_policy(OpenAiTurnStatePolicy::new(
            true,
            ["model-a", "model-b"]
                .map(|model| UpstreamModelId::new(model).unwrap())
                .into(),
        ));
        let mut bundle =
            provider_openai::initialize_with_request_tuning(config.config.clone(), ports, tuning)
                .await
                .unwrap();
        let cancellation = CancellationToken::new();
        let mut discovery = None;
        let mut cycle = None;
        let mut worker = None;
        for contribution in bundle.take_worker_contributions() {
            if let WorkerContribution::Registration(registration) = contribution {
                match registration.runnable {
                    WorkerRunnable::Scheduled { task, .. }
                        if registration.id.owner() == "openai-turn-state" =>
                    {
                        cycle = Some(WorkerCycleContext::new(
                            registration.id,
                            None,
                            cancellation.clone(),
                        ));
                        discovery = Some(task);
                    }
                    WorkerRunnable::Daemon { task, .. }
                        if registration.id.owner() == "openai-turn-state-collector" =>
                    {
                        let token = cancellation.clone();
                        worker = Some(tokio::spawn(async move { task.run(token).await }));
                    }
                    _ => {}
                }
            }
        }
        Self {
            accounts,
            states,
            leases,
            server,
            cancellation,
            discovery: discovery.unwrap(),
            cycle: cycle.unwrap(),
            worker: worker.unwrap(),
        }
    }

    async fn discover(&self) {
        self.discovery.run_cycle(self.cycle.clone()).await.unwrap();
    }

    async fn stop(self) {
        self.cancellation.cancel();
        tokio::time::timeout(Duration::from_secs(2), self.worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(self.leases.reads.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn authentication_rejection_stops_all_models_and_recovery_resumes_discovery() {
    for (body, reason) in [
        (
            serde_json::json!({"error":{"code":"token_expired","message":"Token expired"}}),
            AccountErrorReason::AccessTokenExpired,
        ),
        (
            serde_json::json!({"error":{"code":"token_invalidated","message":"Token revoked"}}),
            AccountErrorReason::CredentialExpired,
        ),
    ] {
        let fixture = Fixture::new().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_json(body))
            .mount(&fixture.server)
            .await;
        fixture.discover().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let account = fixture.accounts.account("acct_recovery_probe").unwrap();
                if account.credential_state() == CredentialState::Expired {
                    assert_eq!(account.last_error_reason(), Some(reason));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        wait_count(&fixture.states.cancelled, 1).await;
        // A second model can be rejected before it starts, or by the one-second watcher.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let count = fixture.server.received_requests().await.unwrap().len();
        fixture.discover().await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            fixture.server.received_requests().await.unwrap().len(),
            count
        );
        assert_eq!(fixture.states.writes.load(Ordering::SeqCst), 0);

        fixture.server.reset().await;
        Mock::given(method("POST"))
            .respond_with(BudgetResponder {
                calls: Arc::default(),
                succeed: true,
                repeat_first: false,
                start_only: false,
                times: Arc::default(),
            })
            .mount(&fixture.server)
            .await;
        let account = fixture.accounts.account("acct_recovery_probe").unwrap();
        let repository = fixture.accounts.repository();
        let mut data = repository.load_complete_data(&account).await.unwrap();
        data.oauth_mut().unwrap().access_token = "synthetic-restored".into();
        repository
            .compare_and_swap_data(&account, data)
            .await
            .unwrap();
        let account = fixture.accounts.account("acct_recovery_probe").unwrap();
        repository
            .apply_state(&account, CredentialState::Ready, SystemTime::now())
            .await
            .unwrap();
        fixture.discover().await;
        wait_count(&fixture.states.writes, 4).await;
        fixture.stop().await;
    }
}

#[tokio::test]
async fn invalid_accounts_are_not_probed_even_if_switches_are_enabled() {
    let fixture = Fixture::new().await;
    for state in [
        CredentialState::Expired,
        CredentialState::Invalid,
        CredentialState::Banned,
    ] {
        let account = fixture.accounts.account("acct_recovery_probe").unwrap();
        fixture
            .accounts
            .repository()
            .apply_state(&account, state, SystemTime::now())
            .await
            .unwrap();
        fixture.discover().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(fixture.server.received_requests().await.unwrap().is_empty());
    }
    fixture.stop().await;
}

#[tokio::test]
async fn late_authentication_rejection_cannot_invalidate_replaced_credentials() {
    let fixture = Fixture::new().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            seen.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({"error":{"code":"token_invalidated"}}))
                .set_delay(Duration::from_millis(750))
        })
        .mount(&fixture.server)
        .await;
    fixture.discover().await;
    wait_count(&calls, 2).await;
    let account = fixture.accounts.account("acct_recovery_probe").unwrap();
    let repository = fixture.accounts.repository();
    let mut data = repository.load_complete_data(&account).await.unwrap();
    data.oauth_mut().unwrap().access_token = "synthetic-new-generation".into();
    repository
        .compare_and_swap_data(&account, data)
        .await
        .unwrap();
    wait_count(&fixture.states.cancelled, 2).await;
    let current = fixture.accounts.account("acct_recovery_probe").unwrap();
    assert_ne!(current.revision(), account.revision());
    assert_eq!(current.credential_state(), CredentialState::Ready);
    assert_eq!(current.last_error_reason(), None);
    assert_eq!(fixture.states.writes.load(Ordering::SeqCst), 0);
    fixture.stop().await;
}

#[tokio::test]
async fn authentication_rejection_is_account_scoped_and_other_accounts_keep_collecting() {
    let fixture = Fixture::new().await;
    let mut imported = CodexCredentialAdmin
        .prepare_import(ImportCodexOAuthCredential {
            account_id: "acct_healthy_probe".into(),
            name: "Healthy maintenance fixture".into(),
            secret: secret("synthetic-healthy"),
            verified_account: profile("synthetic-healthy-owner"),
            next_refresh_at: None,
            enabled: true,
        })
        .unwrap();
    imported.account = imported.account.with_turn_state_injection_enabled(true);
    fixture.accounts.create_account(imported).await.unwrap();
    Mock::given(method("POST"))
        .and(wiremock::matchers::header(
            "chatgpt-account-id",
            "synthetic-recovery-owner",
        ))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({"error":{"code":"token_invalidated"}})),
        )
        .mount(&fixture.server)
        .await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::header(
            "chatgpt-account-id",
            "synthetic-healthy-owner",
        ))
        .respond_with(BudgetResponder {
            calls: Arc::default(),
            succeed: true,
            repeat_first: false,
            start_only: false,
            times: Arc::default(),
        })
        .mount(&fixture.server)
        .await;
    fixture.discover().await;
    wait_count(&fixture.states.writes, 4).await;
    wait_count(&fixture.states.cancelled, 3).await;
    assert_eq!(
        fixture
            .accounts
            .account("acct_recovery_probe")
            .unwrap()
            .credential_state(),
        CredentialState::Expired
    );
    assert_eq!(
        fixture
            .accounts
            .account("acct_healthy_probe")
            .unwrap()
            .credential_state(),
        CredentialState::Ready
    );
    for ((account, _), record) in fixture.states.records.lock().unwrap().iter() {
        assert_eq!(account.as_str(), "acct_healthy_probe");
        assert!(record.active().is_some() && record.standby().is_some());
    }
    fixture.stop().await;
}

#[tokio::test]
async fn transient_errors_keep_credentials_ready_and_do_not_become_authentication_failures() {
    for status in [400, 403, 429, 503] {
        let fixture = Fixture::new().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_json(serde_json::json!({"error":{"code":"synthetic_error"}})),
            )
            .mount(&fixture.server)
            .await;
        fixture.discover().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while fixture.server.received_requests().await.unwrap().len() < 4 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let account = fixture.accounts.account("acct_recovery_probe").unwrap();
        assert_eq!(account.credential_state(), CredentialState::Ready);
        assert_eq!(account.last_error_reason(), None);
        assert_eq!(fixture.states.writes.load(Ordering::SeqCst), 0);
        fixture.stop().await;
    }
}
