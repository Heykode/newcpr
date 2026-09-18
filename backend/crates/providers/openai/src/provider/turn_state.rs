//! Account-and-model scoped managed Codex turn state.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use futures::{StreamExt as _, stream};
use gateway_core::{
    account::{ProviderAccount, ProviderAccountId},
    provider_ports::{
        OpaqueTurnState, ProviderTurnStateAnomaly, ProviderTurnStateCandidate,
        ProviderTurnStatePort, ProviderTurnStateRecord, ProviderTurnStateRefreshStatus,
        ProviderTurnStateSlot, ProviderTurnStateValue,
    },
    routing::UpstreamModelId,
    runtime::RequestTuningHandle,
};
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, Semaphore};
use uuid::Uuid;

use crate::{
    credential::CodexCredentialRepository,
    transport::{
        CodexAccountSelectionTelemetry, CodexBackendClient,
        egress::CodexEgressRuntime,
        protocol::responses::CodexResponsesRequest,
        request::{RequestAccountScope, scope_request_to_account},
    },
};

use super::{build_cookie_header, codex_request_context};

const TURN_STATE_TTL: Duration = Duration::from_secs(60 * 60);
const INDIVIDUAL_NORMAL_LENGTH: u16 = 292;
const INDIVIDUAL_DEGRADED_LENGTH: u16 = 312;
const TEAM_NORMAL_LENGTH: u16 = 332;
const TEAM_DEGRADED_LENGTH: u16 = 356;
const MAX_PENDING_OBSERVATIONS: usize = 256;
const STATE_STORE_TIMEOUT: Duration = Duration::from_secs(1);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PROBES: usize = 500;
const MAX_BATCH: usize = 100;
const MAX_COLLECTORS: usize = 32;

type ObservationKey = (ProviderAccountId, UpstreamModelId);

enum TurnStateObservation {
    Candidate(ProviderTurnStateCandidate),
    Anomaly(ProviderTurnStateAnomaly),
}

impl TurnStateObservation {
    fn priority(&self) -> (u64, Option<u64>, bool) {
        match self {
            Self::Candidate(candidate) => (
                candidate.expected_revision.get(),
                candidate.expected_active_version,
                false,
            ),
            Self::Anomaly(anomaly) => (
                anomaly.expected_revision.get(),
                Some(anomaly.expected_active_version),
                true,
            ),
        }
    }
}

#[derive(Default)]
struct PendingObservations {
    values: Mutex<HashMap<ObservationKey, TurnStateObservation>>,
    ready: Notify,
}

impl PendingObservations {
    fn enqueue(&self, key: ObservationKey, observation: TurnStateObservation) {
        let mut values = self.values.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(current) = values.get_mut(&key) {
            // A same-version echo must not erase rejection before the store sees it.
            if observation.priority() < current.priority() {
                return;
            }
            *current = observation;
        } else if values.len() < MAX_PENDING_OBSERVATIONS {
            values.insert(key, observation);
        } else {
            return;
        }
        self.ready.notify_one();
    }
}

#[derive(Default)]
struct MaintenanceQueue {
    values: Mutex<VecDeque<(ObservationKey, bool)>>,
    ready: Notify,
}

impl MaintenanceQueue {
    fn enqueue(&self, key: ObservationKey, force_standby: bool) -> bool {
        let mut values = self.values.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, force)) = values.iter_mut().find(|(pending, _)| pending == &key) {
            *force |= force_standby;
        } else if values.len() < MAX_PENDING_OBSERVATIONS {
            values.push_back((key, force_standby));
        } else {
            return false;
        }
        self.ready.notify_one();
        true
    }

    fn pop_available(&self, running: &HashSet<ObservationKey>) -> Option<(ObservationKey, bool)> {
        let mut values = self.values.lock().unwrap_or_else(|e| e.into_inner());
        let index = values.iter().position(|(key, _)| !running.contains(key))?;
        values.remove(index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexTurnStateShape {
    Normal,
    Degraded,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParsedCodexTurnState {
    issued_at: SystemTime,
    expires_at: SystemTime,
    encoded_length: u16,
}

impl ParsedCodexTurnState {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.len() > 4096 {
            return None;
        }
        let encoded_length = u16::try_from(value.len()).ok()?;
        let bytes = URL_SAFE
            .decode(value)
            .or_else(|_| URL_SAFE_NO_PAD.decode(value))
            .ok()?;
        if bytes.first().copied() != Some(0x80) || bytes.len() < 9 + 16 + 16 + 32 {
            return None;
        }
        let timestamp = u64::from_be_bytes(bytes.get(1..9)?.try_into().ok()?);
        let cipher_bytes = bytes.len().checked_sub(9 + 16 + 32)?;
        if cipher_bytes == 0 || cipher_bytes % 16 != 0 {
            return None;
        }
        let issued_at = UNIX_EPOCH.checked_add(Duration::from_secs(timestamp))?;
        let expires_at = issued_at.checked_add(TURN_STATE_TTL)?;
        Some(Self {
            issued_at,
            expires_at,
            encoded_length,
        })
    }

    pub(crate) const fn issued_at(self) -> SystemTime {
        self.issued_at
    }

    pub(crate) const fn expires_at(self) -> SystemTime {
        self.expires_at
    }

    pub(crate) const fn encoded_length(self) -> u16 {
        self.encoded_length
    }
}

#[derive(Clone)]
pub(crate) struct CodexTurnStateManager {
    store: Arc<dyn ProviderTurnStatePort>,
    request_tuning: RequestTuningHandle,
    pending: Arc<PendingObservations>,
    maintenance: Arc<MaintenanceQueue>,
    websocket_pool: Option<Arc<crate::transport::websocket::CodexWebSocketPool>>,
}

impl CodexTurnStateManager {
    pub(crate) fn new(
        store: Arc<dyn ProviderTurnStatePort>,
        request_tuning: RequestTuningHandle,
    ) -> Self {
        Self {
            store,
            request_tuning,
            pending: Arc::default(),
            maintenance: Arc::default(),
            websocket_pool: None,
        }
    }

    pub(crate) fn with_websocket_pool(
        mut self,
        pool: Arc<crate::transport::websocket::CodexWebSocketPool>,
    ) -> Self {
        self.websocket_pool = Some(pool);
        self
    }

    fn retire_old_pools(&self, record: &ProviderTurnStateRecord) {
        if let Some(pool) = &self.websocket_pool {
            pool.retire_managed_state(
                record.account_id().as_str(),
                record.upstream_model().as_str(),
                record.state_version(),
            );
        }
    }

    pub(crate) fn feature_enabled_for(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
    ) -> bool {
        let policy = self.request_tuning.openai_turn_state_policy();
        policy.enabled()
            && policy.contains(model)
            && account.enabled()
            && account.turn_state_injection_enabled()
    }

    pub(crate) async fn active(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
        now: SystemTime,
    ) -> Option<(String, u64, SystemTime)> {
        if !self.feature_enabled_for(account, model) {
            return None;
        }
        let record = tokio::time::timeout(
            STATE_STORE_TIMEOUT,
            self.store.read(account.id(), model, account.revision()),
        )
        .await
        .ok()?
        .ok()
        .flatten()?;
        if record.normal_length() != expected_normal_length(account.plan_type()) {
            return None;
        }
        self.retire_old_pools(&record);
        let active = record
            .active()?
            .is_valid_at(now)
            .then_some(record.active()?)?;
        Some((
            active.state().expose_to_provider().to_owned(),
            record.state_version(),
            active.expires_at(),
        ))
    }

    pub(crate) async fn allows_new_request(&self, account: &ProviderAccount, model: &str) -> bool {
        let Ok(model) = UpstreamModelId::new(model) else {
            return false;
        };
        if !self.feature_enabled_for(account, &model) {
            return true;
        }
        if self
            .active(account, &model, SystemTime::now())
            .await
            .is_some()
        {
            return true;
        }
        self.maintenance
            .enqueue((account.id().clone(), model.clone()), false);
        // Opt-out during the read restores ordinary scheduling immediately.
        !self.feature_enabled_for(account, &model)
    }

    pub(crate) fn models(&self) -> Vec<UpstreamModelId> {
        self.request_tuning
            .openai_turn_state_policy()
            .models()
            .iter()
            .cloned()
            .collect()
    }

    pub(crate) async fn record(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
    ) -> Option<ProviderTurnStateRecord> {
        tokio::time::timeout(
            STATE_STORE_TIMEOUT,
            self.store.read(account.id(), model, account.revision()),
        )
        .await
        .ok()?
        .ok()
        .flatten()
    }

    pub(crate) async fn mark_refresh_status(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
        normal_length: u16,
        status: ProviderTurnStateRefreshStatus,
        observed_at: SystemTime,
    ) -> bool {
        tokio::time::timeout(
            STATE_STORE_TIMEOUT,
            self.store.mark_refresh_status(
                account.id(),
                model,
                account.revision(),
                normal_length,
                status,
                observed_at,
            ),
        )
        .await
        .is_ok_and(|result| result.is_ok())
    }

    pub(crate) fn observe(
        &self,
        account: &ProviderAccount,
        requested_model: &UpstreamModelId,
        injected_version: Option<u64>,
        reported_model: Option<&str>,
        value: &str,
        observed_at: SystemTime,
    ) {
        if !self.feature_enabled_for(account, requested_model)
            || reported_model.is_some_and(|model| model.trim() != requested_model.as_str())
        {
            return;
        }
        let normal_length = expected_normal_length(account.plan_type());
        let Some(parsed) = ParsedCodexTurnState::parse(value) else {
            return;
        };
        if parsed.issued_at() > observed_at || parsed.expires_at() <= observed_at {
            return;
        }
        let observation = match classify_length(normal_length, parsed.encoded_length()) {
            CodexTurnStateShape::Normal => {
                TurnStateObservation::Candidate(ProviderTurnStateCandidate {
                    account_id: account.id().clone(),
                    expected_revision: account.revision(),
                    expected_active_version: injected_version,
                    upstream_model: requested_model.clone(),
                    normal_length,
                    slot: ProviderTurnStateSlot::Active,
                    value: ProviderTurnStateValue::new(
                        OpaqueTurnState::new(value.trim().to_owned()),
                        parsed.issued_at(),
                        parsed.expires_at(),
                    ),
                    observed_at,
                })
            }
            CodexTurnStateShape::Degraded => {
                let Some(expected_active_version) = injected_version else {
                    return;
                };
                TurnStateObservation::Anomaly(ProviderTurnStateAnomaly {
                    account_id: account.id().clone(),
                    expected_revision: account.revision(),
                    expected_active_version,
                    upstream_model: requested_model.clone(),
                    normal_length,
                    observed_length: Some(parsed.encoded_length()),
                    promote_standby: true,
                    observed_at,
                })
            }
            // Unknown formats are not evidence that the current state was rejected.
            CodexTurnStateShape::Unexpected => return,
        };
        let key = (account.id().clone(), requested_model.clone());
        self.pending.enqueue(key, observation);
    }

    pub(crate) fn observe_failure(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
        injected_version: Option<u64>,
        reported_model: Option<&str>,
        error: &gateway_core::error::ProviderError,
    ) {
        let Some(expected_active_version) = injected_version else {
            return;
        };
        let code = error.upstream_code().map(|code| code.as_str()).or_else(|| {
            error
                .client_visible_upstream_error()
                .and_then(|error| error.code())
        });
        if !self.feature_enabled_for(account, model)
            || !is_state_rejection_code(code)
            || reported_model.is_some_and(|reported| reported.trim() != model.as_str())
        {
            return;
        }
        let observation = TurnStateObservation::Anomaly(ProviderTurnStateAnomaly {
            account_id: account.id().clone(),
            expected_revision: account.revision(),
            expected_active_version,
            upstream_model: model.clone(),
            normal_length: expected_normal_length(account.plan_type()),
            observed_length: None,
            promote_standby: true,
            observed_at: SystemTime::now(),
        });
        let key = (account.id().clone(), model.clone());
        self.pending.enqueue(key, observation);
    }

    pub(crate) async fn run_observations(&self) {
        loop {
            self.pending.ready.notified().await;
            loop {
                let next =
                    {
                        let mut pending = self
                            .pending
                            .values
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        pending.keys().next().cloned().and_then(|key| {
                            pending.remove(&key).map(|observation| (key, observation))
                        })
                    };
                let Some(((account_id, model), observation)) = next else {
                    break;
                };
                let policy = self.request_tuning.openai_turn_state_policy();
                if !policy.enabled() || !policy.contains(&model) {
                    continue;
                }
                // Passive learning is best effort; never hold a response or an unbounded task.
                let _ = tokio::time::timeout(STATE_STORE_TIMEOUT, async {
                    let revision = match &observation {
                        TurnStateObservation::Candidate(candidate) => candidate.expected_revision,
                        TurnStateObservation::Anomaly(anomaly) => anomaly.expected_revision,
                    };
                    let before = self
                        .store
                        .read(&account_id, &model, revision)
                        .await
                        .ok()
                        .flatten();
                    let result = match observation {
                        TurnStateObservation::Candidate(candidate) => {
                            self.store.put_candidate(candidate).await
                        }
                        TurnStateObservation::Anomaly(anomaly) => {
                            self.store.record_anomaly(anomaly).await
                        }
                    };
                    if let Ok(record) = result
                        && before
                            .as_ref()
                            .is_none_or(|old| old.state_version() != record.state_version())
                    {
                        self.retire_old_pools(&record);
                        self.maintenance.enqueue((account_id, model), true);
                    }
                })
                .await;
            }
        }
    }

    pub(crate) async fn put_probe_candidate(
        &self,
        account: &ProviderAccount,
        model: UpstreamModelId,
        normal_length: u16,
        value: String,
        slot: ProviderTurnStateSlot,
        observed_at: SystemTime,
    ) -> bool {
        let Some(parsed) = ParsedCodexTurnState::parse(&value) else {
            return false;
        };
        if parsed.issued_at() > observed_at
            || parsed.expires_at() <= observed_at
            || classify_length(normal_length, parsed.encoded_length())
                != CodexTurnStateShape::Normal
        {
            return false;
        }
        let margin = match slot {
            ProviderTurnStateSlot::Active => Duration::from_secs(10 * 60),
            ProviderTurnStateSlot::Standby => Duration::from_secs(30 * 60),
        };
        if parsed
            .expires_at()
            .duration_since(observed_at)
            .is_ok_and(|remaining| remaining <= margin)
        {
            return false;
        }
        if self.record(account, &model).await.is_some_and(|record| {
            record
                .active()
                .is_some_and(|active| active.state().expose_to_provider() == value)
                || (slot == ProviderTurnStateSlot::Standby
                    && record
                        .standby()
                        .is_some_and(|standby| standby.state().expose_to_provider() == value))
        }) {
            return false;
        }
        let expected = value.clone();
        let result = tokio::time::timeout(
            STATE_STORE_TIMEOUT,
            self.store.put_candidate(ProviderTurnStateCandidate {
                account_id: account.id().clone(),
                expected_revision: account.revision(),
                expected_active_version: None,
                upstream_model: model,
                normal_length,
                slot,
                value: ProviderTurnStateValue::new(
                    OpaqueTurnState::new(value),
                    parsed.issued_at(),
                    parsed.expires_at(),
                ),
                observed_at,
            }),
        )
        .await;
        let Some(record) = result.ok().and_then(Result::ok) else {
            return false;
        };
        self.retire_old_pools(&record);
        {
            let stored = match slot {
                ProviderTurnStateSlot::Active => record.active(),
                ProviderTurnStateSlot::Standby => record.standby(),
            };
            stored.is_some_and(|stored| stored.state().expose_to_provider() == expected)
        }
    }
}

pub(crate) fn expected_normal_length(plan_type: Option<&str>) -> u16 {
    match plan_type
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("team" | "self_serve_business_prolite" | "self_serve_business_usage_based") => {
            TEAM_NORMAL_LENGTH
        }
        _ => INDIVIDUAL_NORMAL_LENGTH,
    }
}

pub(crate) const fn classify_length(
    normal_length: u16,
    observed_length: u16,
) -> CodexTurnStateShape {
    match (normal_length, observed_length) {
        (INDIVIDUAL_NORMAL_LENGTH, INDIVIDUAL_NORMAL_LENGTH)
        | (TEAM_NORMAL_LENGTH, TEAM_NORMAL_LENGTH) => CodexTurnStateShape::Normal,
        (INDIVIDUAL_NORMAL_LENGTH, INDIVIDUAL_DEGRADED_LENGTH)
        | (TEAM_NORMAL_LENGTH, TEAM_DEGRADED_LENGTH) => CodexTurnStateShape::Degraded,
        _ => CodexTurnStateShape::Unexpected,
    }
}

pub(crate) fn needs_refresh(record: &ProviderTurnStateRecord, now: SystemTime) -> bool {
    const REFRESH_MARGIN: Duration = Duration::from_secs(10 * 60);
    record
        .active()
        .is_none_or(|active| match active.expires_at().duration_since(now) {
            Ok(remaining) => remaining <= REFRESH_MARGIN,
            Err(_) => true,
        })
}

fn standby_needs_refresh(record: &ProviderTurnStateRecord, now: SystemTime) -> bool {
    const STANDBY_MARGIN: Duration = Duration::from_secs(30 * 60);
    record.refresh_status() != ProviderTurnStateRefreshStatus::Ready
        || record
            .standby()
            .is_none_or(|standby| match standby.expires_at().duration_since(now) {
                Ok(remaining) => remaining <= STANDBY_MARGIN,
                Err(_) => true,
            })
}

fn refresh_slots(
    record: Option<&ProviderTurnStateRecord>,
    normal_length: u16,
    now: SystemTime,
) -> (bool, bool) {
    match record {
        Some(record) if record.normal_length() == normal_length => (
            needs_refresh(record, now),
            standby_needs_refresh(record, now),
        ),
        _ => (true, true),
    }
}

#[derive(Clone)]
pub(crate) struct CodexTurnStateMaintenanceService {
    repository: CodexCredentialRepository,
    client: CodexBackendClient,
    egress: Option<Arc<CodexEgressRuntime>>,
    manager: CodexTurnStateManager,
    discovery_cursor: Arc<AtomicUsize>,
    probe_capacity: Arc<Semaphore>,
}

impl CodexTurnStateMaintenanceService {
    pub(crate) fn new(
        repository: CodexCredentialRepository,
        client: CodexBackendClient,
        egress: Option<Arc<CodexEgressRuntime>>,
        manager: CodexTurnStateManager,
    ) -> Self {
        Self {
            repository,
            client,
            egress,
            manager,
            discovery_cursor: Arc::default(),
            probe_capacity: Arc::new(Semaphore::new(MAX_BATCH)),
        }
    }

    pub(crate) async fn synchronize(&self) {
        if !self
            .manager
            .request_tuning
            .openai_turn_state_policy()
            .enabled()
        {
            return;
        }
        if self.egress.is_none() {
            return;
        }
        let Ok(accounts) = self.repository.list_for_provider().await else {
            return;
        };
        let models = self.manager.models();
        let targets = accounts
            .iter()
            .filter(|account| account.enabled() && account.turn_state_injection_enabled())
            .flat_map(|account| {
                models
                    .iter()
                    .map(|model| (account.id().clone(), model.clone()))
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return;
        }
        let start = self.discovery_cursor.load(Ordering::Relaxed) % targets.len();
        for offset in 0..targets.len() {
            let index = (start + offset) % targets.len();
            self.discovery_cursor.store(index, Ordering::Relaxed);
            if !self
                .manager
                .maintenance
                .enqueue(targets[index].clone(), false)
            {
                break;
            }
            self.discovery_cursor
                .store((index + 1) % targets.len(), Ordering::Relaxed);
        }
    }

    pub(crate) async fn run_observations(&self) {
        self.manager.run_observations().await;
    }

    pub(crate) async fn run_maintenance(&self) {
        let mut running = HashSet::new();
        let mut tasks = stream::FuturesUnordered::new();
        loop {
            while tasks.len() < MAX_COLLECTORS {
                let Some((key, force)) = self.manager.maintenance.pop_available(&running) else {
                    break;
                };
                running.insert(key.clone());
                tasks.push(async move {
                    self.run_target(&key.0, &key.1, force).await;
                    key
                });
            }
            tokio::select! {
                Some(key) = tasks.next(), if !tasks.is_empty() => { running.remove(&key); },
                () = self.manager.maintenance.ready.notified(), if tasks.len() < MAX_COLLECTORS => {},
            }
        }
    }

    async fn run_target(
        &self,
        account_id: &ProviderAccountId,
        model: &UpstreamModelId,
        force: bool,
    ) {
        let Some(egress) = &self.egress else { return };
        let Ok(Some(account)) = self.repository.store().get_account(account_id).await else {
            return;
        };
        if !self.target_current(&account, model).await {
            return;
        }
        tokio::select! {
            () = self.maintain_target(&account, model, egress, force) => {},
            () = self.wait_until_invalid(&account, model) => {},
        }
        // A batch-boundary check can observe cancellation before the watcher does.
        // The store only closes a still-refreshing row for this credential revision.
        let _ = tokio::time::timeout(
            STATE_STORE_TIMEOUT,
            self.manager
                .store
                .cancel_refresh(account.id(), model, account.revision()),
        )
        .await;
    }

    async fn target_current(&self, account: &ProviderAccount, model: &UpstreamModelId) -> bool {
        if !self.manager.feature_enabled_for(account, model) {
            return false;
        }
        tokio::time::timeout(
            STATE_STORE_TIMEOUT,
            self.repository.store().get_account(account.id()),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .is_some_and(|current| {
            current.revision() == account.revision()
                && current.plan_type() == account.plan_type()
                && self.manager.feature_enabled_for(&current, model)
        })
    }

    async fn wait_until_invalid(&self, account: &ProviderAccount, model: &UpstreamModelId) {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if !self.target_current(account, model).await {
                return;
            }
        }
    }

    async fn maintain_target(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
        egress: &CodexEgressRuntime,
        force_standby: bool,
    ) {
        let now = SystemTime::now();
        let record = self.manager.record(account, model).await;
        let normal_length = expected_normal_length(account.plan_type());
        let (active_due, standby_due) = refresh_slots(record.as_ref(), normal_length, now);
        let standby_due = standby_due || force_standby;
        if !active_due && !standby_due {
            return;
        }
        let Ok(mut sources) = egress.probe_sources() else {
            return;
        };
        sources.truncate(MAX_PROBES);
        if sources.is_empty() {
            return;
        }
        let runtime = match self.repository.load_runtime_credential(account).await {
            Ok(runtime) => Arc::new(runtime),
            Err(_) => return,
        };
        if active_due
            && self
                .collect_slot(
                    account,
                    model,
                    normal_length,
                    ProviderTurnStateSlot::Active,
                    &runtime,
                    &sources,
                )
                .await
        {
            // Standby has its own acquisition budget after a fresh active is committed.
            let _ = self
                .collect_slot(
                    account,
                    model,
                    normal_length,
                    ProviderTurnStateSlot::Standby,
                    &runtime,
                    &sources,
                )
                .await;
        } else if !active_due && standby_due {
            let _ = self
                .collect_slot(
                    account,
                    model,
                    normal_length,
                    ProviderTurnStateSlot::Standby,
                    &runtime,
                    &sources,
                )
                .await;
        }
    }

    async fn collect_slot(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
        normal_length: u16,
        slot: ProviderTurnStateSlot,
        runtime: &Arc<crate::credential::CodexRuntimeCredential>,
        sources: &[std::net::Ipv6Addr],
    ) -> bool {
        if !self
            .manager
            .mark_refresh_status(
                account,
                model,
                normal_length,
                ProviderTurnStateRefreshStatus::Refreshing,
                SystemTime::now(),
            )
            .await
        {
            return false;
        }
        let mut round = 0_usize;
        let mut cursor = 0_usize;
        let mut failures = HashMap::<&'static str, usize>::new();
        while cursor < MAX_PROBES {
            if !self.target_current(account, model).await {
                return false;
            }
            let batch_size = probe_batch_size(round, cursor, MAX_PROBES);
            let batch = (cursor..cursor + batch_size).map(|index| sources[index % sources.len()]);
            cursor += batch_size;
            round += 1;
            let results = stream::iter(batch.map(|source| async move {
                // Waiting for shared transport capacity does not spend a probe timeout.
                let _permit = self
                    .probe_capacity
                    .acquire()
                    .await
                    .map_err(|_| "capacity_closed")?;
                tokio::time::timeout(
                    PROBE_TIMEOUT,
                    self.probe(account.clone(), model.clone(), Arc::clone(runtime), source),
                )
                .await
                .unwrap_or(Err("timeout"))
            }))
            .buffer_unordered(batch_size);
            tokio::pin!(results);
            while let Some(value) = results.next().await {
                let value = match value {
                    Ok(value) => value,
                    Err(reason) => {
                        *failures.entry(reason).or_default() += 1;
                        continue;
                    }
                };
                if !self.target_current(account, model).await {
                    return false;
                }
                if self
                    .manager
                    .put_probe_candidate(
                        account,
                        model.clone(),
                        normal_length,
                        value,
                        slot,
                        SystemTime::now(),
                    )
                    .await
                {
                    tracing::info!(
                        model = model.as_str(),
                        ?slot,
                        allocated = cursor,
                        "Turn state acquisition succeeded"
                    );
                    // Dropping the bounded batch cancels its remaining probes.
                    return true;
                }
                *failures.entry("unusable_state").or_default() += 1;
            }
        }
        self.manager
            .mark_refresh_status(
                account,
                model,
                normal_length,
                ProviderTurnStateRefreshStatus::Failed,
                SystemTime::now(),
            )
            .await;
        tracing::warn!(
            model = model.as_str(),
            ?slot,
            attempted = cursor,
            ?failures,
            "Turn state acquisition exhausted request budget"
        );
        false
    }

    async fn probe(
        &self,
        account: ProviderAccount,
        model: UpstreamModelId,
        runtime: Arc<crate::credential::CodexRuntimeCredential>,
        source: std::net::Ipv6Addr,
    ) -> Result<String, &'static str> {
        let client = self
            .client
            .for_probe_source(&account, source)
            .map_err(|_| "egress_unavailable")?;
        let authorization = runtime
            .authentication
            .authorization_header()
            .map_err(|_| "authorization")?;
        let cookie = build_cookie_header(&runtime.cookies).map_err(|_| "cookie")?;
        let request_id = Uuid::now_v7().to_string();
        let mut body = Map::new();
        body.insert("model".to_owned(), Value::String(model.as_str().to_owned()));
        body.insert(
            "input".to_owned(),
            json!([{"type":"message","role":"user","content":[{"type":"input_text","text":"Reply exactly yes."}]}]),
        );
        body.insert("stream".to_owned(), Value::Bool(true));
        body.insert("store".to_owned(), Value::Bool(false));
        let mut request = CodexResponsesRequest::from_body(body);
        request.force_http_sse = true;
        request.client_api_key_id = Some("turn-state-maintenance".to_owned());
        request.identity_seed = Some(crate::transport::qx_application::identity_seed(
            &request,
            &request_id,
        ));
        scope_request_to_account(
            &mut request,
            &runtime.installation_id,
            RequestAccountScope::Same,
        );
        let context = codex_request_context(
            &request,
            &request_id,
            &account,
            &runtime.installation_id,
            &authorization,
            cookie.as_ref(),
            CodexAccountSelectionTelemetry::NONE,
        );
        let mut response = client
            .create_response_stream_http_sse(&request, context)
            .await
            .map_err(|error| match error {
                crate::transport::CodexClientError::Upstream { status, .. }
                    if status.as_u16() == 429 =>
                {
                    "upstream_429"
                }
                crate::transport::CodexClientError::Upstream { .. } => "upstream_error",
                _ => "transport_error",
            })?;
        let state = response.turn_state.take();
        let mut decoder = crate::transport::canonical::CodexCanonicalDecoder::new(model.as_str())
            .with_reported_model(response.response_metadata.effective_model.as_deref());
        while let Some(chunk) = response.body.next().await {
            let chunk = chunk.map_err(|_| "stream_error")?;
            if matches!(
                decoder.push(&chunk),
                crate::transport::canonical::CodexCanonicalOutcome::Failed(_)
            ) {
                return Err("response_failed");
            }
        }
        if matches!(
            decoder.finish(),
            crate::transport::canonical::CodexCanonicalOutcome::Failed(_)
        ) || decoder
            .response_model()
            .is_some_and(|reported| reported != model.as_str())
        {
            return Err("response_failed_or_model_mismatch");
        }
        state.ok_or("missing_state")
    }
}

fn probe_batch_size(round: usize, cursor: usize, sources: usize) -> usize {
    let requested = if round == 0 {
        1
    } else {
        round.saturating_mul(10).min(MAX_BATCH)
    };
    requested.min(sources.min(MAX_PROBES).saturating_sub(cursor))
}

fn is_state_rejection_code(code: Option<&str>) -> bool {
    // Deliberately narrow local policy. HTTP status and free-form messages are not evidence.
    matches!(
        code,
        Some("invalid_turn_state" | "turn_state_expired" | "turn_state_mismatch")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_core::{
        account::CredentialRevision,
        provider_ports::NoopProviderTurnStatePort,
        routing::{OpenAiTurnStatePolicy, ProviderKind},
    };

    fn managed_account(id: &str) -> ProviderAccount {
        ProviderAccount::new(
            ProviderAccountId::new(format!("acct_{id}")).unwrap(),
            ProviderKind::new("openai").unwrap(),
            "Test".to_owned(),
            Some("test-user".to_owned()),
            "oauth".to_owned(),
            CredentialRevision::new(1).unwrap(),
            None,
        )
        .with_turn_state_injection_enabled(true)
    }

    fn manager() -> (CodexTurnStateManager, UpstreamModelId) {
        let model = UpstreamModelId::new("test-model").unwrap();
        let tuning = RequestTuningHandle::default();
        tuning.publish_openai_turn_state_policy(OpenAiTurnStatePolicy::new(
            true,
            [model.clone()].into(),
        ));
        (
            CodexTurnStateManager::new(Arc::new(NoopProviderTurnStatePort), tuning),
            model,
        )
    }

    #[test]
    fn maintenance_queue_is_fifo_bounded_and_preserves_forced_standby() {
        let queue = MaintenanceQueue::default();
        let key = |index| {
            (
                managed_account(&format!("queue-{index}")).id().clone(),
                UpstreamModelId::new("model-a").unwrap(),
            )
        };
        for index in 0..MAX_PENDING_OBSERVATIONS + 10 {
            queue.enqueue(key(index), false);
        }
        queue.enqueue(key(0), true);
        queue.enqueue(key(0), false);
        assert_eq!(queue.values.lock().unwrap().len(), MAX_PENDING_OBSERVATIONS);
        assert_eq!(queue.pop_available(&HashSet::new()), Some((key(0), true)));
        for index in 1..MAX_PENDING_OBSERVATIONS {
            assert_eq!(
                queue.pop_available(&HashSet::new()),
                Some((key(index), false))
            );
        }
        assert!(queue.pop_available(&HashSet::new()).is_none());
    }

    #[test]
    fn maintenance_queue_skips_only_running_keys_and_keeps_their_followup() {
        let queue = MaintenanceQueue::default();
        let first = (
            managed_account("one").id().clone(),
            UpstreamModelId::new("model-a").unwrap(),
        );
        let other_model = (first.0.clone(), UpstreamModelId::new("model-b").unwrap());
        let other_account = (managed_account("two").id().clone(), first.1.clone());
        queue.enqueue(first.clone(), true);
        queue.enqueue(other_model.clone(), false);
        queue.enqueue(other_account.clone(), false);
        let running = HashSet::from([first.clone()]);
        assert_eq!(queue.pop_available(&running), Some((other_model, false)));
        assert_eq!(queue.pop_available(&running), Some((other_account, false)));
        assert!(queue.pop_available(&running).is_none());
        assert_eq!(queue.pop_available(&HashSet::new()), Some((first, true)));
    }

    #[tokio::test]
    async fn missing_state_blocks_only_opted_in_models_and_disabling_restores_scheduling() {
        let (manager, model) = manager();
        let account = managed_account("gated");
        assert!(!manager.allows_new_request(&account, model.as_str()).await);
        assert!(manager.allows_new_request(&account, "excluded-model").await);
        assert!(
            manager
                .allows_new_request(
                    &account.clone().with_turn_state_injection_enabled(false),
                    model.as_str()
                )
                .await
        );
        manager
            .request_tuning
            .publish_openai_turn_state_policy(OpenAiTurnStatePolicy::default());
        assert!(manager.allows_new_request(&account, model.as_str()).await);
    }

    #[test]
    fn probe_budget_is_500_per_slot_with_final_partial_batch() {
        let mut cursor = 0;
        let mut batches = Vec::new();
        for round in 0..20 {
            let size = probe_batch_size(round, cursor, 2000);
            if size == 0 {
                break;
            }
            batches.push(size);
            cursor += size;
        }
        assert_eq!(batches, [1, 10, 20, 30, 40, 50, 60, 70, 80, 90, 49]);
        assert_eq!(cursor, MAX_PROBES);
        assert_eq!(
            probe_batch_size(0, cursor, 2000),
            0,
            "the slot stops at 500"
        );
        assert_eq!(probe_batch_size(12, 0, 2000), MAX_BATCH);
        assert_eq!(probe_batch_size(5, 498, 2000), 2);
        assert_eq!(probe_batch_size(5, 2, 3), 1);
        assert_eq!(
            probe_batch_size(0, 1, 500),
            1,
            "new slot starts its own batch schedule"
        );
    }

    #[test]
    fn only_explicit_state_rejection_codes_trigger_rotation() {
        for code in [
            "invalid_turn_state",
            "turn_state_expired",
            "turn_state_mismatch",
        ] {
            assert!(is_state_rejection_code(Some(code)));
        }
        for code in [
            None,
            Some("rate_limit_exceeded"),
            Some("server_error"),
            Some("invalid_request_error"),
            Some("401"),
            Some("429"),
            Some("invalid_state"),
        ] {
            assert!(!is_state_rejection_code(code));
        }
    }

    #[tokio::test]
    async fn http_managed_state_is_injected_and_expired_state_is_blocked_before_sending() {
        use crate::transport::{CodexRequestContext, request::apply_managed_turn_state};
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };

        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/codex/responses"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream")
                .set_body_string("event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n"))
            .expect(2).mount(&server).await;
        let client = CodexBackendClient::new(
            reqwest::Client::new(),
            server.uri(),
            crate::OpenAiConfig::default().wire_profile_state(),
        );
        let mut request = CodexResponsesRequest::from_body(
            serde_json::from_value(json!({
                "model": "model-a", "input": "yes", "stream": true, "store": false
            }))
            .unwrap(),
        );
        request.force_http_sse = true;
        let raw = "synthetic-managed-state";
        for expiry in [
            None,
            Some(SystemTime::now() + TURN_STATE_TTL),
            Some(UNIX_EPOCH),
        ] {
            let mut request = request.clone();
            let capture =
                Arc::new(crate::transport::turn_state_capture::TurnStateCapture::default());
            request.turn_state_capture = Some(Arc::clone(&capture));
            if let Some(expiry) = expiry {
                apply_managed_turn_state(&mut request, raw.to_owned(), 1, expiry);
            }
            let mut context = CodexRequestContext::auxiliary(
                "Bearer synthetic",
                Some("synthetic-account"),
                "test-request",
                Some("synthetic-installation"),
            );
            context.turn_state = request.turn_state.as_deref();
            let response = client
                .create_response_stream_with_pool_account(&request, context, Some("acct_synthetic"))
                .await;
            if expiry == Some(UNIX_EPOCH) {
                assert!(matches!(
                    response,
                    Err(crate::transport::CodexClientError::TurnStateUnavailable)
                ));
                assert!(capture.snapshot().is_none());
                continue;
            }
            let mut response = response.unwrap();
            while let Some(chunk) = response.body.next().await {
                chunk.unwrap();
            }
            let evidence = capture.snapshot().unwrap();
            let injected = expiry.is_some_and(|expiry| expiry > SystemTime::now());
            assert_eq!(evidence["summary"]["injected"], injected);
            assert_eq!(
                evidence["injectedState"],
                if injected { json!(raw) } else { Value::Null }
            );
            assert_eq!(evidence["summary"]["transport"], "http_sse");
        }
        let received = server.received_requests().await.unwrap();
        assert!(!received[0].headers.contains_key("x-codex-turn-state"));
        assert_eq!(received[1].headers.get("x-codex-turn-state").unwrap(), raw);
        assert_eq!(
            received.len(),
            2,
            "expired managed state must not reach upstream"
        );
        let decode = |request: &wiremock::Request| {
            let body = if request
                .headers
                .get("content-encoding")
                .is_some_and(|value| value == "zstd")
            {
                zstd::stream::decode_all(request.body.as_slice()).unwrap()
            } else {
                request.body.clone()
            };
            serde_json::from_slice::<Value>(&body).unwrap()
        };
        assert_eq!(
            decode(&received[1]).pointer("/client_metadata/x-codex-turn-state"),
            Some(&Value::String(raw.to_owned()))
        );
        for name in ["user-agent", "originator"] {
            assert_eq!(received[0].headers.get(name), received[1].headers.get(name));
        }
    }

    #[test]
    fn state_usage_capture_is_bounded_redacted_and_request_local() {
        use crate::transport::{CodexBackendTransport, turn_state_capture::TurnStateCapture};
        let raw = format!("synthetic-{}-state", "a".repeat(300));
        let first = TurnStateCapture::default();
        assert!(first.snapshot().is_none());
        first.dispatch(CodexBackendTransport::WebSocket, Some(&raw));
        let before = first.snapshot().unwrap();
        assert_eq!(before["summary"]["returnedChars"], Value::Null);
        assert_eq!(before["summary"]["chars"], raw.len());
        assert_eq!(before["injectedState"], raw);
        assert!(!format!("{first:?}").contains(&raw));
        assert!(!before["summary"].to_string().contains(&raw));
        first.returned(Some(&raw));
        assert_eq!(first.snapshot().unwrap()["summary"]["returnedSame"], true);
        first.returned(Some(&"b".repeat(356)));
        assert_eq!(first.snapshot().unwrap()["summary"]["returnedChars"], 356);
        assert_eq!(first.snapshot().unwrap()["summary"]["returnedSame"], false);
        let second = TurnStateCapture::default();
        second.dispatch(CodexBackendTransport::WebSocket, None);
        assert_eq!(second.snapshot().unwrap()["summary"]["injected"], false);
        assert_eq!(
            second.snapshot().unwrap()["summary"]["returnedChars"],
            Value::Null
        );
        second.dispatch(CodexBackendTransport::HttpSse, Some(&"x".repeat(2049)));
        assert!(second.snapshot().unwrap()["injectedState"].is_null());
    }

    #[test]
    fn state_usage_capture_preserves_existing_observation_at_metadata_limit() {
        use super::super::{
            CodexProviderTransport, ProviderResponseMetadata, ProviderResponseObservation,
            UpstreamTransport, observation::attach_turn_state_snapshot,
        };
        let metadata = ProviderResponseMetadata::new(
            json!({"existing": "x".repeat(32 * 1024 - 20)}).to_string(),
        )
        .unwrap();
        let observation =
            ProviderResponseObservation::new(UpstreamTransport::new("http_sse").unwrap())
                .with_status_code(400)
                .with_provider_metadata(metadata.clone());
        let result = attach_turn_state_snapshot(
            Some(observation),
            Some(json!({"injectedState": "a".repeat(332)})),
            CodexProviderTransport::HttpOnly,
        )
        .unwrap();
        assert_eq!(result.status_code(), Some(400));
        assert_eq!(result.provider_metadata(), Some(&metadata));
    }

    #[tokio::test]
    async fn state_usage_capture_survives_http_rejection_without_logging_raw_values() {
        use crate::transport::{
            CodexRequestContext, request::apply_managed_turn_state,
            turn_state_capture::TurnStateCapture,
        };
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(400)
                    .insert_header("x-codex-turn-state", "synthetic-returned-state")
                    .set_body_json(json!({"error": {"code": "invalid_turn_state"}})),
            )
            .mount(&server)
            .await;
        let client = CodexBackendClient::new(
            reqwest::Client::new(),
            server.uri(),
            crate::OpenAiConfig::default().wire_profile_state(),
        );
        let mut request = CodexResponsesRequest::from_body(
            serde_json::from_value(json!({
                "model": "model-a", "input": "yes", "stream": true
            }))
            .unwrap(),
        );
        request.force_http_sse = true;
        let raw = "synthetic-injected-state";
        apply_managed_turn_state(
            &mut request,
            raw.to_owned(),
            1,
            SystemTime::now() + TURN_STATE_TTL,
        );
        let capture = Arc::new(TurnStateCapture::default());
        request.turn_state_capture = Some(Arc::clone(&capture));
        let mut context =
            CodexRequestContext::auxiliary("Bearer synthetic", None, "req-state", None);
        context.turn_state = request.turn_state.as_deref();
        assert!(
            client
                .create_response_stream_with_pool_account(&request, context, Some("acct-state"))
                .await
                .is_err()
        );
        let observation = super::super::observation::attach_turn_state_snapshot(
            None,
            capture.snapshot(),
            super::super::CodexProviderTransport::PreferWebSocket,
        )
        .unwrap();
        assert_eq!(observation.transport().as_str(), "http_sse");
        assert!(!format!("{observation:?}").contains(raw));
        let metadata: Value =
            serde_json::from_str(observation.provider_metadata().unwrap().as_json()).unwrap();
        assert_eq!(metadata["turnState"]["injectedState"], raw);
        assert_eq!(metadata["turnState"]["summary"]["returnedSame"], false);
    }

    #[tokio::test]
    async fn state_usage_capture_follows_ws_payload_not_connection_handshake() {
        use crate::transport::{
            CodexRequestContext, request::apply_managed_turn_state,
            turn_state_capture::TurnStateCapture,
        };
        use futures::{SinkExt as _, StreamExt as _};
        use tokio::net::TcpListener;
        use tokio_tungstenite::{
            accept_hdr_async_with_config,
            tungstenite::{
                Message,
                extensions::{ExtensionsConfig, compression::deflate::DeflateConfig},
                handshake::server::{Callback, ErrorResponse, Request, Response},
                protocol::WebSocketConfig,
            },
        };
        struct AcceptStateTestWebSocket;
        impl Callback for AcceptStateTestWebSocket {
            fn on_request(
                self,
                _: &Request,
                mut response: Response,
            ) -> Result<Response, ErrorResponse> {
                response.headers_mut().insert(
                    "sec-websocket-extensions",
                    "permessage-deflate".parse().unwrap(),
                );
                Ok(response)
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut extensions = ExtensionsConfig::default();
            extensions.permessage_deflate = Some(DeflateConfig::default());
            let mut config = WebSocketConfig::default();
            config.extensions = extensions;
            let mut socket =
                accept_hdr_async_with_config(socket, AcceptStateTestWebSocket, Some(config))
                    .await
                    .unwrap();
            let message = socket.next().await.unwrap().unwrap();
            let payload: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            socket.send(Message::Text(json!({
                "type": "response.completed", "response": {"id": "resp-state", "status": "completed", "output": []}
            }).to_string().into())).await.unwrap();
            payload
        });
        let client = CodexBackendClient::new(
            reqwest::Client::new(),
            format!("http://{address}"),
            crate::OpenAiConfig::default().wire_profile_state(),
        );
        let mut request = CodexResponsesRequest::from_body(
            serde_json::from_value(json!({
                "model": "model-a", "input": "yes", "stream": true, "store": false
            }))
            .unwrap(),
        );
        request.use_websocket = true;
        let raw = "synthetic-ws-state";
        apply_managed_turn_state(
            &mut request,
            raw.to_owned(),
            1,
            SystemTime::now() + TURN_STATE_TTL,
        );
        let capture = Arc::new(TurnStateCapture::default());
        request.turn_state_capture = Some(Arc::clone(&capture));
        let mut context =
            CodexRequestContext::auxiliary("Bearer synthetic", None, "req-ws-state", None);
        context.turn_state = request.turn_state.as_deref();
        let mut response = client
            .create_response_stream_with_pool_account(&request, context, Some("acct-state"))
            .await
            .unwrap();
        while let Some(chunk) = response.body.next().await {
            chunk.unwrap();
        }
        let payload = server.await.unwrap();
        assert_eq!(
            payload.pointer("/client_metadata/x-codex-turn-state"),
            Some(&json!(raw))
        );
        let evidence = capture.snapshot().unwrap();
        assert_eq!(evidence["injectedState"], raw);
        assert_eq!(evidence["summary"]["transport"], "websocket");
    }

    #[test]
    fn degraded_client_state_without_managed_injection_cannot_rotate_active() {
        let (manager, model) = manager();
        let account = managed_account("account");
        let value = token(1_700_000_000, 176);
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_001);
        manager.observe(&account, &model, None, None, &value, now);
        assert!(manager.pending.values.lock().unwrap().is_empty());
        manager.observe(&account, &model, Some(3), None, &value, now);
        let pending = manager.pending.values.lock().unwrap();
        let TurnStateObservation::Anomaly(anomaly) = pending.values().next().unwrap() else {
            panic!("expected version-fenced anomaly")
        };
        assert_eq!(anomaly.expected_active_version, 3);
        assert_eq!(anomaly.expected_revision, account.revision());
    }

    #[test]
    fn pending_rejection_is_not_erased_by_a_normal_echo_of_the_same_version() {
        let (manager, model) = manager();
        let account = managed_account("account");
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_001);
        let normal = token(1_700_000_000, 160);
        let degraded = token(1_700_000_000, 176);
        manager.observe(&account, &model, Some(3), None, &degraded, now);
        manager.observe(&account, &model, Some(3), None, &normal, now);
        manager.observe(&account, &model, None, None, &normal, now);
        {
            let pending = manager.pending.values.lock().unwrap();
            let TurnStateObservation::Anomaly(anomaly) = pending.values().next().unwrap() else {
                panic!("same-version or unfenced success must not erase a pending rejection");
            };
            assert_eq!(anomaly.expected_active_version, 3);
        }
        manager.observe(&account, &model, Some(4), None, &normal, now);
        manager.observe(&account, &model, Some(3), None, &degraded, now);
        let pending = manager.pending.values.lock().unwrap();
        let TurnStateObservation::Candidate(candidate) = pending.values().next().unwrap() else {
            panic!("an older rejection must not erase a newer version's observation");
        };
        assert_eq!(candidate.expected_active_version, Some(4));
    }

    #[test]
    fn observation_is_bounded_coalesced_and_does_not_write_in_the_request_path() {
        let (manager, model) = manager();
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_002);
        let value = token(1_700_000_000, 160);
        for index in 0..MAX_PENDING_OBSERVATIONS + 10 {
            manager.observe(
                &managed_account(&format!("account-{index}")),
                &model,
                None,
                None,
                &value,
                now,
            );
        }
        assert_eq!(
            manager.pending.values.lock().unwrap().len(),
            MAX_PENDING_OBSERVATIONS
        );
        let updated = token(1_700_000_001, 160);
        manager.observe(
            &managed_account("account-0"),
            &model,
            None,
            None,
            &updated,
            now,
        );
        let pending = manager.pending.values.lock().unwrap();
        assert_eq!(pending.len(), MAX_PENDING_OBSERVATIONS);
        let key = (ProviderAccountId::new("acct_account-0").unwrap(), model);
        let TurnStateObservation::Candidate(candidate) = pending.get(&key).unwrap() else {
            panic!("expected a normal candidate")
        };
        assert_eq!(candidate.value.state().expose_to_provider(), updated);
    }

    #[test]
    fn disabled_or_mismatched_observations_do_not_enter_the_queue() {
        let (manager, model) = manager();
        let account = managed_account("account");
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_002);
        let value = token(1_700_000_000, 160);
        manager.observe(
            &account.clone().with_turn_state_injection_enabled(false),
            &model,
            None,
            None,
            &value,
            now,
        );
        manager.observe(
            &account,
            &UpstreamModelId::new("excluded").unwrap(),
            None,
            None,
            &value,
            now,
        );
        manager.observe(&account, &model, None, Some("different-model"), &value, now);
        for invalid in [
            "unknown".to_owned(),
            token(1_700_000_000, 192),
            token(1_700_001_000, 160),
            token(1_600_000_000, 160),
        ] {
            manager.observe(&account, &model, None, None, &invalid, now);
        }
        manager
            .request_tuning
            .publish_openai_turn_state_policy(OpenAiTurnStatePolicy::default());
        manager.observe(&account, &model, None, None, &value, now);
        assert!(manager.pending.values.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn observer_drains_failed_writes_and_can_be_cancelled() {
        let (manager, model) = manager();
        manager.observe(
            &managed_account("account"),
            &model,
            None,
            None,
            &token(1_700_000_000, 160),
            UNIX_EPOCH + Duration::from_secs(1_700_000_002),
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), manager.run_observations())
                .await
                .is_err()
        );
        assert!(manager.pending.values.lock().unwrap().is_empty());
    }

    #[test]
    fn plan_changes_refresh_both_slots_without_waiting_for_old_expiry() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let value = ProviderTurnStateValue::new(
            OpaqueTurnState::new(token(1_700_000_000, 160)),
            now,
            now + TURN_STATE_TTL,
        );
        let record = ProviderTurnStateRecord::new(
            ProviderAccountId::new("acct_account").unwrap(),
            UpstreamModelId::new("model-a").unwrap(),
            292,
            Some(value.clone()),
            Some(value),
            1,
            ProviderTurnStateRefreshStatus::Ready,
            Some(292),
        );
        assert_eq!(refresh_slots(Some(&record), 292, now), (false, false));
        assert_eq!(refresh_slots(Some(&record), 332, now), (true, true));
        assert_eq!(refresh_slots(None, 292, now), (true, true));
        assert_eq!(
            refresh_slots(Some(&record), 292, now + Duration::from_secs(3000)),
            (true, true)
        );
    }

    fn token(timestamp: u64, cipher_bytes: usize) -> String {
        let mut bytes = Vec::with_capacity(9 + 16 + cipher_bytes + 32);
        bytes.push(0x80);
        bytes.extend_from_slice(&timestamp.to_be_bytes());
        bytes.extend_from_slice(&[0x11; 16]);
        bytes.extend(std::iter::repeat_n(0x22, cipher_bytes));
        bytes.extend_from_slice(&[0x33; 32]);
        URL_SAFE.encode(bytes)
    }

    #[test]
    fn fernet_shape_extracts_timestamp_and_one_hour_expiry() {
        let value = token(1_700_000_000, 160);
        assert_eq!(value.len(), 292);
        let parsed = ParsedCodexTurnState::parse(&value).expect("valid Fernet shape");
        assert_eq!(parsed.encoded_length(), 292);
        assert_eq!(
            parsed
                .expires_at()
                .duration_since(parsed.issued_at())
                .unwrap(),
            TURN_STATE_TTL
        );
        assert_eq!(
            classify_length(INDIVIDUAL_NORMAL_LENGTH, parsed.encoded_length()),
            CodexTurnStateShape::Normal
        );
    }

    #[test]
    fn known_normal_and_degraded_shapes_are_plan_scoped() {
        assert_eq!(token(1_700_000_000, 176).len(), 312);
        assert_eq!(token(1_700_000_000, 192).len(), 332);
        assert_eq!(token(1_700_000_000, 208).len(), 356);
        assert_eq!(
            classify_length(INDIVIDUAL_NORMAL_LENGTH, INDIVIDUAL_DEGRADED_LENGTH),
            CodexTurnStateShape::Degraded
        );
        assert_eq!(
            classify_length(TEAM_NORMAL_LENGTH, TEAM_DEGRADED_LENGTH),
            CodexTurnStateShape::Degraded
        );
        assert_eq!(
            classify_length(TEAM_NORMAL_LENGTH, INDIVIDUAL_NORMAL_LENGTH),
            CodexTurnStateShape::Unexpected
        );
        assert_eq!(expected_normal_length(Some("team")), TEAM_NORMAL_LENGTH);
        assert_eq!(
            expected_normal_length(Some("plus")),
            INDIVIDUAL_NORMAL_LENGTH
        );
    }

    #[test]
    fn malformed_or_non_fernet_values_are_rejected() {
        assert!(ParsedCodexTurnState::parse("turn-state").is_none());
        let mut invalid = vec![0x81];
        invalid.extend_from_slice(&[0; 72]);
        assert!(ParsedCodexTurnState::parse(&URL_SAFE.encode(invalid)).is_none());
    }
}
