//! Account-and-model scoped managed Codex turn state.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::{
    StreamExt as _,
    stream::{self, FuturesUnordered},
};
use gateway_core::{
    account::{CredentialRevision, CredentialState, ProviderAccount, ProviderAccountId},
    provider_ports::{
        OpaqueTurnState, ProviderLeasePort, ProviderTurnStateAnomaly, ProviderTurnStateCandidate,
        ProviderTurnStatePort, ProviderTurnStateProbeCooldown, ProviderTurnStateProbeProgress,
        ProviderTurnStatePromotion, ProviderTurnStateRecord, ProviderTurnStateRefreshStatus,
        ProviderTurnStateSlot, ProviderTurnStateValue,
    },
    routing::UpstreamModelId,
    runtime::RequestTuningHandle,
    upstream::UpstreamSendState,
};
use serde_json::{Map, Value, json};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::{
    credential::{
        CodexAccountFailure, CodexCredentialQuotaService, CodexCredentialRepository,
        CodexCredentialSelector,
    },
    transport::{
        CodexAccountSelectionTelemetry, CodexBackendClient,
        egress::CodexEgressRuntime,
        request::{RequestAccountScope, encode_responses_body, scope_request_to_account},
    },
};

use super::{
    build_cookie_header, codex_request_context, failure::map_client_error,
    observation::synchronize_passive_quota_headers,
};

const TURN_STATE_TTL: Duration = Duration::from_secs(60 * 60);
const INDIVIDUAL_NORMAL_LENGTH: u16 = 292;
const TEAM_NORMAL_LENGTH: u16 = 332;
const MAX_PENDING_OBSERVATIONS: usize = 256;
const STATE_STORE_TIMEOUT: Duration = Duration::from_secs(1);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_COLLECTING_ACCOUNTS: usize = 5;
const MAX_BATCH: usize = 10;
const REFRESH_MARGIN: Duration = Duration::from_secs(15 * 60);
const SWITCH_MARGIN: Duration = Duration::from_secs(60);
const CLOCK_TOLERANCE: Duration = Duration::from_secs(30);

type ObservationKey = (ProviderAccountId, UpstreamModelId);
type ProbeCooldowns = HashMap<ObservationKey, (CredentialRevision, SystemTime)>;

#[derive(Default)]
struct PendingObservations {
    values: Mutex<HashMap<ObservationKey, VecDeque<ProviderTurnStateAnomaly>>>,
    ready: Notify,
}

impl PendingObservations {
    fn enqueue(&self, key: ObservationKey, observation: ProviderTurnStateAnomaly) {
        let mut values = self.values.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(current) = values.get_mut(&key) {
            let Some(last) = current.back() else { return };
            let incoming = (
                observation.expected_revision.get(),
                observation.expected_active_version,
            );
            let previous = (last.expected_revision.get(), last.expected_active_version);
            if incoming < previous {
                return;
            }
            if incoming > previous {
                current.clear();
            } else if current.iter().any(|item| item.promote_standby)
                || (current.len() == 2 && current.iter().all(|item| item.suspect))
            {
                return;
            }
            // Once two consecutive strikes arrive, a later echo cannot erase that decision.
            if current.len() == 2 {
                current.pop_front();
            }
            current.push_back(observation);
        } else if values.len() < MAX_PENDING_OBSERVATIONS {
            values.insert(key, VecDeque::from([observation]));
        } else {
            return;
        }
        self.ready.notify_one();
    }
}

#[derive(Default)]
struct MaintenanceQueue {
    values: Mutex<VecDeque<(ObservationKey, bool)>>,
    followups: Mutex<HashMap<ObservationKey, bool>>,
    ready: Notify,
}

impl MaintenanceQueue {
    fn enqueue(&self, key: ObservationKey, force_standby: bool) -> bool {
        let mut followups = self.followups.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(force) = followups.get_mut(&key) {
            *force |= force_standby;
            return true;
        }
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
        let mut followups = self.followups.lock().unwrap_or_else(|e| e.into_inner());
        let mut values = self.values.lock().unwrap_or_else(|e| e.into_inner());
        let accounts = running.iter().map(|key| &key.0).collect::<HashSet<_>>();
        let index = values.iter().position(|(key, _)| {
            !running.contains(key)
                && (accounts.contains(&key.0) || accounts.len() < MAX_COLLECTING_ACCOUNTS)
        })?;
        let next = values.remove(index)?;
        followups.insert(next.0.clone(), false);
        Some(next)
    }

    fn finish(&self, key: &ObservationKey) {
        let force = self
            .followups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
        if force == Some(true) {
            self.enqueue(key.clone(), true);
        }
    }

    fn reset_running(&self) {
        self.followups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParsedCodexTurnState {
    issued_at: SystemTime,
    expires_at: SystemTime,
    cipher_blocks: usize,
}

impl ParsedCodexTurnState {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.len() > 2048 || value.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return None;
        }
        let core = value.trim_end_matches('=');
        if value.len() - core.len() > 2 {
            return None;
        }
        let bytes = URL_SAFE_NO_PAD.decode(core).ok()?;
        if bytes.first().copied() != Some(0x80) || bytes.len() < 9 + 16 + 16 + 32 {
            return None;
        }
        let timestamp = u64::from_be_bytes(bytes.get(1..9)?.try_into().ok()?);
        if !(1_577_836_800..4_102_444_800).contains(&timestamp) {
            return None;
        }
        let cipher_bytes = bytes.len().checked_sub(9 + 16 + 32)?;
        if cipher_bytes == 0 || cipher_bytes % 16 != 0 {
            return None;
        }
        let issued_at = UNIX_EPOCH.checked_add(Duration::from_secs(timestamp))?;
        let expires_at = issued_at.checked_add(TURN_STATE_TTL)?;
        Some(Self {
            issued_at,
            expires_at,
            cipher_blocks: cipher_bytes / 16,
        })
    }

    fn normal_for(self, normal_length: u16) -> bool {
        self.cipher_blocks
            == if normal_length == TEAM_NORMAL_LENGTH {
                12
            } else {
                10
            }
    }

    pub(crate) const fn issued_at(self) -> SystemTime {
        self.issued_at
    }

    pub(crate) const fn expires_at(self) -> SystemTime {
        self.expires_at
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
        let record = self.record(account, model).await?;
        if record.normal_length() != expected_normal_length(account.plan_type()) {
            return None;
        }
        self.retire_old_pools(&record);
        let active = record
            .active()?
            .is_valid_at(now)
            .then_some(record.active()?)?;
        if active.expires_at() <= now + SWITCH_MARGIN {
            return None;
        }
        Some((
            active.state().expose_to_provider().to_owned(),
            record.state_version(),
            active.expires_at().checked_sub(SWITCH_MARGIN)?,
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
        tokio::time::timeout(STATE_STORE_TIMEOUT, self.read_and_promote(account, model))
            .await
            .ok()
            .flatten()
    }

    async fn read_and_promote(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
    ) -> Option<ProviderTurnStateRecord> {
        let record = tokio::time::timeout(
            STATE_STORE_TIMEOUT,
            self.store
                .read(account.id(), model, account.turn_state_binding_revision()),
        )
        .await
        .ok()?
        .ok()
        .flatten()?;
        let now = SystemTime::now();
        if record
            .active()
            .is_none_or(|value| value.expires_at() <= now + SWITCH_MARGIN)
            && record.standby().is_some_and(|value| {
                value.is_valid_at(now) && value.expires_at() > now + SWITCH_MARGIN
            })
            && let Ok(Ok(Some(promoted))) = tokio::time::timeout(
                STATE_STORE_TIMEOUT,
                self.store.promote_standby(ProviderTurnStatePromotion {
                    account_id: account.id().clone(),
                    expected_revision: account.turn_state_binding_revision(),
                    expected_active_version: record.state_version(),
                    upstream_model: model.clone(),
                    normal_length: expected_normal_length(account.plan_type()),
                    observed_at: now,
                    minimum_remaining: SWITCH_MARGIN,
                }),
            )
            .await
        {
            self.retire_old_pools(&promoted);
            return Some(promoted);
        }
        Some(record)
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
                account.turn_state_binding_revision(),
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
        _reported_model: Option<&str>,
        value: &str,
        observed_at: SystemTime,
    ) {
        if !self.feature_enabled_for(account, requested_model) || injected_version.is_none() {
            return;
        }
        let normal_length = expected_normal_length(account.plan_type());
        let healthy = ParsedCodexTurnState::parse(value).is_some_and(|parsed| {
            parsed.normal_for(normal_length)
                && parsed.issued_at() <= observed_at + CLOCK_TOLERANCE
                && parsed.expires_at() > observed_at + CLOCK_TOLERANCE
        });
        let observation = ProviderTurnStateAnomaly {
            account_id: account.id().clone(),
            expected_revision: account.turn_state_binding_revision(),
            expected_active_version: injected_version.unwrap_or_default(),
            upstream_model: requested_model.clone(),
            normal_length,
            observed_length: u16::try_from(value.trim().len())
                .ok()
                .filter(|len| (1..=4096).contains(len)),
            suspect: !healthy,
            promote_standby: false,
            observed_at,
        };
        let key = (account.id().clone(), requested_model.clone());
        self.pending.enqueue(key, observation);
    }

    pub(crate) fn observe_failure(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
        injected_version: Option<u64>,
        _reported_model: Option<&str>,
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
        if !self.feature_enabled_for(account, model) || !is_state_rejection_code(code) {
            return;
        }
        let observation = ProviderTurnStateAnomaly {
            account_id: account.id().clone(),
            expected_revision: account.turn_state_binding_revision(),
            expected_active_version,
            upstream_model: model.clone(),
            normal_length: expected_normal_length(account.plan_type()),
            observed_length: None,
            suspect: true,
            promote_standby: true,
            observed_at: SystemTime::now(),
        };
        let key = (account.id().clone(), model.clone());
        self.pending.enqueue(key, observation);
    }

    pub(crate) async fn run_observations(&self) {
        loop {
            self.pending.ready.notified().await;
            loop {
                let next = {
                    let mut pending = self
                        .pending
                        .values
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    pending.keys().next().cloned().and_then(|key| {
                        let observation = pending.get_mut(&key)?.pop_front()?;
                        if pending.get(&key).is_some_and(VecDeque::is_empty) {
                            pending.remove(&key);
                        }
                        Some((key, observation))
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
                    let revision = observation.expected_revision;
                    let before = self
                        .store
                        .read(&account_id, &model, revision)
                        .await
                        .ok()
                        .flatten();
                    let result = self.store.record_anomaly(observation).await;
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
        expected_version: u64,
        observed_at: SystemTime,
    ) -> bool {
        let value = value.trim().to_owned();
        let Some(parsed) = ParsedCodexTurnState::parse(&value) else {
            return false;
        };
        if parsed.issued_at() > observed_at + CLOCK_TOLERANCE || !parsed.normal_for(normal_length) {
            return false;
        }
        if parsed.expires_at() <= observed_at + SWITCH_MARGIN {
            return false;
        }
        let current = self.record(account, &model).await;
        if current
            .as_ref()
            .map_or(0, ProviderTurnStateRecord::state_version)
            != expected_version
        {
            return false;
        }
        let (active_due, next_due) = refresh_slots(current.as_ref(), normal_length, observed_at);
        if !active_due && !next_due {
            return false;
        }
        let slot = if active_due {
            ProviderTurnStateSlot::Active
        } else {
            ProviderTurnStateSlot::Standby
        };
        if slot == ProviderTurnStateSlot::Standby
            && current
                .as_ref()
                .and_then(ProviderTurnStateRecord::active)
                .is_some_and(|active| parsed.expires_at() <= active.expires_at())
        {
            return false;
        }
        if current.as_ref().is_some_and(|record| {
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
                expected_revision: account.turn_state_binding_revision(),
                expected_active_version: Some(expected_version),
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
        Some(
            "team" | "business" | "self_serve_business_prolite" | "self_serve_business_usage_based",
        ) => TEAM_NORMAL_LENGTH,
        _ => INDIVIDUAL_NORMAL_LENGTH,
    }
}

pub(crate) fn needs_refresh(record: &ProviderTurnStateRecord, now: SystemTime) -> bool {
    record
        .active()
        .is_none_or(|active| match active.expires_at().duration_since(now) {
            Ok(remaining) => remaining <= REFRESH_MARGIN,
            Err(_) => true,
        })
}

fn standby_needs_refresh(record: &ProviderTurnStateRecord, now: SystemTime) -> bool {
    needs_refresh(record, now)
        && record.standby().is_none_or(|standby| {
            !standby.is_valid_at(now)
                || standby.expires_at() <= now + SWITCH_MARGIN
                || record
                    .active()
                    .is_some_and(|active| standby.expires_at() <= active.expires_at())
        })
}

fn refresh_slots(
    record: Option<&ProviderTurnStateRecord>,
    normal_length: u16,
    now: SystemTime,
) -> (bool, bool) {
    match record {
        Some(record) if record.normal_length() == normal_length => {
            let missing = record.active().is_none_or(|active| {
                !active.is_valid_at(now) || active.expires_at() <= now + SWITCH_MARGIN
            });
            (missing, !missing && standby_needs_refresh(record, now))
        }
        _ => (true, false),
    }
}

#[derive(Clone)]
pub(crate) struct CodexTurnStateMaintenanceService {
    repository: CodexCredentialRepository,
    client: CodexBackendClient,
    egress: Option<Arc<CodexEgressRuntime>>,
    manager: CodexTurnStateManager,
    selector: Arc<CodexCredentialSelector>,
    quota: Arc<CodexCredentialQuotaService>,
    response_origin: url::Url,
    discovery_cursor: Arc<AtomicUsize>,
    probe_cooldowns: Arc<Mutex<ProbeCooldowns>>,
}

impl CodexTurnStateMaintenanceService {
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        repository: CodexCredentialRepository,
        _leases: Arc<dyn ProviderLeasePort>,
        client: CodexBackendClient,
        egress: Option<Arc<CodexEgressRuntime>>,
        manager: CodexTurnStateManager,
        selector: Arc<CodexCredentialSelector>,
        quota: Arc<CodexCredentialQuotaService>,
        response_origin: url::Url,
    ) -> Self {
        Self {
            repository,
            client,
            egress,
            manager,
            selector,
            quota,
            response_origin,
            discovery_cursor: Arc::default(),
            probe_cooldowns: Arc::default(),
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
        if self.egress.is_none() && self.probe_proxy().is_none() {
            return;
        }
        let Ok(accounts) = self.repository.list_for_provider().await else {
            return;
        };
        self.enqueue_accounts(&accounts, false).await;
    }

    pub(crate) async fn accounts_changed(&self, account_ids: &[ProviderAccountId]) {
        if !self
            .manager
            .request_tuning
            .openai_turn_state_policy()
            .enabled()
            || (self.egress.is_none() && self.probe_proxy().is_none())
        {
            return;
        }
        let mut accounts = Vec::with_capacity(account_ids.len());
        for id in account_ids {
            if let Ok(Ok(Some(account))) =
                tokio::time::timeout(STATE_STORE_TIMEOUT, self.repository.store().get_account(id))
                    .await
            {
                accounts.push(account);
            }
        }
        self.enqueue_accounts(&accounts, true).await;
    }

    async fn enqueue_accounts(&self, accounts: &[ProviderAccount], wake_running: bool) {
        let models = self.manager.models();
        let mut targets = Vec::new();
        for account in accounts {
            if !account.enabled()
                || !account.turn_state_injection_enabled()
                || !maintenance_credential_ready(account, SystemTime::now())
            {
                continue;
            }
            if !matches!(
                tokio::time::timeout(
                    STATE_STORE_TIMEOUT,
                    self.quota.rate_limited_until(account.id())
                )
                .await,
                Ok(Ok(None))
            ) {
                continue;
            }
            for model in &models {
                if !self.probe_ready(account, model).await {
                    continue;
                }
                let record = self.manager.record(account, model).await;
                let (active, next) = refresh_slots(
                    record.as_ref(),
                    expected_normal_length(account.plan_type()),
                    SystemTime::now(),
                );
                if active || next {
                    targets.push((account.id().clone(), model.clone()));
                    if record.as_ref().is_none_or(|record| {
                        !matches!(
                            record.refresh_status(),
                            ProviderTurnStateRefreshStatus::Refreshing
                                | ProviderTurnStateRefreshStatus::Queued
                        )
                    }) {
                        self.manager
                            .mark_refresh_status(
                                account,
                                model,
                                expected_normal_length(account.plan_type()),
                                ProviderTurnStateRefreshStatus::Queued,
                                SystemTime::now(),
                            )
                            .await;
                    }
                }
            }
        }
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
                .enqueue(targets[index].clone(), wake_running)
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
        // A restarted daemon must not inherit deduplication ownership of dropped futures.
        self.manager.maintenance.reset_running();
        let mut running = HashSet::new();
        let mut tasks = FuturesUnordered::new();
        loop {
            while let Some((key, force)) = self.manager.maintenance.pop_available(&running) {
                running.insert(key.clone());
                tasks.push(async move {
                    self.run_target(&key.0, &key.1, force).await;
                    key
                });
            }
            tokio::select! {
                () = self.manager.maintenance.ready.notified() => {},
                Some(key) = tasks.next(), if !tasks.is_empty() => {
                    running.remove(&key);
                    self.manager.maintenance.finish(&key);
                }
            }
        }
    }

    async fn run_target(
        &self,
        account_id: &ProviderAccountId,
        model: &UpstreamModelId,
        force: bool,
    ) {
        let proxy = self.probe_proxy();
        if self.egress.is_none() && proxy.is_none() {
            return;
        }
        let Ok(Ok(Some(account))) = tokio::time::timeout(
            STATE_STORE_TIMEOUT,
            self.repository.store().get_account(account_id),
        )
        .await
        else {
            return;
        };
        if !self.target_current(&account, model).await {
            return;
        }
        tokio::select! {
            () = self.maintain_target(&account, model, proxy.as_ref(), force) => {},
            () = self.wait_until_invalid(&account, model, proxy.as_ref()) => {},
        }
        if matches!(
            tokio::time::timeout(
                STATE_STORE_TIMEOUT,
                self.quota.rate_limited_until(account.id())
            )
            .await,
            Ok(Ok(Some(_)))
        ) {
            self.manager
                .mark_refresh_status(
                    &account,
                    model,
                    expected_normal_length(account.plan_type()),
                    ProviderTurnStateRefreshStatus::Cooldown,
                    SystemTime::now(),
                )
                .await;
        }
        // A batch-boundary check can observe cancellation before the watcher does.
        // The store only closes a still-refreshing row for this binding generation.
        let _ = tokio::time::timeout(
            STATE_STORE_TIMEOUT,
            self.manager.store.cancel_refresh(
                account.id(),
                model,
                account.turn_state_binding_revision(),
            ),
        )
        .await;
        if self.probe_proxy().as_ref() != proxy.as_ref() {
            self.enqueue_accounts(&[account], true).await;
        }
    }

    fn probe_proxy(&self) -> Option<gateway_core::account::OutboundProxy> {
        self.manager
            .request_tuning
            .openai_turn_state_policy()
            .probe_proxy()
            .cloned()
    }

    async fn target_current(&self, account: &ProviderAccount, model: &UpstreamModelId) -> bool {
        self.current_target(account, model).await.is_some()
    }

    async fn current_target(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
    ) -> Option<ProviderAccount> {
        if !self.manager.feature_enabled_for(account, model)
            || !matches!(
                tokio::time::timeout(
                    STATE_STORE_TIMEOUT,
                    self.quota.rate_limited_until(account.id())
                )
                .await,
                Ok(Ok(None))
            )
        {
            return None;
        }
        tokio::time::timeout(
            STATE_STORE_TIMEOUT,
            self.repository.store().get_account(account.id()),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .filter(|current| {
            current.turn_state_binding_revision() == account.turn_state_binding_revision()
                && current.plan_type() == account.plan_type()
                && self.manager.feature_enabled_for(current, model)
                && maintenance_credential_ready(current, SystemTime::now())
        })
    }

    async fn wait_until_invalid(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
        proxy: Option<&gateway_core::account::OutboundProxy>,
    ) {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if self.probe_proxy().as_ref() != proxy || !self.target_current(account, model).await {
                return;
            }
        }
    }

    async fn maintain_target(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
        proxy: Option<&gateway_core::account::OutboundProxy>,
        _force_refresh: bool,
    ) {
        let now = SystemTime::now();
        let record = self.manager.record(account, model).await;
        let normal_length = expected_normal_length(account.plan_type());
        let (active_due, standby_due) = refresh_slots(record.as_ref(), normal_length, now);
        if !active_due && !standby_due {
            return;
        }
        self.collect_slot(account, model, normal_length, proxy)
            .await;
    }

    async fn collect_slot(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
        normal_length: u16,
        proxy: Option<&gateway_core::account::OutboundProxy>,
    ) -> bool {
        if !self.target_current(account, model).await
            || !self.probe_ready(account, model).await
            || !self
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
        let mut round = 0;
        let mut attempted = 0_u64;
        let mut returned_length = None;
        let mut runtime = None;
        let mut failures = HashMap::<&'static str, usize>::new();
        loop {
            if self.probe_proxy().as_ref() != proxy {
                return false;
            }
            let Some(current_account) = self.current_target(account, model).await else {
                return false;
            };
            if runtime.as_ref().is_none_or(
                |(revision, _): &(
                    gateway_core::account::CredentialRevision,
                    Arc<crate::credential::CodexRuntimeCredential>,
                )| *revision != current_account.revision(),
            ) {
                let Ok(Ok(current_runtime)) = tokio::time::timeout(
                    STATE_STORE_TIMEOUT,
                    self.repository.load_runtime_credential(&current_account),
                )
                .await
                else {
                    return false;
                };
                runtime = Some((current_account.revision(), Arc::new(current_runtime)));
            }
            let record = self.manager.record(account, model).await;
            let (active_due, next_due) =
                refresh_slots(record.as_ref(), normal_length, SystemTime::now());
            if !active_due && !next_due {
                return true;
            }
            let slot = if active_due {
                ProviderTurnStateSlot::Active
            } else {
                ProviderTurnStateSlot::Standby
            };
            let batch_size = probe_batch_size(round);
            round = round.saturating_add(1);
            let Some((_, runtime)) = runtime.as_ref() else {
                return false;
            };
            let probe_account = &current_account;
            let sent = AtomicUsize::new(0);
            let sent_ref = &sent;
            let before_batch = attempted;
            let results = stream::iter((0..batch_size).map(|_| async move {
                if self.probe_proxy().as_ref() != proxy
                    || !self.target_current(probe_account, model).await
                {
                    return Err("account_stopped");
                }
                let client = match proxy {
                    Some(proxy) => self.client.for_probe_proxy(proxy),
                    None => {
                        let source = self
                            .egress
                            .as_ref()
                            .ok_or("egress_unavailable")?
                            .next_probe_source()
                            .map_err(|_| "egress_unavailable")?;
                        self.client.for_probe_source(probe_account, source)
                    }
                }
                .map_err(|_| "egress_unavailable")?;
                let attempt = before_batch
                    .saturating_add(sent_ref.fetch_add(1, Ordering::Relaxed) as u64 + 1);
                tokio::time::timeout(
                    PROBE_TIMEOUT,
                    self.probe(
                        probe_account.clone(),
                        model.clone(),
                        Arc::clone(runtime),
                        client,
                    ),
                )
                .await
                .unwrap_or(Err("timeout"))
                .map(|value| (attempt, value))
            }))
            .buffer_unordered(batch_size);
            tokio::pin!(results);
            let mut last_reason = None;
            let mut progress_tick = tokio::time::interval(Duration::from_secs(1));
            progress_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            progress_tick.tick().await;
            let mut reported = before_batch;
            loop {
                let value = tokio::select! {
                    value = results.next() => {
                        let Some(value) = value else { break };
                        value
                    },
                    _ = progress_tick.tick() => {
                        let count = before_batch.saturating_add(sent.load(Ordering::Relaxed) as u64);
                        if count != reported {
                            self.probe_progress(account, model, ProviderTurnStateProbeProgress {
                                attempts: count, reason: last_reason, successful_attempt: None, returned_length,
                            }).await;
                            reported = count;
                        }
                        continue;
                    }
                };
                attempted = before_batch.saturating_add(sent.load(Ordering::Relaxed) as u64);
                let (attempt, value) = match value {
                    Ok(value) => value,
                    Err(reason) => {
                        *failures.entry(reason).or_default() += 1;
                        last_reason = Some(reason);
                        if matches!(
                            reason,
                            "account_rejected" | "account_stopped" | "probe_rate_limited"
                        ) {
                            self.probe_progress(
                                account,
                                model,
                                ProviderTurnStateProbeProgress {
                                    attempts: attempted,
                                    reason: last_reason,
                                    successful_attempt: None,
                                    returned_length,
                                },
                            )
                            .await;
                            return false;
                        }
                        continue;
                    }
                };
                if self.probe_proxy().as_ref() != proxy
                    || !self.target_current(account, model).await
                {
                    return false;
                }
                returned_length = u16::try_from(value.len()).ok();
                let Some(parsed) = ParsedCodexTurnState::parse(&value) else {
                    last_reason = Some("invalid_envelope");
                    *failures.entry("invalid_envelope").or_default() += 1;
                    continue;
                };
                let invalid = if !parsed.normal_for(normal_length) {
                    Some("unexpected_shape")
                } else if parsed.issued_at() > SystemTime::now() + CLOCK_TOLERANCE {
                    Some("future_state")
                } else if parsed.expires_at() <= SystemTime::now() + SWITCH_MARGIN {
                    Some("insufficient_lifetime")
                } else {
                    None
                };
                if let Some(reason) = invalid {
                    last_reason = Some(reason);
                    *failures.entry(reason).or_default() += 1;
                    continue;
                }
                if self
                    .manager
                    .put_probe_candidate(
                        account,
                        model.clone(),
                        normal_length,
                        value,
                        record
                            .as_ref()
                            .map_or(0, ProviderTurnStateRecord::state_version),
                        SystemTime::now(),
                    )
                    .await
                {
                    self.probe_progress(
                        account,
                        model,
                        ProviderTurnStateProbeProgress {
                            attempts: attempted,
                            reason: None,
                            successful_attempt: Some(attempt),
                            returned_length,
                        },
                    )
                    .await;
                    tracing::info!(
                        account_id = account.id().as_str(),
                        model = model.as_str(),
                        ?slot,
                        attempted,
                        successful_attempt = attempt,
                        "Turn state acquisition succeeded"
                    );
                    return true;
                }
                last_reason = Some("duplicate_or_write_conflict");
                *failures.entry("duplicate_or_write_conflict").or_default() += 1;
            }
            self.probe_progress(
                account,
                model,
                ProviderTurnStateProbeProgress {
                    attempts: attempted,
                    reason: last_reason,
                    successful_attempt: None,
                    returned_length,
                },
            )
            .await;
            if attempted % 100 < batch_size as u64 {
                tracing::info!(
                    account_id = account.id().as_str(),
                    model = model.as_str(),
                    ?slot,
                    attempted,
                    ?failures,
                    returned_length,
                    "Turn state acquisition continues"
                );
            }
            if sent.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }

    async fn probe_progress(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
        progress: ProviderTurnStateProbeProgress,
    ) {
        let _ = tokio::time::timeout(
            STATE_STORE_TIMEOUT,
            self.manager.store.record_probe_progress(
                account.id(),
                model,
                account.turn_state_binding_revision(),
                progress,
            ),
        )
        .await;
    }

    async fn probe_ready(&self, account: &ProviderAccount, model: &UpstreamModelId) -> bool {
        {
            let cooldowns = self
                .probe_cooldowns
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if cooldowns
                .get(&(account.id().clone(), model.clone()))
                .is_some_and(|(revision, until)| {
                    *revision == account.turn_state_binding_revision() && *until > SystemTime::now()
                })
            {
                return false;
            }
        }
        matches!(
            tokio::time::timeout(
                STATE_STORE_TIMEOUT,
                self.manager.store.read_probe_cooldown(
                    account.id(),
                    model,
                    account.turn_state_binding_revision(),
                ),
            )
            .await,
            Ok(Ok(None))
        )
    }

    async fn probe(
        &self,
        account: ProviderAccount,
        model: UpstreamModelId,
        runtime: Arc<crate::credential::CodexRuntimeCredential>,
        client: CodexBackendClient,
    ) -> Result<String, &'static str> {
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
        // The fixed maintenance prompt has no location/tool fields or client metadata.
        let mut request = encode_responses_body(body, model.as_str(), None);
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
        let mut response = match client.probe_turn_state_response(&request, context).await {
            Ok(response) => response,
            Err(error) => {
                let reason = probe_error_reason(&error);
                let failure = map_client_error(error, UpstreamSendState::Ambiguous, false);
                return Err(self
                    .observe_probe_failure(&account, &model, failure, reason)
                    .await);
            }
        };
        synchronize_passive_quota_headers(&self.quota, &account, &response.rate_limit_headers)
            .await;
        let state = response.turn_state.take();
        let mut decoder = super::turn_state_probe_response::ProbeResponse::default();
        while let Some(chunk) = response.body.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    let failure = map_client_error(error, UpstreamSendState::Ambiguous, false);
                    return Err(self
                        .observe_probe_failure(&account, &model, failure, "stream_error")
                        .await);
                }
            };
            if let Err(error) = decoder.push(&chunk) {
                let failure = super::failure::map_canonical_error(
                    error,
                    &response.diagnostics,
                    &response.set_cookie_headers,
                    &response.rate_limit_headers,
                    super::failure::ReplayBoundary::BeforeSemanticOutput,
                );
                return Err(self
                    .observe_probe_failure(&account, &model, failure, "stream_failed")
                    .await);
            }
        }
        let completed = match decoder.finish() {
            Ok(completed) => completed,
            Err(error) => {
                let failure = super::failure::map_canonical_error(
                    error,
                    &response.diagnostics,
                    &response.set_cookie_headers,
                    &response.rate_limit_headers,
                    super::failure::ReplayBoundary::BeforeSemanticOutput,
                );
                return Err(self
                    .observe_probe_failure(&account, &model, failure, "stream_failed")
                    .await);
            }
        };
        // Account rejection must be recorded before a Cookie CAS advances the revision.
        self.observe_response_cookies(&account, &response.set_cookie_headers)
            .await;
        if !completed {
            return Err("missing_completed");
        }
        state.ok_or("missing_state")
    }

    async fn observe_probe_failure(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
        failure: super::failure::MappedProviderFailure,
        reason: &'static str,
    ) -> &'static str {
        if let Some(CodexAccountFailure::RateLimited { retry_after }) = failure.account_failure {
            let delay = retry_after
                .unwrap_or_else(|| {
                    Duration::from_secs(
                        self.manager
                            .request_tuning
                            .load()
                            .rate_limit_cooldown_seconds,
                    )
                })
                .min(Duration::from_secs(u64::from(u32::MAX)));
            let now = SystemTime::now();
            // The probe's 429 is not evidence that sibling models or business traffic failed.
            let cooldown = ProviderTurnStateProbeCooldown {
                until: now.checked_add(delay).unwrap_or(now),
                http_status: failure.error.upstream_status(),
                retry_from_upstream: retry_after.is_some(),
                error_code: safe_probe_error_code(
                    failure.error.upstream_code().map(|code| code.as_str()),
                ),
            };
            // A supplemental diagnostics write can fail; still honor this process's Retry-After.
            {
                let mut cooldowns = self
                    .probe_cooldowns
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                cooldowns.retain(|_, (_, until)| *until > now);
                let incoming = (account.turn_state_binding_revision(), cooldown.until);
                let saved = cooldowns
                    .entry((account.id().clone(), model.clone()))
                    .or_insert(incoming);
                if saved.0 < incoming.0 {
                    *saved = incoming;
                } else if saved.0 == incoming.0 {
                    saved.1 = saved.1.max(incoming.1);
                }
            }
            let recorded = matches!(
                tokio::time::timeout(
                    STATE_STORE_TIMEOUT,
                    self.manager.store.record_probe_cooldown(
                        account.id(),
                        model,
                        account.turn_state_binding_revision(),
                        cooldown,
                    ),
                )
                .await,
                Ok(Ok(()))
            );
            tracing::warn!(
                account_id = account.id().as_str(),
                model = model.as_str(),
                status = cooldown.http_status,
                error_code = cooldown.error_code,
                retry_after_seconds = delay.as_secs(),
                retry_from_upstream = cooldown.retry_from_upstream,
                recorded,
                "Turn state probe model entered cooldown"
            );
            return "probe_rate_limited";
        }
        // Only account facts belong here, not business scoring or session exclusions.
        if let Some(account_failure) = failure.account_failure.filter(|failure| {
            matches!(
                failure,
                CodexAccountFailure::CredentialExpired
                    | CodexAccountFailure::CredentialRevoked
                    | CodexAccountFailure::IdentityVerificationRequired
                    | CodexAccountFailure::Banned
                    | CodexAccountFailure::QuotaExhausted
                    | CodexAccountFailure::UsageLimitExhausted { .. }
            )
        }) {
            // Routine Cookie writes may advance the material CAS while a probe is in flight.
            // Reload only the same identity generation; an old rejection cannot affect a relogin.
            let current = tokio::time::timeout(
                STATE_STORE_TIMEOUT,
                self.repository.store().get_account(account.id()),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .filter(|current| {
                current.turn_state_binding_revision() == account.turn_state_binding_revision()
            });
            let recorded = if let Some(current) = current {
                self.selector
                    .record_failure(&current, account_failure, failure.error_message)
                    .await
                    .is_ok()
            } else {
                false
            };
            tracing::warn!(
                account_id = account.id().as_str(),
                model = model.as_str(),
                status = failure.error.upstream_status(),
                reason,
                recorded,
                "Turn state acquisition stopped after account rejection"
            );
            return "account_rejected";
        }
        synchronize_passive_quota_headers(&self.quota, account, &failure.rate_limit_headers).await;
        if failure.capture_response_cookies {
            self.observe_response_cookies(account, &failure.set_cookie_headers)
                .await;
        }
        reason
    }

    async fn observe_response_cookies(&self, account: &ProviderAccount, headers: &[String]) {
        // Like business responses, a concurrent Cookie save is supplemental.
        // The caller and store still fence State writes by current binding and eligibility.
        if let Err(error) = self
            .selector
            .capture_response_cookies(account, &self.response_origin, headers)
            .await
        {
            tracing::debug!(
                account_id = account.id().as_str(),
                error = %error,
                "Turn state probe response Cookie observation skipped"
            );
        }
    }
}

fn safe_probe_error_code(code: Option<&str>) -> Option<&'static str> {
    match code? {
        "rate_limit_exceeded" => Some("rate_limit_exceeded"),
        "rate_limit_reached" => Some("rate_limit_reached"),
        "rate_limit_error" => Some("rate_limit_error"),
        _ => Some("other"),
    }
}

fn maintenance_credential_ready(account: &ProviderAccount, now: SystemTime) -> bool {
    account.credential_state() == CredentialState::Ready
        && account
            .access_token_expires_at()
            .is_none_or(|expiry| expiry > now)
        && !account.quota().is_exhausted()
}

fn probe_error_reason(error: &crate::transport::CodexClientError) -> &'static str {
    match error {
        crate::transport::CodexClientError::Upstream { status, .. } => match status.as_u16() {
            400 => "upstream_400",
            401 => "upstream_401",
            402 => "upstream_402",
            403 => "upstream_403",
            404 => "upstream_404",
            429 => "upstream_429",
            500..=599 => "upstream_5xx",
            _ => "upstream_error",
        },
        _ => "transport_error",
    }
}
fn probe_batch_size(round: usize) -> usize {
    match round {
        0 => 1,
        1 => 5,
        _ => MAX_BATCH,
    }
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
    use crate::transport::protocol::responses::CodexResponsesRequest;
    use base64::engine::general_purpose::URL_SAFE;
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

    #[test]
    fn running_keys_cannot_fill_the_pending_queue_and_forced_followups_survive() {
        let queue = MaintenanceQueue::default();
        let mut running = HashSet::new();
        for index in 0..MAX_PENDING_OBSERVATIONS + 64 {
            let key = (
                managed_account(&format!("running-{}", index % MAX_COLLECTING_ACCOUNTS))
                    .id()
                    .clone(),
                UpstreamModelId::new(format!("model-{index}")).unwrap(),
            );
            assert!(queue.enqueue(key.clone(), false));
            assert_eq!(queue.pop_available(&running), Some((key.clone(), false)));
            running.insert(key);
        }
        for key in &running {
            assert!(queue.enqueue(key.clone(), true));
        }
        assert!(queue.values.lock().unwrap().is_empty());
        let key = running.iter().next().unwrap().clone();
        running.remove(&key);
        queue.finish(&key);
        assert_eq!(queue.pop_available(&running), Some((key, true)));
        queue.reset_running();
        assert!(queue.followups.lock().unwrap().is_empty());
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
    fn probe_batches_are_capped_at_ten() {
        let expected = [1, 5, 10, 10, 10, 10, 10, 10, 10, 10, 10];
        for (round, expected) in expected.into_iter().enumerate() {
            let size = probe_batch_size(round);
            assert_eq!(size, expected);
        }
        assert!((0..1000).all(|round| (1..=10).contains(&probe_batch_size(round))));
        assert_eq!(probe_batch_size(100), 10);
        assert_eq!(probe_batch_size(usize::MAX), 10);
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
    async fn full_probe_keeps_success_body_open_and_bounds_authentication_errors() {
        use crate::transport::CodexRequestContext;
        use tokio::{
            io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
            net::TcpListener,
        };

        for (status, framing) in [
            (200, "Content-Length: 100000"),
            (204, "Content-Length: 100000"),
            (401, "Content-Length: 100000"),
            (429, "Content-Length: 100000"),
            (500, "Content-Length: 100000"),
            (401, "Transfer-Encoding: chunked"),
            (401, "Content-Length: 100"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let state = format!("gAAAAA{}", "x".repeat(286));
            let expected = state.clone();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut reader = BufReader::new(socket);
                let mut content_length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).await.unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        content_length = value.trim().parse::<usize>().unwrap();
                    }
                }
                reader
                    .read_exact(&mut vec![0; content_length])
                    .await
                    .unwrap();
                let mut socket = reader.into_inner();
                socket.write_all(format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: text/event-stream\r\n{framing}\r\nX-Codex-Turn-State: {state}\r\n\r\n"
                ).as_bytes()).await.unwrap();
                // Never send the body. Error inspection must also have a finite bound.
                let mut byte = [0];
                tokio::time::timeout(Duration::from_secs(3), socket.read(&mut byte))
                    .await
                    .expect("probe must release the connection")
            });
            let client = CodexBackendClient::new(
                reqwest::Client::new(),
                format!("http://{address}"),
                crate::OpenAiConfig::default().wire_profile_state(),
            );
            let request = CodexResponsesRequest::from_body(
                serde_json::from_value(
                    json!({"model":"model-a","input":"yes","stream":true,"store":false}),
                )
                .unwrap(),
            );
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                client.probe_turn_state_response(
                    &request,
                    CodexRequestContext::auxiliary("Bearer synthetic", None, "probe-test", None),
                ),
            )
            .await
            .expect("response headers must arrive before the body");
            if status == 200 {
                let mut response = result.unwrap();
                assert_eq!(response.turn_state, Some(expected));
                assert!(
                    tokio::time::timeout(Duration::from_millis(50), response.body.next())
                        .await
                        .is_err(),
                    "a header is not a completed probe"
                );
            } else {
                assert!(matches!(
                    result,
                    Err(crate::transport::CodexClientError::Upstream { .. })
                ));
            }
            drop(client);
            let closed = server.await.unwrap();
            assert!(matches!(closed, Ok(0) | Err(_)));
        }
    }

    #[tokio::test]
    async fn probe_errors_keep_bounded_authentication_details_and_discard_oversized_bodies() {
        use crate::transport::{CodexClientError, CodexRequestContext};
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        for body in [
            r#"{"error":{"code":"token_invalidated","message":"Token revoked"}}"#.to_owned(),
            "x".repeat(20 * 1024),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(401).set_body_string(body.clone()))
                .mount(&server)
                .await;
            let client = CodexBackendClient::new(
                reqwest::Client::new(),
                server.uri(),
                crate::OpenAiConfig::default().wire_profile_state(),
            );
            let request = encode_responses_body(
                serde_json::from_value(json!({"input":"yes","temperature":0.5,"store":true}))
                    .unwrap(),
                "model-a",
                None,
            );
            assert!(request.body().get("temperature").is_none());
            assert_eq!(request.body().get("store"), Some(&Value::Bool(false)));
            let result = client
                .probe_turn_state_response(
                    &request,
                    CodexRequestContext::auxiliary("Bearer synthetic", None, "probe-error", None),
                )
                .await;
            let Err(CodexClientError::Upstream {
                status,
                body: captured,
                ..
            }) = result
            else {
                panic!("expected an upstream status, never a generated success");
            };
            assert_eq!(status.as_u16(), 401);
            assert_eq!(captured, if body.len() > 16 * 1024 { "" } else { &body });
        }
    }

    #[tokio::test]
    async fn probe_and_business_sse_share_wire_identity_and_body_projection() {
        use crate::transport::CodexRequestContext;
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .insert_header("set-cookie", "__cf_bm=synthetic; Path=/")
                    .insert_header("x-codex-primary-used-percent", "10")
                    .set_body_string("data: [DONE]\n\n"),
            )
            .mount(&server)
            .await;
        let client = CodexBackendClient::new(
            reqwest::Client::new(),
            server.uri(),
            crate::OpenAiConfig::default().wire_profile_state(),
        );
        let mut request = encode_responses_body(
            serde_json::from_value(json!({
                "input": [{"role":"user", "content":[{"type":"input_text", "text":"yes"}]}],
                "temperature": 0.5,
                "store": true
            }))
            .unwrap(),
            "model-a",
            None,
        );
        request.identity_seed = Some(crate::transport::qx_application::identity_seed(
            &request,
            "synthetic-request",
        ));
        scope_request_to_account(&mut request, "synthetic-device", RequestAccountScope::Same);
        let mut context = CodexRequestContext::auxiliary(
            "Bearer synthetic",
            Some("synthetic-owner"),
            "synthetic-request",
            Some("synthetic-device"),
        );
        context.cookie_header = Some("__cf_bm=synthetic");
        let probe = client
            .probe_turn_state_response(&request, context)
            .await
            .unwrap();
        let business = client
            .create_response_stream_http_sse(&request, context)
            .await
            .unwrap();
        assert_eq!(probe.set_cookie_headers, business.set_cookie_headers);
        assert_eq!(probe.rate_limit_headers, business.rate_limit_headers);
        assert!(!probe.set_cookie_headers.is_empty());
        assert!(!probe.rate_limit_headers.is_empty());
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].headers, requests[1].headers);
        assert_eq!(requests[0].body, requests[1].body);
        for name in [
            "authorization",
            "chatgpt-account-id",
            "user-agent",
            "x-codex-installation-id",
            "cookie",
        ] {
            assert!(requests[0].headers.contains_key(name), "{name}");
        }
        assert!(!requests[0].headers.contains_key("x-codex-turn-state"));
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert!(body.get("temperature").is_none());
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
        let anomaly = pending.values().next().unwrap().back().unwrap();
        assert!(anomaly.suspect);
        assert_eq!(anomaly.expected_active_version, 3);
        assert_eq!(
            anomaly.expected_revision,
            account.turn_state_binding_revision()
        );
    }

    #[test]
    fn observations_retain_consecutive_outcomes_and_fence_old_versions() {
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
            let observations = pending.values().next().unwrap();
            assert_eq!(observations.len(), 2);
            assert!(observations[0].suspect);
            let anomaly = observations.back().unwrap();
            assert!(!anomaly.suspect);
            assert_eq!(anomaly.expected_active_version, 3);
        }
        manager.observe(&account, &model, Some(4), None, &normal, now);
        manager.observe(&account, &model, Some(3), None, &degraded, now);
        let pending = manager.pending.values.lock().unwrap();
        let observations = pending.values().next().unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].expected_active_version, 4);
        assert!(!observations[0].suspect);
        drop(pending);
        manager.observe(&account, &model, Some(4), None, &degraded, now);
        manager.observe(&account, &model, Some(4), None, &degraded, now);
        manager.observe(&account, &model, Some(4), None, &normal, now);
        let pending = manager.pending.values.lock().unwrap();
        assert!(
            pending
                .values()
                .next()
                .unwrap()
                .iter()
                .all(|item| item.suspect)
        );
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
                Some(1),
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
            Some(1),
            None,
            &updated,
            now,
        );
        let pending = manager.pending.values.lock().unwrap();
        assert_eq!(pending.len(), MAX_PENDING_OBSERVATIONS);
        let key = (ProviderAccountId::new("acct_account-0").unwrap(), model);
        let observations = pending.get(&key).unwrap();
        assert_eq!(observations.len(), 2);
        assert!(!observations.back().unwrap().suspect);
        assert_eq!(observations.back().unwrap().observed_at, now);
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

    #[test]
    fn observations_flag_expired_or_future_states_without_publishing_candidates() {
        let (manager, model) = manager();
        let account = managed_account("capture");
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_002);
        for timestamp in [1_600_000_000, 1_700_001_000] {
            manager.observe(&account, &model, Some(1), None, &token(timestamp, 160), now);
            assert!(
                manager
                    .pending
                    .values
                    .lock()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()
                    .back()
                    .unwrap()
                    .suspect
            );
        }
        manager.observe(
            &account,
            &model,
            Some(2),
            None,
            &token(1_700_000_000, 160),
            now,
        );
        let pending = manager.pending.values.lock().unwrap();
        let observation = pending.values().next().unwrap().back().unwrap();
        assert_eq!(observation.expected_active_version, 2);
        assert!(!observation.suspect);
        assert!(!observation.promote_standby);
    }

    #[tokio::test]
    async fn observer_drains_failed_writes_and_can_be_cancelled() {
        let (manager, model) = manager();
        manager.observe(
            &managed_account("account"),
            &model,
            Some(1),
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
    fn plan_changes_collect_active_and_healthy_states_only_refresh_near_expiry() {
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
        assert_eq!(refresh_slots(Some(&record), 332, now), (true, false));
        assert_eq!(refresh_slots(None, 292, now), (true, false));
        assert_eq!(
            refresh_slots(Some(&record), 292, now + Duration::from_secs(3000)),
            (false, true)
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
    fn fernet_shape_extracts_timestamp_and_rejects_prefix_only_values() {
        let value = token(1_700_000_000, 160);
        assert_eq!(value.len(), 292);
        let parsed = ParsedCodexTurnState::parse(&value).expect("valid Fernet shape");
        assert_eq!(parsed.cipher_blocks, 10);
        assert_eq!(
            ParsedCodexTurnState::parse(value.trim_end_matches('=')),
            Some(parsed)
        );
        let opaque = format!("gAAAAA{}", "!".repeat(286));
        assert!(ParsedCodexTurnState::parse(&opaque).is_none());
        assert_eq!(
            parsed.issued_at(),
            UNIX_EPOCH + Duration::from_secs(1_700_000_000)
        );
        assert_eq!(
            parsed
                .expires_at()
                .duration_since(parsed.issued_at())
                .unwrap(),
            TURN_STATE_TTL
        );
        assert!(ParsedCodexTurnState::parse(&format!("gAAAAA{}", "x".repeat(4096))).is_none());
        assert!(parsed.normal_for(INDIVIDUAL_NORMAL_LENGTH));
    }

    #[test]
    fn known_normal_and_degraded_shapes_are_plan_scoped() {
        assert_eq!(token(1_700_000_000, 176).len(), 312);
        assert_eq!(token(1_700_000_000, 192).len(), 332);
        assert_eq!(token(1_700_000_000, 208).len(), 356);
        assert!(
            !ParsedCodexTurnState::parse(&token(1_700_000_000, 176))
                .unwrap()
                .normal_for(292)
        );
        assert!(
            !ParsedCodexTurnState::parse(&token(1_700_000_000, 208))
                .unwrap()
                .normal_for(332)
        );
        assert!(
            !ParsedCodexTurnState::parse(&token(1_700_000_000, 160))
                .unwrap()
                .normal_for(332)
        );
        assert!(
            ParsedCodexTurnState::parse(&token(1_700_000_000, 192))
                .unwrap()
                .normal_for(332)
        );
        for plan in [
            "team",
            "business",
            " Business ",
            "self_serve_business_prolite",
            "self_serve_business_usage_based",
        ] {
            assert_eq!(expected_normal_length(Some(plan)), TEAM_NORMAL_LENGTH);
        }
        assert_eq!(
            expected_normal_length(Some("plus")),
            INDIVIDUAL_NORMAL_LENGTH
        );
    }

    #[test]
    fn missing_or_wrong_prefix_values_are_rejected() {
        assert!(ParsedCodexTurnState::parse("turn-state").is_none());
        let mut invalid = vec![0x81];
        invalid.extend_from_slice(&[0; 72]);
        assert!(ParsedCodexTurnState::parse(&URL_SAFE.encode(invalid)).is_none());
    }
}
