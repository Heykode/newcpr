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
    account::{
        AccountErrorReason, AccountRuntimeSignals, CredentialRevision, CredentialState,
        ProviderAccountId, ProviderAccountStore,
    },
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
    task::{
        ScheduledTask, WorkerContribution, WorkerCycleContext, WorkerRunnable, WorkerTaskError,
    },
};
use provider_openai::credential::{CodexCredentialAdmin, ImportCodexOAuthCredential};
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{method, path},
};

use crate::{
    admin::{provider_ports_with_accounts, valid_config},
    support::{MemoryAccountStore, MemoryCooldownPort, profile, secret},
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
    progress: Mutex<BTreeMap<(ProviderAccountId, UpstreamModelId), ProbeProgress>>,
}

type ProbeProgress = (u64, Option<&'static str>, Option<u64>);

impl ProviderTurnStatePort for ProbeStates {
    fn record_probe_progress<'a>(
        &'a self,
        account: &'a ProviderAccountId,
        model: &'a UpstreamModelId,
        _: CredentialRevision,
        attempts: u64,
        reason: Option<&'static str>,
        successful_attempt: Option<u64>,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            self.progress.lock().unwrap().insert(
                (account.clone(), model.clone()),
                (attempts, reason, successful_attempt),
            );
            Ok(())
        })
    }

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
                || current.active().is_some_and(|active| {
                    active.expires_at() > promotion.observed_at + promotion.minimum_remaining
                        || standby.expires_at() <= active.expires_at()
                })
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
                ProviderTurnStateRefreshStatus::Ready,
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
            if candidate.expected_active_version.is_some_and(|expected| {
                previous.map_or(0, ProviderTurnStateRecord::state_version) != expected
            }) {
                return Ok(previous.unwrap().clone());
            }
            let (active, standby, status) = match candidate.slot {
                ProviderTurnStateSlot::Active => (
                    Some(candidate.value),
                    None,
                    ProviderTurnStateRefreshStatus::Ready,
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
            let mut records = self.records.lock().unwrap();
            let key = (account.clone(), model.clone());
            let old = records.get(&key);
            let record = ProviderTurnStateRecord::new(
                account.clone(),
                model.clone(),
                normal_length,
                old.and_then(|old| old.active().cloned()),
                old.and_then(|old| old.standby().cloned()),
                old.map_or(0, ProviderTurnStateRecord::state_version),
                status,
                None,
            );
            records.insert(key, record.clone());
            Ok(record)
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
        "collection ignores business concurrency and never acquires a lease"
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
    issued_at: SystemTime,
    succeed: bool,
    repeat_first: bool,
    start_only: bool,
    soft_cookie: bool,
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
            let count = calls.entry((owner, model.clone())).or_default();
            *count += 1;
            *count
        };
        self.times
            .lock()
            .unwrap()
            .push((count, std::time::Instant::now()));
        if !self.succeed || (self.soft_cookie && count == 1) {
            let response = match count % 6 {
                0 => ResponseTemplate::new(502),
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
                    Duration::from_millis(250)
                })
            } else {
                response
            };
        }
        let repeated = self.repeat_first && count <= 2;
        let value = if repeated { 1 } else { count };
        let response = ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .insert_header("x-codex-turn-state", state_at(value, if repeated {
                self.issued_at - Duration::from_secs(2800)
            } else { self.issued_at }))
            .set_body_string(format!(
                "event: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp_probe\",\"model\":\"{model}\",\"status\":\"completed\",\"output\":[]}}}}\n\n"
            ));
        if self.soft_cookie && count == 2 {
            response.set_delay(Duration::from_millis(250))
        } else {
            response
        }
    }
}

fn fresh_state(value: usize) -> String {
    state_at(value, SystemTime::now())
}

fn state_at(value: usize, issued_at: SystemTime) -> String {
    let mut bytes = vec![0x80];
    bytes.extend_from_slice(
        &issued_at
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_be_bytes(),
    );
    bytes.extend_from_slice(&value.to_be_bytes());
    bytes.resize(9 + 16 + 160 + 32, 0x22);
    URL_SAFE.encode(bytes)
}

fn loopback_sources(count: usize) -> Vec<ProviderEgressAddress> {
    // Synthetic duplicate addresses exercise batch budgets using only local sockets.
    (0..count)
        .map(|index| ProviderEgressAddress {
            id: format!("loopback-{index}"),
            address: Ipv6Addr::LOCALHOST,
            enabled: true,
        })
        .collect()
}

#[derive(Clone, Copy)]
enum AcquisitionScenario {
    ContinuousMisses,
    ImmediateStandby,
    RepeatedStandby,
    SerialStarts(usize),
    RefreshActive,
    SoftCookieRefresh,
}

async fn bounded_collectors(scenario: AcquisitionScenario) {
    let succeed = matches!(
        scenario,
        AcquisitionScenario::ImmediateStandby
            | AcquisitionScenario::RepeatedStandby
            | AcquisitionScenario::RefreshActive
            | AcquisitionScenario::SoftCookieRefresh
    );
    let soft_cookie = matches!(scenario, AcquisitionScenario::SoftCookieRefresh);
    let repeat_first = matches!(scenario, AcquisitionScenario::RepeatedStandby);
    let start_only = matches!(scenario, AcquisitionScenario::SerialStarts(_));
    let refresh = matches!(scenario, AcquisitionScenario::RefreshActive);
    let accounts = Arc::new(MemoryAccountStore::default());
    let account_count = match scenario {
        AcquisitionScenario::SerialStarts(count) => count,
        _ => 2,
    };
    let key_count = account_count * 2;
    let running_key_count = account_count.min(5) * 2;
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
    let issued_at = SystemTime::now();
    let captured = issued_at - Duration::from_secs(2800);
    if refresh || repeat_first {
        for index in 0..account_count {
            for model in ["model-a", "model-b"] {
                let id = ProviderAccountId::new(format!("acct_parallel_{index}")).unwrap();
                let model = UpstreamModelId::new(model).unwrap();
                let state = ProviderTurnStateValue::new(
                    OpaqueTurnState::new(state_at(1, captured)),
                    captured,
                    captured + Duration::from_secs(3600),
                );
                states.records.lock().unwrap().insert(
                    (id.clone(), model.clone()),
                    ProviderTurnStateRecord::new(
                        id,
                        model,
                        292,
                        Some(state),
                        None,
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
        addresses: loopback_sources(510),
        ..ProviderEgressConfig::default()
    })));
    let server = MockServer::builder()
        .listener(std::net::TcpListener::bind("[::1]:0").unwrap())
        .start()
        .await;
    let responder = BudgetResponder {
        calls: Arc::default(),
        issued_at,
        succeed,
        repeat_first,
        start_only,
        soft_cookie,
        times: Arc::default(),
    };
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(responder.clone())
        .expect(if start_only {
            running_key_count as u64..=running_key_count as u64
        } else if !succeed {
            2200..=usize::MAX as u64
        } else if repeat_first {
            12..=24
        } else if soft_cookie {
            8..=24
        } else {
            4..=4
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
        for index in 0..account_count {
            let owner = format!("parallel-owner-{index}");
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let seen = responder
                        .calls
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|((account, _), count)| account == &owner && *count >= 1);
                    if seen {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            // Real response-cookie allowlists exclude loopback origins. Simulate the
            // committed routine Cookie save without weakening production validation.
            let account = accounts.account(&format!("acct_parallel_{index}")).unwrap();
            let repository = accounts.repository();
            let mut data = repository.load_complete_data(&account).await.unwrap();
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
            repository
                .compare_and_swap_data(&account, data)
                .await
                .unwrap();
            let changed = accounts.account(&format!("acct_parallel_{index}")).unwrap();
            assert_ne!(changed.revision(), account.revision());
            assert_eq!(
                changed.turn_state_binding_revision(),
                account.turn_state_binding_revision()
            );
        }
    }
    // This verifies continued acquisition, not requests/second. Linux rebuilds
    // native TLS clients for all 2,200+ probes; the per-probe timeout is separate.
    let completion_budget = if matches!(scenario, AcquisitionScenario::ContinuousMisses) {
        Duration::from_secs(120)
    } else {
        Duration::from_secs(30)
    };
    let completed = tokio::time::timeout(completion_budget, async {
        while if start_only {
            responder.calls.lock().unwrap().len() < running_key_count
        } else if succeed {
            states.writes.load(Ordering::SeqCst) < key_count
        } else {
            let calls = responder.calls.lock().unwrap();
            calls.len() < key_count || calls.values().any(|count| *count < 550)
        } {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if start_only {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if soft_cookie && completed.is_ok() {
        wait_count(&states.cancelled, key_count).await;
    }
    cancel.cancel();
    handle.await.unwrap().unwrap();
    let calls = responder.calls.lock().unwrap().clone();
    assert!(
        completed.is_ok(),
        "bounded tasks timed out: calls={calls:?}, failed={}, cancelled={}",
        states.failed.load(Ordering::SeqCst),
        states.cancelled.load(Ordering::SeqCst),
    );
    assert_eq!(
        calls.len(),
        if start_only {
            running_key_count
        } else {
            key_count
        },
        "five accounts run at once, with both models concurrent"
    );
    assert!(calls.values().all(|count| if start_only {
        *count == 1
    } else if !succeed {
        *count >= 550
    } else if repeat_first {
        (3..=6).contains(count)
    } else if soft_cookie {
        (2..=6).contains(count)
    } else {
        *count == 1
    }));
    assert_eq!(
        states.failed.load(Ordering::SeqCst),
        0,
        "ordinary misses never terminate at a fixed request budget"
    );
    if repeat_first {
        let times = responder.times.lock().unwrap();
        let first_second = times
            .iter()
            .filter(|(n, _)| *n == 2)
            .map(|(_, at)| *at)
            .min()
            .unwrap();
        let earliest_third = times
            .iter()
            .filter(|(n, _)| *n == 3)
            .map(|(_, at)| *at)
            .min()
            .unwrap();
        assert!(
            earliest_third.duration_since(first_second) < Duration::from_secs(5),
            "standby retries use the restored escalating batches without six-second pacing"
        );
    }
    if succeed {
        let records = states.records.lock().unwrap();
        assert_eq!(records.len(), 4);
        for record in records.values() {
            if refresh || repeat_first {
                assert!(record.active().unwrap().issued_at() <= captured);
                assert!(record.standby().unwrap().issued_at() > captured);
                assert_eq!(record.state_version(), 1);
            } else {
                assert!(record.standby().is_none(), "no permanent spare acquisition");
            }
            assert_eq!(
                record.refresh_status(),
                ProviderTurnStateRefreshStatus::Ready
            );
        }
    }
    assert_eq!(states.promoted.load(Ordering::SeqCst), 0);
    if soft_cookie {
        let requests = server.received_requests().await.unwrap();
        assert!(
            requests
                .iter()
                .filter(|request| {
                    request
                        .headers
                        .get("cookie")
                        .is_some_and(|value| value == "__cf_bm=synthetic-fresh")
                })
                .count()
                >= 4,
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
    bounded_collectors(AcquisitionScenario::SoftCookieRefresh).await;
}

// Match the service runtime: native TLS client construction is synchronous on Linux.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_model_tasks_continue_past_500_with_reusable_sources() {
    bounded_collectors(AcquisitionScenario::ContinuousMisses).await;
}

#[tokio::test]
async fn parallel_account_model_tasks_only_acquire_active_initially() {
    bounded_collectors(AcquisitionScenario::ImmediateStandby).await;
}

#[tokio::test]
async fn standby_repeats_are_rejected_and_retried_with_escalating_batches() {
    bounded_collectors(AcquisitionScenario::RepeatedStandby).await;
}

#[tokio::test]
async fn first_five_accounts_run_all_models_while_later_accounts_wait() {
    for count in [1, 4, 5, 7] {
        bounded_collectors(AcquisitionScenario::SerialStarts(count)).await;
    }
}

#[tokio::test]
async fn fifteen_minute_refresh_keeps_active_until_switch_cutoff() {
    bounded_collectors(AcquisitionScenario::RefreshActive).await;
}

struct CredentialRecoveryFixture {
    admin: Arc<dyn gateway_admin::ports::provider::ProviderAdmin>,
    accounts: Arc<MemoryAccountStore>,
    states: Arc<ProbeStates>,
    leases: Arc<ReadOnlyLeases>,
    cooldowns: Arc<MemoryCooldownPort>,
    tuning: RequestTuningHandle,
    server: MockServer,
    discovery: Box<dyn ScheduledTask>,
    cycle: WorkerCycleContext,
    cancellation: CancellationToken,
    worker: tokio::task::JoinHandle<Result<(), WorkerTaskError>>,
}

impl CredentialRecoveryFixture {
    async fn new() -> Self {
        Self::with_probe_proxy(None).await
    }

    async fn with_probe_proxy(proxy: Option<gateway_core::account::OutboundProxy>) -> Self {
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
        let cooldowns = Arc::new(MemoryCooldownPort::new());
        let ports = ProviderStorePorts::new(
            base.accounts(),
            leases.clone(),
            base.session_affinity(),
            base.session_exclusions(),
            base.catalog_cache(),
            base.artifact_profiles(),
            base.credential_state(),
            cooldowns.clone(),
            base.runtime_policy(),
            base.oauth_pending(),
        )
        .with_turn_states(states.clone());
        let ports = if proxy.is_none() {
            ports.with_egress(Arc::new(LoopbackEgress(ProviderEgressConfig {
                revision: 1,
                addresses: loopback_sources(12),
                ..ProviderEgressConfig::default()
            })))
        } else {
            ports
        };
        let server = MockServer::builder()
            .listener(std::net::TcpListener::bind("[::1]:0").unwrap())
            .start()
            .await;
        let mut config = valid_config();
        config.config.api.base_url = server.uri();
        let tuning = RequestTuningHandle::default();
        tuning.publish_openai_turn_state_policy(
            OpenAiTurnStatePolicy::new(
                true,
                ["model-a", "model-b"]
                    .map(|model| UpstreamModelId::new(model).unwrap())
                    .into(),
            )
            .with_probe_proxy(proxy),
        );
        let mut bundle = provider_openai::initialize_with_request_tuning(
            config.config.clone(),
            ports,
            tuning.clone(),
        )
        .await
        .unwrap();
        let cancellation = CancellationToken::new();
        let admin = bundle.admin_provider();
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
            admin,
            accounts,
            states,
            leases,
            cooldowns,
            tuning,
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
        let fixture = CredentialRecoveryFixture::new().await;
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
                issued_at: SystemTime::now(),
                succeed: true,
                repeat_first: false,
                start_only: false,
                soft_cookie: false,
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
        fixture
            .admin
            .account_facts_changed(std::slice::from_ref(account.id()))
            .await;
        wait_count(&fixture.states.writes, 2).await;
        fixture.stop().await;
    }
}

#[tokio::test]
async fn probe_proxy_works_without_ipv6_and_closes_each_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = gateway_core::account::OutboundProxy::parse(&format!(
        "http://{}",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let fixture = CredentialRecoveryFixture::with_probe_proxy(Some(proxy)).await;
    let response_state = fresh_state(1);
    let server = tokio::spawn(async move {
        let mut sockets = Vec::new();
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0; 1];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
                assert!(request.len() < 65536);
            }
            let headers = String::from_utf8(request).unwrap().to_lowercase();
            assert!(headers.starts_with("post http://"));
            assert!(headers.contains("connection: close"));
            assert!(headers.contains("authorization: bearer "));
            assert!(headers.contains("user-agent: "));
            let body = "data: {\"type\":\"response.completed\"}\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nX-Codex-Turn-State: {response_state}\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            // Keep the peer socket open: the next probe must still open a new one.
            sockets.push(socket);
        }
    });
    fixture.discover().await;
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap();
    wait_count(&fixture.states.writes, 2).await;
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn changing_probe_proxy_discards_old_response_and_collects_via_new_proxy() {
    let old = MockServer::start().await;
    let new = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-turn-state", fresh_state(1))
                .set_body_string("data: {\"type\":\"response.completed\"}\n\n")
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&old)
        .await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-turn-state", fresh_state(2))
                .set_body_string("data: {\"type\":\"response.completed\"}\n\n"),
        )
        .mount(&new)
        .await;
    let fixture = CredentialRecoveryFixture::with_probe_proxy(Some(
        gateway_core::account::OutboundProxy::parse(&old.uri()).unwrap(),
    ))
    .await;
    fixture.discover().await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while old.received_requests().await.unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    fixture.tuning.publish_openai_turn_state_policy(
        OpenAiTurnStatePolicy::new(
            true,
            ["model-a", "model-b"]
                .map(|m| UpstreamModelId::new(m).unwrap())
                .into(),
        )
        .with_probe_proxy(Some(
            gateway_core::account::OutboundProxy::parse(&new.uri()).unwrap(),
        )),
    );
    wait_count(&fixture.states.writes, 2).await;
    assert_eq!(new.received_requests().await.unwrap().len(), 2);
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn proxy_failure_never_falls_back_to_direct_or_invalidates_credentials() {
    let proxy = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&proxy)
        .await;
    let fixture = CredentialRecoveryFixture::with_probe_proxy(Some(
        gateway_core::account::OutboundProxy::parse(&proxy.uri()).unwrap(),
    ))
    .await;
    fixture.discover().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while proxy.received_requests().await.unwrap().len() < 4 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.states.writes.load(Ordering::SeqCst), 0);
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
    let account = fixture
        .accounts
        .get_account(&ProviderAccountId::new("acct_recovery_probe").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(account.credential_state(), CredentialState::Ready);
    fixture.stop().await;
}

#[tokio::test]
async fn restored_binding_wakes_after_old_collectors_exit_without_another_discovery_cycle() {
    let fixture = CredentialRecoveryFixture::new().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&fixture.server)
        .await;
    fixture.discover().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.server.received_requests().await.unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let old = fixture.accounts.account("acct_recovery_probe").unwrap();
    let repository = fixture.accounts.repository();
    let mut data = repository.load_complete_data(&old).await.unwrap();
    data.oauth_mut().unwrap().access_token = "synthetic-restored-inflight".into();
    repository.compare_and_swap_data(&old, data).await.unwrap();
    let restored = fixture.accounts.account("acct_recovery_probe").unwrap();
    assert_ne!(
        restored.turn_state_binding_revision(),
        old.turn_state_binding_revision()
    );
    fixture.server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-turn-state", fresh_state(2))
                .set_body_string("data: {\"type\":\"response.completed\"}\n\n"),
        )
        .mount(&fixture.server)
        .await;
    fixture
        .admin
        .account_facts_changed(std::slice::from_ref(restored.id()))
        .await;
    wait_count(&fixture.states.writes, 2).await;
    fixture.stop().await;
}

#[tokio::test]
async fn pending_probe_counts_are_visible_before_response_and_success_keeps_its_attempt_number() {
    let fixture = CredentialRecoveryFixture::new().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-turn-state", fresh_state(1))
                .set_body_string("data: {\"type\":\"response.completed\"}\n\n")
                .set_delay(Duration::from_secs(3)),
        )
        .expect(2)
        .mount(&fixture.server)
        .await;
    fixture.discover().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.states.progress.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.states.writes.load(Ordering::SeqCst), 0);
    assert!(
        fixture
            .states
            .progress
            .lock()
            .unwrap()
            .values()
            .all(|value| *value == (1, None, None))
    );
    wait_count(&fixture.states.cancelled, 2).await;
    assert!(
        fixture
            .states
            .progress
            .lock()
            .unwrap()
            .values()
            .all(|value| *value == (1, None, Some(1)))
    );
    fixture.stop().await;
}

#[tokio::test]
async fn invalid_accounts_are_not_probed_even_if_switches_are_enabled() {
    let fixture = CredentialRecoveryFixture::new().await;
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
    let fixture = CredentialRecoveryFixture::new().await;
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
    wait_count(&calls, 1).await;
    let account = fixture.accounts.account("acct_recovery_probe").unwrap();
    let repository = fixture.accounts.repository();
    let mut data = repository.load_complete_data(&account).await.unwrap();
    data.oauth_mut().unwrap().access_token = "synthetic-new-generation".into();
    repository
        .compare_and_swap_data(&account, data)
        .await
        .unwrap();
    wait_count(&fixture.states.cancelled, 1).await;
    let current = fixture.accounts.account("acct_recovery_probe").unwrap();
    assert_ne!(current.revision(), account.revision());
    assert_eq!(current.credential_state(), CredentialState::Ready);
    assert_eq!(current.last_error_reason(), None);
    assert_eq!(fixture.states.writes.load(Ordering::SeqCst), 0);
    fixture.stop().await;
}

#[tokio::test]
async fn authentication_rejection_is_account_scoped_and_other_accounts_keep_collecting() {
    let fixture = CredentialRecoveryFixture::new().await;
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
            issued_at: SystemTime::now(),
            succeed: true,
            repeat_first: false,
            start_only: false,
            soft_cookie: false,
            times: Arc::default(),
        })
        .mount(&fixture.server)
        .await;
    fixture.discover().await;
    wait_count(&fixture.states.writes, 2).await;
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
        if account.as_str() == "acct_healthy_probe" {
            assert!(record.active().is_some() && record.standby().is_none());
        } else {
            assert!(record.active().is_none());
        }
    }
    fixture.stop().await;
}

#[tokio::test]
async fn transient_errors_keep_credentials_ready_and_do_not_become_authentication_failures() {
    for status in [400, 403, 503] {
        let fixture = CredentialRecoveryFixture::new().await;
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

#[tokio::test]
async fn probe_requires_completion_but_not_an_exact_reported_model() {
    for (body, reported_model, accept) in [
        (
            "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"synthetic failure\"}}}\n\n",
            None,
            false,
        ),
        (
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_probe\",\"model\":\"different-model\",\"status\":\"completed\",\"output\":[]}}\n\n",
            None,
            true,
        ),
        (
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_probe\",\"status\":\"completed\",\"output\":[]}}\n\n",
            Some("different-model"),
            true,
        ),
        ("data: [DONE]\n\n", None, false),
        (
            "event: response.completed\ndata: {\"type\":\"response.completed\"}\n\nevent: error\ndata: {\"error\":{\"code\":\"server_error\"}}\n\n",
            None,
            false,
        ),
    ] {
        let fixture = CredentialRecoveryFixture::new().await;
        let mut response = ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .insert_header("x-codex-turn-state", fresh_state(1))
            .set_body_string(body);
        if let Some(model) = reported_model {
            response = response.insert_header("openai-model", model);
        }
        Mock::given(method("POST"))
            .respond_with(response)
            .mount(&fixture.server)
            .await;
        fixture.discover().await;
        if accept {
            wait_count(&fixture.states.writes, 2).await;
        } else {
            tokio::time::timeout(Duration::from_secs(5), async {
                while fixture.server.received_requests().await.unwrap().len() < 22 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(fixture.states.writes.load(Ordering::SeqCst), 0);
        }
        assert_eq!(fixture.states.failed.load(Ordering::SeqCst), 0);
        assert_eq!(
            fixture
                .accounts
                .account("acct_recovery_probe")
                .unwrap()
                .credential_state(),
            CredentialState::Ready
        );
        fixture.stop().await;
    }
}

#[tokio::test]
async fn authentication_failure_inside_sse_stops_collection_without_accepting_its_state() {
    let fixture = CredentialRecoveryFixture::new().await;
    Mock::given(method("POST")).respond_with(
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .insert_header("x-codex-turn-state", fresh_state(1))
            .set_body_string("event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"token_invalidated\",\"message\":\"Token revoked\"}}}\n\n")
    ).mount(&fixture.server).await;
    fixture.discover().await;
    wait_count(&fixture.states.cancelled, 1).await;
    assert_eq!(fixture.states.writes.load(Ordering::SeqCst), 0);
    assert!((1..=2).contains(&fixture.server.received_requests().await.unwrap().len()));
    assert_eq!(
        fixture
            .accounts
            .account("acct_recovery_probe")
            .unwrap()
            .credential_state(),
        CredentialState::Expired
    );
    fixture.stop().await;
}

#[tokio::test]
async fn rate_limit_stops_sibling_models_and_requeues_only_after_shared_cooldown() {
    let fixture = CredentialRecoveryFixture::new().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "60")
                .set_body_json(serde_json::json!({"error":{"code":"rate_limit_exceeded"}})),
        )
        .mount(&fixture.server)
        .await;
    fixture.discover().await;
    wait_count(&fixture.states.cancelled, 1).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let calls = fixture.server.received_requests().await.unwrap().len();
    assert!((1..=2).contains(&calls));
    assert!(!fixture.cooldowns.cooldowns.lock().unwrap().is_empty());
    fixture.discover().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        fixture.server.received_requests().await.unwrap().len(),
        calls
    );
    assert_eq!(
        fixture
            .accounts
            .account("acct_recovery_probe")
            .unwrap()
            .credential_state(),
        CredentialState::Ready
    );

    fixture.server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-turn-state", fresh_state(1))
                .set_body_string("data: {\"type\":\"response.completed\"}\n\n"),
        )
        .mount(&fixture.server)
        .await;
    fixture.cooldowns.cooldowns.lock().unwrap().clear();
    fixture.discover().await;
    wait_count(&fixture.states.writes, 2).await;
    fixture.stop().await;
}

#[tokio::test]
async fn disable_and_reenable_reuses_valid_capture_without_starting_another_probe() {
    let fixture = CredentialRecoveryFixture::new().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-turn-state", fresh_state(1))
                .set_body_string("data: {\"type\":\"response.completed\"}\n\n"),
        )
        .mount(&fixture.server)
        .await;
    fixture.discover().await;
    wait_count(&fixture.states.writes, 2).await;
    wait_count(&fixture.states.cancelled, 2).await;
    let original = fixture.states.records.lock().unwrap().clone();
    fixture
        .tuning
        .publish_openai_turn_state_policy(OpenAiTurnStatePolicy::default());
    fixture.discover().await;
    assert_eq!(*fixture.states.records.lock().unwrap(), original);
    fixture
        .tuning
        .publish_openai_turn_state_policy(OpenAiTurnStatePolicy::new(
            true,
            ["model-a", "model-b"]
                .map(|model| UpstreamModelId::new(model).unwrap())
                .into(),
        ));
    fixture.discover().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
    assert_eq!(*fixture.states.records.lock().unwrap(), original);
    fixture.stop().await;
}

#[tokio::test]
async fn soft_cookie_save_does_not_hide_an_inflight_authentication_rejection() {
    let fixture = CredentialRecoveryFixture::new().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            seen.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({"error":{"code":"token_invalidated"}}))
                .set_delay(Duration::from_millis(350))
        })
        .mount(&fixture.server)
        .await;
    fixture.discover().await;
    wait_count(&calls, 1).await;
    let account = fixture.accounts.account("acct_recovery_probe").unwrap();
    let repository = fixture.accounts.repository();
    let mut data = repository.load_complete_data(&account).await.unwrap();
    data.cookies_mut()
        .push(provider_openai::credential::CodexCookie {
            name: "__Secure-next-auth.session-token".into(),
            value: "synthetic-soft-cookie".into(),
            domain: "::1".into(),
            path: "/".into(),
            host_only: true,
            secure: false,
            expires_at: None,
        });
    repository
        .compare_and_swap_data(&account, data)
        .await
        .unwrap();
    let changed = fixture.accounts.account("acct_recovery_probe").unwrap();
    assert_ne!(changed.revision(), account.revision());
    assert_eq!(
        changed.turn_state_binding_revision(),
        account.turn_state_binding_revision()
    );
    wait_count(&fixture.states.cancelled, 1).await;
    assert_eq!(
        fixture
            .accounts
            .account("acct_recovery_probe")
            .unwrap()
            .credential_state(),
        CredentialState::Expired
    );
    assert_eq!(fixture.states.writes.load(Ordering::SeqCst), 0);
    fixture.stop().await;
}

#[tokio::test]
async fn account_slots_refill_without_waiting_for_other_accounts_or_releasing_on_one_model() {
    let fixture = CredentialRecoveryFixture::new().await;
    // Together with the fixture's account: five admitted, two queued.
    for index in 0..6 {
        let mut imported = CodexCredentialAdmin
            .prepare_import(ImportCodexOAuthCredential {
                account_id: format!("acct_queue_{index}"),
                name: format!("Queue {index}"),
                secret: secret(&format!("queue-{index}")),
                verified_account: profile(&format!("queue-owner-{index}")),
                next_refresh_at: None,
                enabled: true,
            })
            .unwrap();
        imported.account = imported.account.with_turn_state_injection_enabled(true);
        fixture.accounts.create_account(imported).await.unwrap();
    }
    let calls = Arc::new(Mutex::new(BTreeMap::<String, usize>::new()));
    let seen = calls.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let owner = request.headers["chatgpt-account-id"]
                .to_str()
                .unwrap()
                .to_owned();
            *seen.lock().unwrap().entry(owner.clone()).or_default() += 1;
            let raw = if request
                .headers
                .get("content-encoding")
                .is_some_and(|v| v == "zstd")
            {
                zstd::stream::decode_all(request.body.as_slice()).unwrap()
            } else {
                request.body.clone()
            };
            let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            let delay = match (owner.as_str(), body["model"].as_str()) {
                ("queue-owner-0", Some("model-a")) => Duration::ZERO,
                ("queue-owner-0", _) => Duration::from_millis(700),
                ("queue-owner-1", _) => Duration::from_millis(1300),
                _ => Duration::from_secs(30),
            };
            ResponseTemplate::new(200)
                .insert_header("x-codex-turn-state", fresh_state(1))
                .set_body_string("data: {\"type\":\"response.completed\"}\n\n")
                .set_delay(delay)
        })
        .mount(&fixture.server)
        .await;
    fixture.discover().await;
    wait_count(&fixture.states.writes, 1).await;
    // Repeated discovery must neither duplicate a key nor admit a sixth account early.
    fixture.discover().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(calls.lock().unwrap().len(), 5);
    assert!(calls.lock().unwrap().values().all(|count| *count == 2));
    wait_count(&fixture.states.writes, 2).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.lock().unwrap().len() < 6 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(calls.lock().unwrap().len(), 6);
    wait_count(&fixture.states.writes, 4).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        // Account admission is visible after its first model arrives; wait for
        // both model requests before checking that neither key was duplicated.
        while {
            let counts = calls.lock().unwrap();
            counts.len() < 7 || counts.values().any(|count| *count < 2)
        } {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(calls.lock().unwrap().values().all(|count| *count == 2));
    fixture.stop().await;
}
