//! BPS session/quality semantics adapted from Sub2API bps*.go.
use super::state::digest;
use gateway_admin::model::mihomo::MihomoWarmStatus;
use gateway_core::{
    account::{OutboundProxy, ResponsesUpstream},
    provider_ports::session_proxy::{
        RequestProxySource, SessionProxyError, SessionProxyLease, SessionProxyOutcome,
        SessionProxyPool,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) const HEALTH_TTL: Duration = Duration::from_secs(120);
const DYNAMIC_WINDOW: Duration = Duration::from_secs(1200);
const SESSION_TTL: Duration = Duration::from_secs(1800);
const MAX_SESSIONS: usize = 4096;

#[derive(Clone, Default)]
struct Rate {
    successes: f64,
    samples: f64,
    updated: Option<Instant>,
}
impl Rate {
    fn decayed(&self, now: Instant) -> (f64, f64) {
        let factor = self.updated.map_or(1.0, |t| {
            (-now.saturating_duration_since(t).as_secs_f64() / 1800.0).exp2()
        });
        (self.successes * factor, self.samples * factor)
    }
    fn observe(&mut self, success: bool, now: Instant) {
        (self.successes, self.samples) = self.decayed(now);
        if self.samples >= 63.0 {
            self.successes *= 63.0 / self.samples;
            self.samples = 63.0;
        }
        self.samples += 1.0;
        if success {
            self.successes += 1.0;
        }
        self.updated = Some(now);
    }
    fn value(&self, now: Instant) -> f64 {
        let (s, n) = self.decayed(now);
        (s + 3.0) / (n + 4.0)
    }
}

pub(super) struct Exit {
    pub proxy: OutboundProxy,
    pub dynamic: bool,
    pub subscription: bool,
    pub generation: u64,
    pub revision: u64,
    pub verified_until: Option<Instant>,
    pub retry_after: Option<Instant>,
    pub probing: bool,
    pub probe_owner: Option<(u64, u64)>,
    pub reason: Option<&'static str>,
    pub last_probe: Option<Instant>,
    pub failures: u32,
    window: Instant,
    model: Rate,
    connect: Rate,
    stream_failures: u32,
    last_stream_failure: Option<Instant>,
}
impl Exit {
    pub fn new(proxy: OutboundProxy, dynamic: bool, subscription: bool, now: Instant) -> Self {
        Self {
            proxy,
            dynamic,
            subscription,
            generation: 1,
            revision: 0,
            verified_until: None,
            retry_after: None,
            probing: false,
            probe_owner: None,
            reason: None,
            last_probe: None,
            failures: 0,
            window: now,
            model: Rate::default(),
            connect: Rate::default(),
            stream_failures: 0,
            last_stream_failure: None,
        }
    }
    fn advance(&mut self, now: Instant) {
        if self.dynamic && now.saturating_duration_since(self.window) >= DYNAMIC_WINDOW {
            self.window = now;
            self.generation += 1;
            self.revision += 1;
            self.model = Rate::default();
            self.connect = Rate::default();
            self.verified_until = None;
            self.retry_after = None;
            self.failures = 0;
            self.stream_failures = 0;
            self.last_stream_failure = None;
            self.reason = None;
        }
    }
    pub fn cooling(&self, now: Instant) -> bool {
        self.retry_after.is_some_and(|t| t > now)
    }
    pub fn ready(&self, now: Instant) -> bool {
        !self.cooling(now) && self.verified_until.is_some_and(|t| t > now)
    }
    fn score(&self, active: usize, bound: usize, now: Instant) -> f64 {
        (0.7 * self.model.value(now) + 0.3 * self.connect.value(now))
            / (1.0 + 0.15 * active as f64 + 0.02 * bound as f64)
    }
    fn cooldown(&mut self, now: Instant, duration: Duration) {
        self.retry_after = Some(
            self.retry_after
                .map_or(now + duration, |t| t.max(now + duration)),
        );
    }
}

struct Session {
    node: String,
    generation: u64,
    active: usize,
    failed: bool,
    used: Instant,
    transient: bool,
}

#[derive(Default)]
pub(super) struct PoolState {
    pub available: bool,
    pub target: usize,
    pub exits: BTreeMap<String, Exit>,
    sessions: BTreeMap<String, Session>,
    serial: u64,
}

impl PoolState {
    fn sweep(&mut self, now: Instant) {
        self.sessions.retain(|_, s| {
            s.active > 0 || (!s.transient && now.saturating_duration_since(s.used) < SESSION_TTL)
        });
        for exit in self.exits.values_mut() {
            exit.advance(now);
        }
    }
    pub fn loads(&self) -> BTreeMap<String, (usize, usize)> {
        let mut loads = BTreeMap::new();
        for s in self.sessions.values() {
            let item = loads.entry(s.node.clone()).or_insert((0, 0));
            item.0 += s.active;
            item.1 += 1;
        }
        loads
    }
    pub fn replace(
        &mut self,
        exits: Vec<(String, OutboundProxy, bool, bool)>,
        available: bool,
        now: Instant,
    ) {
        self.serial = self
            .serial
            .max(self.exits.values().map(|h| h.generation).max().unwrap_or(0));
        let keep: BTreeSet<_> = exits.iter().map(|(id, _, _, _)| id.clone()).collect();
        self.exits.retain(|id, _| keep.contains(id));
        for (id, proxy, dynamic, subscription) in exits {
            if let Some(exit) = self.exits.get_mut(&id) {
                exit.dynamic = dynamic;
                exit.subscription = subscription;
            }
            if let std::collections::btree_map::Entry::Vacant(entry) = self.exits.entry(id) {
                self.serial += 1;
                let mut exit = Exit::new(proxy, dynamic, subscription, now);
                exit.generation = self.serial;
                entry.insert(exit);
            }
        }
        self.available = available;
        self.sweep(now);
    }
    fn fail(&mut self, node: &str, now: Instant) {
        if let Some(h) = self.exits.get_mut(node) {
            h.failures = h.failures.saturating_add(1);
            h.revision += 1;
            h.verified_until = None;
            if h.failures >= 2 {
                h.cooldown(now, Duration::from_secs(60));
            }
        }
        for s in self.sessions.values_mut().filter(|s| s.node == node) {
            s.failed = true;
        }
    }
    pub fn qualify(
        &mut self,
        id: &str,
        generation: u64,
        revision: u64,
        result: Result<(), &'static str>,
        observations: &[bool],
        now: Instant,
    ) {
        let Some(h) = self.exits.get_mut(id) else {
            return;
        };
        if h.probe_owner == Some((generation, revision)) {
            h.probing = false;
            h.probe_owner = None;
        }
        h.advance(now);
        if h.generation != generation || h.revision != revision {
            return;
        }
        h.last_probe = Some(now);
        for success in observations {
            h.connect.observe(*success, now);
        }
        match result {
            Ok(()) => {
                h.verified_until = Some(now + HEALTH_TTL);
                h.failures = 0;
                h.reason = None;
            }
            Err(reason) => {
                h.reason = Some(reason);
                if matches!(reason, "access_denied" | "proxy_auth_required") {
                    h.cooldown(now, Duration::from_secs(300));
                }
                h.cooldown(now, Duration::from_secs(15));
                for _ in 0..observations
                    .iter()
                    .filter(|success| !**success)
                    .count()
                    .max(1)
                {
                    self.fail(id, now);
                }
            }
        }
    }
    pub fn status(&mut self, now: Instant) -> MihomoWarmStatus {
        self.sweep(now);
        let mut status = MihomoWarmStatus {
            target: self.target,
            ..Default::default()
        };
        if !self.available {
            return status;
        }
        for exit in self.exits.values() {
            if exit.probing {
                status.checking += 1;
            }
            if exit.cooling(now) {
                status.cooling += 1;
                if let Some(reason) = exit.reason {
                    *status.failure_reasons.entry(reason.into()).or_default() += 1;
                }
            } else {
                status.eligible += 1;
            }
            if exit.ready(now) {
                status.ready += 1;
                if exit.subscription {
                    status.ready_subscription += 1;
                } else if exit.dynamic {
                    status.ready_dynamic += 1;
                }
            }
        }
        status
    }
    pub fn rank(&self, now: Instant) -> Vec<String> {
        let loads = self.loads();
        let mut ids: Vec<_> = self.exits.keys().cloned().collect();
        ids.sort_by(|a, b| {
            let x = &self.exits[a];
            let y = &self.exits[b];
            let (xa, xb) = loads.get(a).copied().unwrap_or_default();
            let (ya, yb) = loads.get(b).copied().unwrap_or_default();
            y.subscription
                .cmp(&x.subscription)
                .then_with(|| y.score(ya, yb, now).total_cmp(&x.score(xa, xb, now)))
                .then_with(|| x.last_probe.cmp(&y.last_probe))
                .then_with(|| a.cmp(b))
        });
        ids
    }
}

pub(super) fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Default)]
pub struct ExitPools {
    pub(super) managed: Arc<Mutex<PoolState>>,
    pub(super) regular: Arc<Mutex<PoolState>>,
    pub(super) codex_managed: Arc<Mutex<PoolState>>,
    pub(super) codex_regular: Arc<Mutex<PoolState>>,
}

impl ExitPools {
    pub(super) fn route_pools(&self, route: ResponsesUpstream) -> [&Arc<Mutex<PoolState>>; 2] {
        match route {
            ResponsesUpstream::Excel => [&self.managed, &self.regular],
            ResponsesUpstream::Codex => [&self.codex_managed, &self.codex_regular],
        }
    }
}

struct Lease {
    pool: Arc<Mutex<PoolState>>,
    key: String,
    node: String,
    generation: u64,
    proxy: OutboundProxy,
    reported: AtomicBool,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = lock(&self.pool);
        if let Some(s) = state.sessions.get_mut(&self.key) {
            if s.node != self.node || s.generation != self.generation {
                return;
            }
            s.active = s.active.saturating_sub(1);
            s.used = Instant::now();
            if s.active == 0 && s.transient {
                state.sessions.remove(&self.key);
            }
        }
    }
}
impl SessionProxyLease for Lease {
    fn retry_unsent(&self) -> Option<Arc<dyn SessionProxyLease>> {
        let mut state = lock(&self.pool);
        let now = Instant::now();
        state.sweep(now);
        if !state.available {
            return None;
        }
        let session = state.sessions.get(&self.key)?;
        if session.node != self.node
            || session.generation != self.generation
            || session.active != 1
            || !session.failed
        {
            return None;
        }
        let transient = session.transient;
        let node = state
            .rank(now)
            .into_iter()
            .find(|id| *id != self.node && state.exits[id].ready(now))?;
        let generation = state.exits[&node].generation;
        let proxy = state.exits[&node].proxy.clone();
        state.sessions.insert(
            self.key.clone(),
            Session {
                node: node.clone(),
                generation,
                active: 1,
                failed: false,
                used: now,
                transient,
            },
        );
        Some(Arc::new(Lease {
            pool: self.pool.clone(),
            key: self.key.clone(),
            node,
            generation,
            proxy,
            reported: AtomicBool::new(false),
        }))
    }

    fn proxy(&self) -> &OutboundProxy {
        &self.proxy
    }
    fn node_id(&self) -> &str {
        &self.node
    }
    fn report(&self, outcome: SessionProxyOutcome) {
        if self.reported.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut state = lock(&self.pool);
        let now = Instant::now();
        let Some(h) = state.exits.get_mut(&self.node) else {
            return;
        };
        h.advance(now);
        if h.generation != self.generation {
            return;
        }
        h.model
            .observe(outcome == SessionProxyOutcome::Completed, now);
        match outcome {
            SessionProxyOutcome::Completed | SessionProxyOutcome::UpstreamFailure => {}
            SessionProxyOutcome::NetworkFailure => {
                h.connect.observe(false, now);
                h.reason = Some("network_failure");
                h.cooldown(now, Duration::from_secs(15));
                state.fail(&self.node, now);
            }
            SessionProxyOutcome::StreamFailure => {
                let window = if h.dynamic {
                    DYNAMIC_WINDOW
                } else {
                    Duration::from_secs(1800)
                };
                if h.last_stream_failure
                    .is_none_or(|t| now.saturating_duration_since(t) >= window)
                {
                    h.stream_failures = 0;
                }
                h.stream_failures = (h.stream_failures + 1).min(4);
                h.last_stream_failure = Some(now);
                let (base, max) = if h.dynamic { (30, 120) } else { (300, 1800) };
                h.cooldown(
                    now,
                    Duration::from_secs((base << (h.stream_failures - 1)).min(max)),
                );
                h.reason = Some("stream_failure");
                state.fail(&self.node, now);
            }
        }
    }
}

impl SessionProxyPool for ExitPools {
    fn acquire(
        &self,
        source: RequestProxySource,
        route: ResponsesUpstream,
        scope: &str,
        transient: bool,
    ) -> Result<Arc<dyn SessionProxyLease>, SessionProxyError> {
        if scope.is_empty() {
            return Err(SessionProxyError::Identity);
        }
        let pools = self.route_pools(route);
        let pool = match source {
            RequestProxySource::Mihomo => pools[0],
            RequestProxySource::ProxyPool => pools[1],
            RequestProxySource::Account => return Err(SessionProxyError::Unavailable),
        };
        let mut state = lock(pool);
        let now = Instant::now();
        state.sweep(now);
        if !state.available {
            return Err(SessionProxyError::Unavailable);
        }
        let key = digest(scope.as_bytes());
        if let Some(binding) = state.sessions.get(&key) {
            let valid = !binding.failed
                && state
                    .exits
                    .get(&binding.node)
                    .is_some_and(|e| e.ready(now) && e.generation == binding.generation);
            if !valid {
                if binding.active > 0 {
                    return Err(SessionProxyError::Draining);
                }
                state.sessions.remove(&key);
            }
        }
        if !state.sessions.contains_key(&key) {
            if state.sessions.len() >= MAX_SESSIONS {
                return Err(SessionProxyError::Capacity);
            }
            let node = state
                .rank(now)
                .into_iter()
                .find(|id| state.exits[id].ready(now))
                .ok_or(SessionProxyError::Warming)?;
            let generation = state.exits[&node].generation;
            state.sessions.insert(
                key.clone(),
                Session {
                    node,
                    generation,
                    active: 0,
                    failed: false,
                    used: now,
                    transient,
                },
            );
        }
        let binding = state.sessions.get_mut(&key).expect("session inserted");
        binding.active += 1;
        binding.used = now;
        let node = binding.node.clone();
        let generation = binding.generation;
        let proxy = state.exits[&node].proxy.clone();
        Ok(Arc::new(Lease {
            pool: Arc::clone(pool),
            key,
            node,
            generation,
            proxy,
            reported: AtomicBool::new(false),
        }))
    }
}
