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
        ProviderLeaseAcquisition, ProviderLeasePort, ProviderLeaseRequest, ProviderSchedulingState,
        ProviderStoreError, ProviderStorePorts, ProviderTurnStateAnomaly,
        ProviderTurnStateCandidate, ProviderTurnStatePort, ProviderTurnStateRecord,
        ProviderTurnStateRefreshStatus, ProviderTurnStateSlot,
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
struct ProbeStates {
    record: Mutex<Option<ProviderTurnStateRecord>>,
    writes: AtomicUsize,
}

impl ProviderTurnStatePort for ProbeStates {
    fn read<'a>(
        &'a self,
        _: &'a ProviderAccountId,
        _: &'a UpstreamModelId,
        _: CredentialRevision,
    ) -> BoxFuture<'a, Result<Option<ProviderTurnStateRecord>, ProviderStoreError>> {
        Box::pin(async { Ok(self.record.lock().unwrap().clone()) })
    }

    fn put_candidate(
        &self,
        candidate: ProviderTurnStateCandidate,
    ) -> BoxFuture<'_, Result<ProviderTurnStateRecord, ProviderStoreError>> {
        Box::pin(async move {
            assert_eq!(candidate.slot, ProviderTurnStateSlot::Active);
            self.writes.fetch_add(1, Ordering::SeqCst);
            let record = ProviderTurnStateRecord::new(
                candidate.account_id,
                candidate.upstream_model,
                candidate.normal_length,
                Some(candidate.value),
                None,
                1,
                ProviderTurnStateRefreshStatus::Refreshing,
                Some(candidate.normal_length),
            );
            *self.record.lock().unwrap() = Some(record.clone());
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
            Ok(self.record.lock().unwrap().clone().unwrap_or_else(|| {
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
    let base = provider_ports_with_accounts(accounts);
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
        account_overrides: [(account_id, None)].into(),
        ..ProviderEgressConfig::default()
    })));
    let listener = std::net::TcpListener::bind("[::1]:0").expect("IPv6 loopback");
    let server = MockServer::builder().listener(listener).start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ProbeResponder(calls.clone()))
        .expect(2)
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
    wait_count(&leases.reads, 1).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "busy account yields to business traffic"
    );
    leases.busy.store(0, Ordering::SeqCst);
    discover.run_cycle(cycle.clone()).await.unwrap();
    wait_count(&calls, 1).await;
    tuning.publish_openai_turn_state_policy(OpenAiTurnStatePolicy::default());
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(states.writes.load(Ordering::SeqCst), 0);
    tuning.publish_openai_turn_state_policy(policy);
    discover.run_cycle(cycle).await.unwrap();
    // A serial collector can start this second probe only after the first was cancelled.
    wait_count(&calls, 2).await;
    wait_count(&states.writes, 1).await;
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
