use std::{
    collections::BTreeMap,
    net::Ipv6Addr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use futures::future::BoxFuture;
use gateway_core::{
    account::{AccountRuntimeSignals, CredentialRevision, ProviderAccountId, ProviderAccountStore},
    lifecycle::CancellationToken,
    policy::ClientApiKeyId,
    provider_ports::{
        OpaqueTurnState, ProviderLeaseAcquisition, ProviderLeasePort, ProviderLeaseRequest,
        ProviderSchedulingState, ProviderStoreError, ProviderStorePorts, ProviderTurnStateAnomaly,
        ProviderTurnStateCandidate, ProviderTurnStatePort, ProviderTurnStatePromotion,
        ProviderTurnStateRecord, ProviderTurnStateRefreshStatus, ProviderTurnStateSlot,
        ProviderTurnStateValue,
        egress::{ProviderEgressAddress, ProviderEgressConfig, ProviderEgressStorePort},
    },
    routing::{OpenAiTurnStatePolicy, ProviderKind, UpstreamModelId},
    runtime::RequestTuningHandle,
    task::{WorkerContribution, WorkerCycleContext, WorkerRunnable},
};
use provider_openai::credential::{CodexCredentialAdmin, ImportCodexOAuthCredential};
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{method, path},
};

use crate::{
    admin::{provider_ports_with_accounts, valid_config},
    support::{MemoryAccountStore, profile, secret},
};

#[derive(Default)]
struct ReadOnlyLeases {
    reads: AtomicUsize,
    busy: AtomicUsize,
}

impl ProviderLeasePort for ReadOnlyLeases {
    fn load_signals<'a>(
        &'a self,
        _: &'a ProviderKind,
        accounts: &'a [ProviderAccountId],
    ) -> BoxFuture<'a, Result<BTreeMap<ProviderAccountId, AccountRuntimeSignals>, ProviderStoreError>>
    {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(accounts
                .iter()
                .map(|id| {
                    (
                        id.clone(),
                        AccountRuntimeSignals {
                            in_flight: u32::try_from(self.busy.load(Ordering::SeqCst)).unwrap(),
                            last_started_at: None,
                            quota_reset_at: None,
                            quota_remaining_rank: None,
                            rate_limited_until: None,
                            failure_rate_basis_points: None,
                            first_output_latency_ms: None,
                        },
                    )
                })
                .collect())
        })
    }

    fn load_state<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a ProviderKind,
        _: &'a [ProviderAccountId],
    ) -> BoxFuture<'a, Result<ProviderSchedulingState, ProviderStoreError>> {
        Box::pin(async { panic!("maintenance must not enter business scheduling") })
    }

    fn try_acquire(
        &self,
        _: ProviderLeaseRequest,
    ) -> BoxFuture<'_, Result<ProviderLeaseAcquisition, ProviderStoreError>> {
        Box::pin(async { panic!("maintenance must not acquire business leases") })
    }
}

struct LoopbackEgress(ProviderEgressConfig);

impl ProviderEgressStorePort for LoopbackEgress {
    fn load(&self) -> BoxFuture<'_, Result<Arc<ProviderEgressConfig>, ProviderStoreError>> {
        Box::pin(async { Ok(Arc::new(self.0.clone())) })
    }

    fn ensure_fixed_affinity(
        &self,
        _: &ProviderAccountId,
    ) -> BoxFuture<'_, Result<Option<Ipv6Addr>, ProviderStoreError>> {
        Box::pin(async { panic!("probe must not write account affinity") })
    }
}

#[derive(Default)]
pub(super) struct ProbeStates {
    pub(super) records:
        Mutex<BTreeMap<(ProviderAccountId, UpstreamModelId), ProviderTurnStateRecord>>,
    writes: AtomicUsize,
    failed: AtomicUsize,
    cancelled: AtomicUsize,
    promoted: AtomicUsize,
}

impl ProviderTurnStatePort for ProbeStates {
    fn promote_standby(
        &self,
        promotion: ProviderTurnStatePromotion,
    ) -> BoxFuture<'_, Result<Option<ProviderTurnStateRecord>, ProviderStoreError>> {
        Box::pin(async move {
            let mut records = self.records.lock().unwrap();
            let key = (promotion.account_id, promotion.upstream_model);
            let Some(current) = records.get(&key) else {
                return Ok(None);
            };
            let Some(standby) = current.standby() else {
                return Ok(None);
            };
            if current.state_version() != promotion.expected_active_version
                || !standby.is_valid_at(promotion.observed_at)
                || standby.expires_at() <= promotion.observed_at + promotion.minimum_remaining
            {
                return Ok(None);
            }
            let next = ProviderTurnStateRecord::new(
                key.0.clone(),
                key.1.clone(),
                current.normal_length(),
                Some(standby.clone()),
                None,
                current.state_version() + 1,
                ProviderTurnStateRefreshStatus::Refreshing,
                None,
            );
            records.insert(key, next.clone());
            self.promoted.fetch_add(1, Ordering::SeqCst);
            Ok(Some(next))
        })
    }

    fn read<'a>(
        &'a self,
        account: &'a ProviderAccountId,
        model: &'a UpstreamModelId,
        _: CredentialRevision,
    ) -> BoxFuture<'a, Result<Option<ProviderTurnStateRecord>, ProviderStoreError>> {
        Box::pin(async move {
            Ok(self
                .records
                .lock()
                .unwrap()
                .get(&(account.clone(), model.clone()))
                .cloned())
        })
    }

    fn cancel_refresh<'a>(
        &'a self,
        _: &'a ProviderAccountId,
        _: &'a UpstreamModelId,
        _: CredentialRevision,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn put_candidate(
        &self,
        candidate: ProviderTurnStateCandidate,
    ) -> BoxFuture<'_, Result<ProviderTurnStateRecord, ProviderStoreError>> {
        Box::pin(async move {
            self.writes.fetch_add(1, Ordering::SeqCst);
            let key = (
                candidate.account_id.clone(),
                candidate.upstream_model.clone(),
            );
            let mut records = self.records.lock().unwrap();
            let previous = records.get(&key);
            let (active, standby, status) = match candidate.slot {
                ProviderTurnStateSlot::Active => (
                    Some(candidate.value),
                    None,
                    ProviderTurnStateRefreshStatus::Refreshing,
                ),
                ProviderTurnStateSlot::Standby => (
                    previous.and_then(|r| r.active().cloned()),
                    Some(candidate.value),
                    ProviderTurnStateRefreshStatus::Ready,
                ),
            };
            let version = previous.map_or(0, ProviderTurnStateRecord::state_version)
                + u64::from(candidate.slot == ProviderTurnStateSlot::Active);
            let record = ProviderTurnStateRecord::new(
                candidate.account_id,
                candidate.upstream_model,
                candidate.normal_length,
                active,
                standby,
                version,
                status,
                Some(candidate.normal_length),
            );
            records.insert(key, record.clone());
            Ok(record)
        })
    }

    fn record_anomaly(
        &self,
        _: ProviderTurnStateAnomaly,
    ) -> BoxFuture<'_, Result<ProviderTurnStateRecord, ProviderStoreError>> {
        Box::pin(async { panic!("unused anomaly path") })
    }

    fn mark_refresh_status<'a>(
        &'a self,
        account: &'a ProviderAccountId,
        model: &'a UpstreamModelId,
        _: CredentialRevision,
        normal_length: u16,
        status: ProviderTurnStateRefreshStatus,
        _: SystemTime,
    ) -> BoxFuture<'a, Result<ProviderTurnStateRecord, ProviderStoreError>> {
        Box::pin(async move {
            if status == ProviderTurnStateRefreshStatus::Failed {
                self.failed.fetch_add(1, Ordering::SeqCst);
            }
            Ok(self
                .records
                .lock()
                .unwrap()
                .get(&(account.clone(), model.clone()))
                .cloned()
                .unwrap_or_else(|| {
                    ProviderTurnStateRecord::new(
                        account.clone(),
                        model.clone(),
                        normal_length,
                        None,
                        None,
                        0,
                        status,
                        None,
                    )
                }))
        })
    }
}

struct ProbeResponder(Arc<AtomicUsize>);

impl Respond for ProbeResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        assert!(
            !request.headers.contains_key("x-codex-turn-state"),
            "probes must be uninjected"
        );
        let first = self.0.fetch_add(1, Ordering::SeqCst) == 0;
        let mut bytes = vec![0x80];
        bytes.extend_from_slice(
            &SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .to_be_bytes(),
        );
        bytes.extend(std::iter::repeat_n(0x22, 16 + 160 + 32));
        let response = ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .insert_header("x-codex-turn-state", URL_SAFE.encode(bytes))
            .set_body_string("event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_probe\",\"model\":\"model-a\",\"status\":\"completed\",\"output\":[]}}\n\n");
        if first {
            response.set_delay(Duration::from_secs(20))
        } else {
            response
        }
    }
}

async fn wait_count(counter: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while counter.load(Ordering::SeqCst) < expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("maintenance progress");
}

#[tokio::test]
async fn turn_state_worker_is_read_only_to_scheduler_and_switch_cancels_inflight_probe() {
    collector_cancellation(false).await;
}

#[tokio::test]
async fn hard_credential_rotation_cancels_inflight_probe_and_restarts_with_new_binding() {
    collector_cancellation(true).await;
}

async fn collector_cancellation(hard_rotation: bool) {
    let accounts = Arc::new(MemoryAccountStore::default());
    let mut imported = CodexCredentialAdmin
        .prepare_import(ImportCodexOAuthCredential {
            account_id: "acct_probe".to_owned(),
            name: "Probe test".to_owned(),
            secret: secret("synthetic-probe"),
            verified_account: profile("synthetic-probe-owner"),
            next_refresh_at: None,
            enabled: true,
        })
        .unwrap();
    imported.account = imported.account.with_turn_state_injection_enabled(true);
    let account_id = imported.account.id().clone();
    accounts.create_account(imported).await.unwrap();
    let base = provider_ports_with_accounts(accounts.clone());
    let leases = Arc::new(ReadOnlyLeases::default());
    let states = Arc::new(ProbeStates::default());
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
            id: "loopback".to_owned(),
            address: Ipv6Addr::LOCALHOST,
            enabled: true,
        }],
        account_overrides: [(account_id.clone(), None)].into(),
        ..ProviderEgressConfig::default()
    })));
    let listener = std::net::TcpListener::bind("[::1]:0").expect("IPv6 loopback");
    let server = MockServer::builder().listener(listener).start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ProbeResponder(calls.clone()))
        .expect(2..)
        .mount(&server)
        .await;
    let tuning = RequestTuningHandle::default();
    let policy =
        OpenAiTurnStatePolicy::new(true, [UpstreamModelId::new("model-a").unwrap()].into());
    let mut config = valid_config();
    config.config.api.base_url = server.uri();
    let mut bundle = provider_openai::initialize_with_request_tuning(
        config.config.clone(),
        ports,
        tuning.clone(),
    )
    .await
    .unwrap();
    let mut discover = None;
    let mut collector = None;
    for contribution in bundle.take_worker_contributions() {
        if let WorkerContribution::Registration(registration) = contribution {
            match registration.runnable {
                WorkerRunnable::Scheduled { task, .. }
                    if registration.id.owner() == "openai-turn-state" =>
                {
                    discover = Some((registration.id, task));
                }
                WorkerRunnable::Daemon { task, .. }
                    if registration.id.owner() == "openai-turn-state-collector" =>
                {
                    collector = Some(task);
                }
                _ => {}
            }
        }
    }
    let (id, discover) = discover.unwrap();
    let cancellation = CancellationToken::new();
    let cycle = WorkerCycleContext::new(id, None, cancellation.clone());
    let run_cancel = cancellation.clone();
    let collector = collector.unwrap();
    let handle = tokio::spawn(async move { collector.run(run_cancel).await });
    discover.run_cycle(cycle.clone()).await.unwrap();
    assert_eq!(
        leases.reads.load(Ordering::SeqCst),
        0,
        "global off does no acquisition"
    );
    tuning.publish_openai_turn_state_policy(policy.clone());
    leases.busy.store(1, Ordering::SeqCst);
    discover.run_cycle(cycle.clone()).await.unwrap();
    wait_count(&calls, 1).await;
    assert_eq!(
        leases.reads.load(Ordering::SeqCst),
        0,
        "collection is independent of business leases"
    );
    if hard_rotation {
        let account = accounts.get_account(&account_id).await.unwrap().unwrap();
        let mut data = accounts
            .repository()
            .load_complete_data(&account)
            .await
            .unwrap();
        data.oauth_mut().unwrap().access_token = "synthetic-replacement".into();
        accounts
            .repository()
            .compare_and_swap_data(&account, data)
            .await
            .unwrap();
    } else {
        tuning.publish_openai_turn_state_policy(OpenAiTurnStatePolicy::default());
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(states.writes.load(Ordering::SeqCst), 0);
    assert_eq!(states.cancelled.load(Ordering::SeqCst), 1);
    tuning.publish_openai_turn_state_policy(policy);
    discover.run_cycle(cycle).await.unwrap();
    // Cancellation released the same-key collector before another generation can start.
    wait_count(&calls, 2).await;
    wait_count(&states.writes, 1).await;
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[derive(Clone)]
struct BudgetResponder {
    calls: Arc<Mutex<BTreeMap<(String, String), usize>>>,
    succeed: bool,
    repeat_first: bool,
    start_only: bool,
    times: Arc<Mutex<Vec<(usize, std::time::Instant)>>>,
}

impl Respond for BudgetResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        assert!(!request.headers.contains_key("x-codex-turn-state"));
        let body = if request
            .headers
            .get("content-encoding")
            .is_some_and(|v| v == "zstd")
        {
            zstd::stream::decode_all(request.body.as_slice()).unwrap()
        } else {
            request.body.clone()
        };
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let model = body["model"].as_str().unwrap().to_owned();
        let owner = request
            .headers
            .get("chatgpt-account-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let count = {
            let mut calls = self.calls.lock().unwrap();
            if !self.succeed
                && !self.start_only
                && calls.get(&(owner.clone(), model.clone())) == Some(&1)
            {
                assert_eq!(
                    calls.len(),
                    4,
                    "all keys must start before any key completes its first round"
                );
            }
            let count = calls.entry((owner, model)).or_default();
            *count += 1;
            *count
        };
        self.times
            .lock()
            .unwrap()
            .push((count, std::time::Instant::now()));
        if !self.succeed {
            let response = match count % 6 {
                0 => ResponseTemplate::new(429),
                1 => ResponseTemplate::new(500),
                2 => ResponseTemplate::new(200),
                3 => {
                    ResponseTemplate::new(200).insert_header("x-codex-turn-state", "x".repeat(292))
                }
                4 => ResponseTemplate::new(200)
                    .insert_header("x-codex-turn-state", format!("gAAAAA{}", "x".repeat(306))),
                _ => ResponseTemplate::new(204)
                    .insert_header("x-codex-turn-state", format!("gAAAAA{}", "x".repeat(286))),
            }
            .set_body_json(serde_json::json!({"error":{"code":"synthetic_failure"}}));
            return if count == 1 {
                response.set_delay(if self.start_only {
                    Duration::from_secs(30)
                } else {
                    Duration::from_millis(400)
                })
            } else {
                response
            };
        }
        let value = if self.repeat_first && count == 2 {
            1
        } else {
            count
        };
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .insert_header("x-codex-turn-state", format!("gAAAAA{value:0286}"))
            .set_body_string("event: response.failed\ndata: {\"type\":\"response.failed\"}\n\n")
    }
}

#[derive(Clone, Copy)]
enum AcquisitionScenario {
    ContinuousMisses,
    ImmediateStandby,
    RepeatedStandby,
    ConcurrentStarts,
    PromoteStandby,
    SoftCookieRefresh,
}

async fn independent_collectors(scenario: AcquisitionScenario) {
    let succeed = matches!(
        scenario,
        AcquisitionScenario::ImmediateStandby
            | AcquisitionScenario::RepeatedStandby
            | AcquisitionScenario::PromoteStandby
            | AcquisitionScenario::SoftCookieRefresh
    );
    let soft_cookie = matches!(scenario, AcquisitionScenario::SoftCookieRefresh);
    let repeat_first = matches!(scenario, AcquisitionScenario::RepeatedStandby) || soft_cookie;
    let start_only = matches!(scenario, AcquisitionScenario::ConcurrentStarts);
    let promote = matches!(scenario, AcquisitionScenario::PromoteStandby);
    let accounts = Arc::new(MemoryAccountStore::default());
    let account_count = if start_only { 20 } else { 2 };
    let key_count = account_count * 2;
    for index in 0..account_count {
        let mut imported = CodexCredentialAdmin
            .prepare_import(ImportCodexOAuthCredential {
                account_id: format!("acct_parallel_{index}"),
                name: format!("Parallel {index}"),
                secret: secret(&format!("parallel-{index}")),
                verified_account: profile(&format!("parallel-owner-{index}")),
                next_refresh_at: None,
                enabled: true,
            })
            .unwrap();
        imported.account = imported.account.with_turn_state_injection_enabled(true);
        accounts.create_account(imported).await.unwrap();
    }
    let base = provider_ports_with_accounts(accounts.clone());
    let states = Arc::new(ProbeStates::default());
    let captured = SystemTime::now() - Duration::from_secs(1200);
    if promote {
        for index in 0..account_count {
            for model in ["model-a", "model-b"] {
                let id = ProviderAccountId::new(format!("acct_parallel_{index}")).unwrap();
                let model = UpstreamModelId::new(model).unwrap();
                let state = |ch: char, captured: SystemTime| {
                    ProviderTurnStateValue::new(
                        OpaqueTurnState::new(format!("gAAAAA{}", ch.to_string().repeat(286))),
                        captured,
                        captured + Duration::from_secs(3600),
                    )
                };
                states.records.lock().unwrap().insert(
                    (id.clone(), model.clone()),
                    ProviderTurnStateRecord::new(
                        id,
                        model,
                        292,
                        Some(state('a', captured - Duration::from_secs(1900))),
                        Some(state('b', captured)),
                        1,
                        ProviderTurnStateRefreshStatus::Ready,
                        None,
                    ),
                );
            }
        }
    }
    let ports = ProviderStorePorts::new(
        base.accounts(),
        Arc::new(ReadOnlyLeases::default()),
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
            id: "loopback".to_owned(),
            address: Ipv6Addr::LOCALHOST,
            enabled: true,
        }],
        ..ProviderEgressConfig::default()
    })));
    let server = MockServer::builder()
        .listener(std::net::TcpListener::bind("[::1]:0").unwrap())
        .start()
        .await;
    let responder = BudgetResponder {
        calls: Arc::default(),
        succeed,
        repeat_first,
        start_only,
        times: Arc::default(),
    };
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(responder.clone())
        .expect(if promote {
            4..=4
        } else if start_only {
            40..=40
        } else if !succeed {
            2044..=u64::MAX
        } else if repeat_first {
            12..=12
        } else {
            8..=8
        })
        .mount(&server)
        .await;
    let tuning = RequestTuningHandle::default();
    tuning.publish_openai_turn_state_policy(OpenAiTurnStatePolicy::new(
        true,
        ["model-a", "model-b"]
            .into_iter()
            .map(|model| UpstreamModelId::new(model).unwrap())
            .collect(),
    ));
    let mut config = valid_config();
    config.config.api.base_url = server.uri();
    let mut bundle = provider_openai::initialize_with_request_tuning(config.config, ports, tuning)
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let mut discover = None;
    let mut collector = None;
    for contribution in bundle.take_worker_contributions() {
        if let WorkerContribution::Registration(registration) = contribution {
            match registration.runnable {
                WorkerRunnable::Scheduled { task, .. }
                    if registration.id.owner() == "openai-turn-state" =>
                {
                    discover = Some((registration.id, task))
                }
                WorkerRunnable::Daemon { task, .. }
                    if registration.id.owner() == "openai-turn-state-collector" =>
                {
                    collector = Some(task)
                }
                _ => {}
            }
        }
    }
    let runner_cancel = cancel.clone();
    let handle = tokio::spawn(async move { collector.unwrap().run(runner_cancel).await });
    let (id, task) = discover.unwrap();
    task.run_cycle(WorkerCycleContext::new(id, None, cancel.clone()))
        .await
        .unwrap();
    if soft_cookie {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let ready = {
                    let calls = responder.calls.lock().unwrap();
                    calls.len() == key_count && calls.values().all(|count| *count >= 2)
                };
                if ready {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        for index in 0..account_count {
            let id = format!("acct_parallel_{index}");
            let account = accounts.account(&id).unwrap();
            let mut data = accounts
                .repository()
                .load_complete_data(&account)
                .await
                .unwrap();
            data.cookies_mut()
                .push(provider_openai::credential::CodexCookie {
                    name: "__cf_bm".into(),
                    value: "synthetic-fresh".into(),
                    domain: "::1".into(),
                    path: "/".into(),
                    host_only: true,
                    secure: false,
                    expires_at: None,
                });
            accounts
                .repository()
                .compare_and_swap_data(&account, data)
                .await
                .unwrap();
        }
    }
    let completed = tokio::time::timeout(
        Duration::from_secs(if start_only { 20 } else { 300 }),
        async {
            while if start_only {
                responder.calls.lock().unwrap().len() < key_count
            } else if succeed {
                states.writes.load(Ordering::SeqCst) < if promote { 4 } else { 8 }
            } else {
                let calls = responder.calls.lock().unwrap();
                calls.len() < 4 || calls.values().any(|count| *count < 511)
            } {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        },
    )
    .await;
    if soft_cookie && completed.is_ok() {
        wait_count(&states.cancelled, key_count).await;
    }
    cancel.cancel();
    handle.await.unwrap().unwrap();
    let calls = responder.calls.lock().unwrap().clone();
    assert!(
        completed.is_ok(),
        "independent tasks timed out: calls={calls:?}, failed={}, cancelled={}",
        states.failed.load(Ordering::SeqCst),
        states.cancelled.load(Ordering::SeqCst),
    );
    assert_eq!(
        calls.len(),
        key_count,
        "each account and model owns its own task"
    );
    assert!(calls.values().all(|count| if start_only || promote {
        *count == 1
    } else if !succeed {
        *count >= 511
    } else if repeat_first {
        *count == 3
    } else {
        *count == 2
    }));
    assert_eq!(
        states.failed.load(Ordering::SeqCst),
        0,
        "ordinary misses do not exhaust a task"
    );
    if repeat_first {
        let times = responder.times.lock().unwrap();
        let latest_second = times
            .iter()
            .filter(|(n, _)| *n == 2)
            .map(|(_, at)| *at)
            .max()
            .unwrap();
        let earliest_third = times
            .iter()
            .filter(|(n, _)| *n == 3)
            .map(|(_, at)| *at)
            .min()
            .unwrap();
        assert!(
            earliest_third.duration_since(latest_second) >= Duration::from_secs(5),
            "standby misses must not use the urgent retry loop"
        );
    }
    if succeed {
        let records = states.records.lock().unwrap();
        assert_eq!(records.len(), 4);
        for record in records.values() {
            if promote {
                assert_eq!(record.active().unwrap().issued_at(), captured);
                assert_eq!(
                    record.active().unwrap().expires_at(),
                    captured + Duration::from_secs(3600)
                );
                assert_eq!(record.state_version(), 2);
            }
            assert_eq!(
                record.refresh_status(),
                ProviderTurnStateRefreshStatus::Ready
            );
            assert_ne!(
                record.active().unwrap().state(),
                record.standby().unwrap().state()
            );
        }
    }
    if promote {
        assert_eq!(states.promoted.load(Ordering::SeqCst), 4);
    }
    if soft_cookie {
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| {
                    request
                        .headers
                        .get("cookie")
                        .is_some_and(|value| value == "__cf_bm=synthetic-fresh")
                })
                .count(),
            4,
            "the next batch must reload each account's Cookie material"
        );
        assert_eq!(
            states.cancelled.load(Ordering::SeqCst),
            4,
            "only completed tasks are cleaned up, not cancelled/restarted by soft saves"
        );
    }
}

#[tokio::test]
async fn soft_cookie_updates_keep_collectors_alive_and_reload_next_batch_material() {
    independent_collectors(AcquisitionScenario::SoftCookieRefresh).await;
}

#[tokio::test]
async fn independent_accounts_and_models_continue_past_500_with_one_ipv6_and_upstream_errors() {
    independent_collectors(AcquisitionScenario::ContinuousMisses).await;
}

#[tokio::test]
async fn independent_accounts_and_models_immediately_acquire_distinct_standby() {
    independent_collectors(AcquisitionScenario::ImmediateStandby).await;
}

#[tokio::test]
async fn standby_repeats_are_rejected_and_retried_on_the_background_interval() {
    independent_collectors(AcquisitionScenario::RepeatedStandby).await;
}

#[tokio::test]
async fn more_than_32_account_model_collectors_start_without_waiting_for_other_keys() {
    independent_collectors(AcquisitionScenario::ConcurrentStarts).await;
}

#[tokio::test]
async fn expiring_active_promotes_standby_without_renewal_then_immediately_refills() {
    independent_collectors(AcquisitionScenario::PromoteStandby).await;
}
