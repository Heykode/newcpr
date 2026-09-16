//! Codex WebSocket 连接池。

mod capacity;
mod lease;
#[cfg(test)]
mod lifecycle_tests;
mod state;
mod supervisor;
#[cfg(test)]
mod tests;

use std::{
    future::Future,
    sync::{Arc, Mutex, MutexGuard, atomic::AtomicBool},
    time::Duration,
};

use tokio::sync::watch;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

use self::state::{
    WebSocketPoolConnecting, WebSocketPoolSlot, WebSocketPoolState, close_pooled_connection,
};
use super::pump::PumpKeepalive;
use super::pump::WebSocketConnectionObservation;
use gateway_core::runtime::{AccountConcurrencyHandle, RequestTuningHandle};

pub use self::state::CodexWebSocketPoolKey;
pub(crate) use self::state::CodexWebSocketRouting;
pub(crate) use self::{
    lease::{
        WebSocketPoolAcquire, WebSocketPoolConnectLease, WebSocketPoolConnectOutcome,
        WebSocketPoolConnectWaiter, WebSocketPoolLease,
    },
    state::{
        CodexWebSocketConnectionMetadata, PooledWebSocketConnection, WebSocketContinuationState,
    },
};

const DEFAULT_MAX_AGE: Duration = Duration::from_mins(55);
const DEFAULT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(25);
const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(25);
// 心跳也覆盖正在生成的连接，给短时链路停顿留出恢复余量。
const DEFAULT_PING_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// WebSocket 连接池。
#[derive(Clone)]
pub struct CodexWebSocketPool {
    inner: Arc<Mutex<WebSocketPoolState>>,
    config: CodexWebSocketPoolConfig,
    tasks: TaskTracker,
    shutdown: CancellationToken,
    capacity_changed: watch::Sender<()>,
    account_concurrency: Option<AccountConcurrencyHandle>,
    maintenance_started: Arc<AtomicBool>,
    request_tuning: Option<RequestTuningHandle>,
}

impl Default for CodexWebSocketPool {
    fn default() -> Self {
        Self::with_config(CodexWebSocketPoolConfig::default())
    }
}

/// WebSocket 连接池配置。
#[derive(Debug, Clone, Copy)]
pub struct CodexWebSocketPoolConfig {
    /// 是否启用连接池。
    pub enabled: bool,
    /// 单个 socket 的最大生命周期。
    pub max_age: Duration,
    /// 后台维护间隔；`None` 表示不启动后台任务。
    pub maintenance_interval: Option<Duration>,
    /// 池化连接的探活 ping 间隔（包括正在生成的连接）；`None` 表示不主动 ping。
    pub ping_interval: Option<Duration>,
    /// 发送 ping 后等待任意入站帧的超时时间；零值表示不校验 Pong deadline。
    pub ping_timeout: Duration,
    /// idle socket 无活动多久后视为失活。
    pub liveness_timeout: Option<Duration>,
    /// 等待下一条上游消息的空闲超时；`None` 或零值使用默认 300 秒。
    pub stream_idle_timeout: Option<Duration>,
}

impl Default for CodexWebSocketPoolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_age: DEFAULT_MAX_AGE,
            maintenance_interval: Some(DEFAULT_MAINTENANCE_INTERVAL),
            ping_interval: Some(DEFAULT_PING_INTERVAL),
            ping_timeout: DEFAULT_PING_TIMEOUT,
            // idle 连接不设失活截断：靠 ping/pong 保活，只在 max_age（55 分钟）
            // 或 ping 失败时关闭，维持跨轮可复用连接。
            liveness_timeout: None,
            stream_idle_timeout: Some(DEFAULT_STREAM_IDLE_TIMEOUT),
        }
    }
}

impl CodexWebSocketPoolConfig {
    /// pump 后台任务的保活策略：从连接池配置派生出 ping/pong 与 liveness 策略。
    pub(crate) fn keepalive(&self) -> PumpKeepalive {
        PumpKeepalive {
            ping_interval: self.ping_interval,
            ping_timeout: (!self.ping_timeout.is_zero()).then_some(self.ping_timeout),
            liveness_timeout: self.liveness_timeout,
        }
    }
}

impl CodexWebSocketPool {
    /// Resolve the current physical owner before selecting a random/fresh route.
    pub(crate) fn routing_owner(
        &self,
        key: &CodexWebSocketPoolKey,
        response_id: Option<&str>,
    ) -> Option<CodexWebSocketPoolKey> {
        let response_id = response_id?;
        let state = self.lock_state();
        if state.shutting_down || !self.config.enabled {
            return None;
        }
        state.slots.iter().find_map(|(candidate, slot)| {
            if !candidate.matches_routing_owner(key) {
                return None;
            }
            let matches = slot.latest_response_id() == Some(response_id);
            matches.then(|| candidate.clone())
        })
    }

    /// 构造连接池策略和状态；生产容量由账号运行快照提供。
    pub fn new(max_age: Duration) -> Self {
        Self::with_config(CodexWebSocketPoolConfig {
            max_age,
            maintenance_interval: None,
            ping_interval: None,
            liveness_timeout: None,
            ..CodexWebSocketPoolConfig::default()
        })
    }

    /// 使用完整配置构造连接池。
    pub fn with_config(config: CodexWebSocketPoolConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(WebSocketPoolState::default())),
            config,
            tasks: TaskTracker::new(),
            shutdown: CancellationToken::new(),
            capacity_changed: watch::channel(()).0,
            account_concurrency: None,
            maintenance_started: Arc::new(AtomicBool::new(false)),
            request_tuning: None,
        }
    }

    #[must_use]
    pub fn with_request_tuning(mut self, request_tuning: RequestTuningHandle) -> Self {
        self.account_concurrency = Some(request_tuning.account_concurrency());
        self.request_tuning = Some(request_tuning);
        // Attach the live handle before spawning maintenance so the supervisor
        // observes the same settings as foreground pool operations.
        self.spawn_maintenance_task();
        self
    }

    /// Keep the public legacy provider initializer's standalone pool behavior.
    pub(crate) fn with_request_tuning_without_account_concurrency(
        mut self,
        request_tuning: RequestTuningHandle,
    ) -> Self {
        self.request_tuning = Some(request_tuning);
        self.spawn_maintenance_task();
        self
    }

    fn max_age(&self) -> Duration {
        self.request_tuning
            .as_ref()
            .map(|handle| Duration::from_millis(handle.load().websocket_max_age_ms))
            .unwrap_or(self.config.max_age)
    }

    /// Attach authoritative account limits when embedding the standalone transport.
    #[must_use]
    pub fn with_account_concurrency(mut self, limits: AccountConcurrencyHandle) -> Self {
        self.account_concurrency = Some(limits);
        self
    }

    /// pump 后台任务的保活策略（供建连时传入）。
    pub(crate) fn keepalive(&self) -> PumpKeepalive {
        self.config.keepalive()
    }

    /// 等待下一条上游消息的空闲超时；`None` 或零值使用默认 300 秒。
    pub(crate) fn stream_idle_timeout(&self) -> Option<Duration> {
        self.request_tuning
            .as_ref()
            .map(|handle| {
                let timeout = handle.load().websocket_stream_idle_timeout_ms;
                (timeout != 0).then_some(Duration::from_millis(timeout))
            })
            .unwrap_or(self.config.stream_idle_timeout)
    }

    /// 注册由连接池生命周期托管的 opening 任务。
    pub(crate) fn spawn_connect_task(&self, future: impl Future<Output = ()> + Send + 'static) {
        drop(self.tasks.spawn(future));
    }

    pub(crate) async fn acquire(
        &self,
        key: &CodexWebSocketPoolKey,
        required_response_id: Option<&str>,
    ) -> WebSocketPoolAcquire {
        self.spawn_maintenance_task();
        let mut changed = self.capacity_changed.subscribe();
        loop {
            changed.borrow_and_update();
            let (acquire, close) = self.acquire_once(key, required_response_id);
            drop(self.start_closing(close));
            if let Some(acquire) = acquire {
                return acquire;
            }
            // Core snapshots have no Tokio dependency. The bounded recheck notices
            // live limit increases even when this account has no returning socket.
            tokio::select! {
                _ = self.shutdown.cancelled() => {
                    return WebSocketPoolAcquire::Bypass(WebSocketPoolBypassReason::Disabled);
                }
                _ = changed.changed() => {}
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
    }

    fn acquire_once(
        &self,
        key: &CodexWebSocketPoolKey,
        required_response_id: Option<&str>,
    ) -> (
        Option<WebSocketPoolAcquire>,
        Vec<capacity::RetiredConnection>,
    ) {
        let mut close = Vec::new();
        let acquire = (|| {
            let mut state = self.lock_state();
            if !self.config.enabled || state.shutting_down {
                return Some(WebSocketPoolAcquire::Bypass(
                    WebSocketPoolBypassReason::Disabled,
                ));
            }
            if let Some(error) = state
                .egress_config
                .as_ref()
                .and_then(|config| key.unavailable_egress(config))
            {
                return Some(WebSocketPoolAcquire::LocalEgress(error));
            }
            let Some(limit) = self.account_limit(key.account_id()) else {
                return Some(WebSocketPoolAcquire::LocalEgress(
                    crate::transport::egress::CodexEgressError::ConfigurationChanged,
                ));
            };
            let key = if let Some(response_id) = required_response_id {
                let Some(key) = state.slots.iter().find_map(|(candidate, slot)| {
                    (candidate.same_logical_connection(key)
                        && slot.latest_response_id() == Some(response_id))
                    .then(|| candidate.clone())
                }) else {
                    return Some(
                        state
                            .continuation_loss(key, response_id, tokio::time::Instant::now())
                            .map_or(
                                WebSocketPoolAcquire::Bypass(
                                    WebSocketPoolBypassReason::ContinuationNotFound,
                                ),
                                WebSocketPoolAcquire::ContinuationLost,
                            ),
                    );
                };
                key
            } else {
                key.clone()
            };
            close.extend(self.trim_account_idle(&mut state, key.account_id(), Some(&key)));
            match state.slots.get(&key) {
                Some(WebSocketPoolSlot::Busy(_)) => {
                    return Some(WebSocketPoolAcquire::Bypass(
                        WebSocketPoolBypassReason::Busy,
                    ));
                }
                Some(WebSocketPoolSlot::Connecting(connecting)) => {
                    if connecting.cancellation.is_cancelled() {
                        return None;
                    }
                    return Some(WebSocketPoolAcquire::Wait(WebSocketPoolConnectWaiter {
                        receiver: connecting.outcome.subscribe(),
                        started_at: connecting.started_at,
                    }));
                }
                Some(WebSocketPoolSlot::Closing(_)) => return None,
                Some(WebSocketPoolSlot::Idle { .. }) => {
                    let Some(WebSocketPoolSlot::Idle { connection, .. }) = state.slots.remove(&key)
                    else {
                        return Some(WebSocketPoolAcquire::Bypass(
                            WebSocketPoolBypassReason::Busy,
                        ));
                    };
                    // 零成本探活：后台 pump 已实时感知连接死亡（RST/Close/EOF/失活），
                    // 复用前只需读取 is_closed 标志，避免复用到静默死连接后卡到超时。
                    let expired = connection.created_at.elapsed() >= self.max_age();
                    let closed = connection.websocket.is_closed();
                    let over_capacity =
                        Self::account_retained_slots(&state, key.account_id()) >= limit;
                    if !expired && !closed && !over_capacity {
                        let lease = WebSocketPoolLease::reserve(
                            self.clone(),
                            key.clone(),
                            connection.continuation.latest_response_id(),
                        )
                        .with_websocket(&connection.websocket);
                        state.slots.insert(
                            key.clone(),
                            WebSocketPoolSlot::Busy(lease.reservation.clone()),
                        );
                        return Some(WebSocketPoolAcquire::Reused { connection, lease });
                    }
                    let observation = if over_capacity {
                        connection
                            .websocket
                            .observation()
                            .with_exit_reason("account_capacity_reclaimed")
                    } else if expired && !closed {
                        connection
                            .websocket
                            .observation()
                            .with_exit_reason("max_age_expired")
                    } else {
                        connection.websocket.observation()
                    };
                    close.push(Self::retire_connection(
                        &mut state,
                        key,
                        *connection,
                        if over_capacity {
                            Some("account_capacity_reclaimed")
                        } else {
                            (expired && !closed).then_some("max_age_expired")
                        },
                    ));
                    if required_response_id.is_some() {
                        return Some(WebSocketPoolAcquire::ContinuationLost(observation));
                    }
                    return None;
                }
                None => {}
            }

            if required_response_id.is_some() {
                return Some(WebSocketPoolAcquire::Bypass(
                    WebSocketPoolBypassReason::ContinuationNotFound,
                ));
            }
            if Self::account_slots(&state, key.account_id()) >= limit {
                // Already-retiring owners will free capacity without sacrificing another chain.
                if close.is_empty()
                    && Self::account_retained_slots(&state, key.account_id()) >= limit
                    && let Some(retired) =
                        Self::retire_oldest_idle(&mut state, key.account_id(), None)
                {
                    close.push(retired);
                }
                return None;
            }
            let lease = WebSocketPoolConnectLease::reserve(self.clone(), key.clone());
            state.slots.insert(
                key,
                WebSocketPoolSlot::Connecting(WebSocketPoolConnecting {
                    id: lease.id,
                    started_at: lease.started_at,
                    outcome: lease.outcome.clone(),
                    cancellation: lease.cancellation.clone(),
                }),
            );
            Some(WebSocketPoolAcquire::Connect(lease))
        })();
        (acquire, close)
    }

    async fn put_reserved(
        &self,
        key: &CodexWebSocketPoolKey,
        reservation_id: Uuid,
        connection: PooledWebSocketConnection,
    ) {
        let mut connection = Some(connection);
        let mut close = Vec::new();
        {
            let mut state = self.lock_state();
            let expired = connection
                .as_ref()
                .is_some_and(|connection| connection.created_at.elapsed() >= self.max_age());
            let owns_reservation = matches!(
                state.slots.get(key),
                Some(WebSocketPoolSlot::Busy(reservation))
                    if reservation.id == reservation_id
            );
            let retire_on_return = matches!(
                state.slots.get(key),
                Some(WebSocketPoolSlot::Busy(reservation))
                    if reservation.id == reservation_id && reservation.retire_on_return
            );
            let over_capacity = Self::account_retained_slots(&state, key.account_id())
                > self.account_limit(key.account_id()).unwrap_or(0);
            if owns_reservation
                && (expired
                    || retire_on_return
                    || over_capacity
                    || state.shutting_down
                    || !self.config.enabled)
            {
                if let Some(connection) = connection.take() {
                    close.push(Self::retire_connection(
                        &mut state,
                        key.clone(),
                        connection,
                        Some(if expired {
                            "max_age_expired"
                        } else {
                            "account_capacity_reclaimed"
                        }),
                    ));
                }
            } else if owns_reservation && let Some(connection) = connection.take() {
                state.slots.insert(
                    key.clone(),
                    WebSocketPoolSlot::Idle {
                        connection: Box::new(connection),
                    },
                );
            }
        }
        self.capacity_changed.send_replace(());
        self.close_retired_connections(close).await;
        if let Some(connection) = connection {
            let _ = self.tasks.spawn(close_pooled_connection(connection)).await;
        }
    }

    fn discard_reserved_now(&self, key: &CodexWebSocketPoolKey, reservation_id: Uuid) {
        let mut state = self.lock_state();
        if matches!(
            state.slots.get(key),
            Some(WebSocketPoolSlot::Busy(reservation)) if reservation.id == reservation_id
        ) {
            state.slots.remove(key);
            self.capacity_changed.send_replace(());
        }
    }

    fn discard_reserved_with_observation(
        &self,
        key: &CodexWebSocketPoolKey,
        reservation_id: Uuid,
        websocket: super::pump::PumpedWebSocket,
        observation: WebSocketConnectionObservation,
    ) -> tokio::task::JoinHandle<()> {
        let mut state = self.lock_state();
        let latest_response_id = match state.slots.get(key) {
            Some(WebSocketPoolSlot::Busy(reservation)) if reservation.id == reservation_id => {
                reservation.latest_response_id.clone()
            }
            _ => {
                drop(state);
                return self.start_closing_socket(websocket, None);
            }
        };
        let retired = Self::retire_websocket(
            &mut state,
            key.clone(),
            websocket,
            latest_response_id.as_deref(),
            observation,
        );
        drop(state);
        self.start_closing_one(retired)
    }

    fn discard_reserved_after_termination(
        &self,
        key: &CodexWebSocketPoolKey,
        reservation_id: Uuid,
        termination: super::pump::PumpTermination,
    ) {
        let mut state = self.lock_state();
        let reservation = match state.slots.get(key) {
            Some(WebSocketPoolSlot::Busy(reservation)) if reservation.id == reservation_id => {
                let latest_response_id = reservation.latest_response_id.clone();
                state.remember_continuation_loss(
                    key,
                    latest_response_id.as_deref(),
                    termination.observation().with_exit_reason("lease_dropped"),
                    tokio::time::Instant::now(),
                );
                state
                    .slots
                    .insert(key.clone(), WebSocketPoolSlot::Closing(reservation_id));
                Some((key.clone(), reservation_id))
            }
            _ => None,
        };
        drop(state);
        self.start_aborting_socket(termination, reservation);
    }

    async fn finish_connect_reserved(
        &self,
        key: &CodexWebSocketPoolKey,
        connect_id: Uuid,
        connection: PooledWebSocketConnection,
    ) -> Result<(Box<PooledWebSocketConnection>, WebSocketPoolLease), tokio::task::JoinHandle<()>>
    {
        let mut state = self.lock_state();
        let owns_connect = matches!(
            state.slots.get(key),
            Some(WebSocketPoolSlot::Connecting(connecting)) if connecting.id == connect_id
        );
        let cancelled = matches!(state.slots.get(key),
            Some(WebSocketPoolSlot::Connecting(connecting)) if connecting.cancellation.is_cancelled());
        if owns_connect && !cancelled && !state.shutting_down && self.config.enabled {
            let lease = WebSocketPoolLease::reserve(self.clone(), key.clone(), None)
                .with_websocket(&connection.websocket);
            state.slots.insert(
                key.clone(),
                WebSocketPoolSlot::Busy(lease.reservation.clone()),
            );
            Ok((Box::new(connection), lease))
        } else {
            let closing = if owns_connect {
                let retired = Self::retire_connection(
                    &mut state,
                    key.clone(),
                    connection,
                    Some("opening_cancelled"),
                );
                drop(state);
                self.start_closing_one(retired)
            } else {
                drop(state);
                self.start_closing_socket(connection.websocket, None)
            };
            Err(closing)
        }
    }

    async fn fail_connect(&self, key: &CodexWebSocketPoolKey, connect_id: Uuid) {
        self.fail_connect_now(key, connect_id);
    }

    fn fail_connect_now(&self, key: &CodexWebSocketPoolKey, connect_id: Uuid) {
        let mut state = self.lock_state();
        if matches!(
            state.slots.get(key),
            Some(WebSocketPoolSlot::Connecting(connecting)) if connecting.id == connect_id
        ) {
            state.slots.remove(key);
            self.capacity_changed.send_replace(());
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, WebSocketPoolState> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketPoolBypassReason {
    Disabled,
    Busy,
    Cap,
    ContinuationNotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebSocketPoolDecision {
    kind: WebSocketPoolDecisionKind,
}

impl WebSocketPoolDecision {
    pub fn new() -> Self {
        Self {
            kind: WebSocketPoolDecisionKind::New,
        }
    }

    pub fn reuse() -> Self {
        Self {
            kind: WebSocketPoolDecisionKind::Reuse,
        }
    }

    pub fn kind(self) -> &'static str {
        self.kind.as_str()
    }

    pub const fn is_reuse(self) -> bool {
        matches!(self.kind, WebSocketPoolDecisionKind::Reuse)
    }
}

impl Default for WebSocketPoolDecision {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebSocketPoolDecisionKind {
    New,
    Reuse,
}

impl WebSocketPoolDecisionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Reuse => "reuse",
        }
    }
}
