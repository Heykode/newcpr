//! WebSocket 连接池维护、驱逐与关闭监督。

use std::sync::{
    Arc,
    atomic::{Ordering, Ordering::AcqRel},
};

use tokio::time::Instant;

use crate::transport::egress::{CodexEgressError, CodexEgressRuntime};

use super::CodexWebSocketPool;
use super::capacity::RetiredConnection;
use super::state::{
    CodexWebSocketPoolKey, WebSocketPoolSlot, WebSocketPoolState, close_pooled_connections,
    should_close_idle_connection,
};

impl CodexWebSocketPool {
    /// 驱逐指定账号的全部 slot，取消 opening，并阻止 busy 连接回收到池中。
    pub async fn evict_account(&self, account_id: &str) {
        let idle_connections = {
            let mut state = self.lock_state();
            let keys = state
                .slots
                .keys()
                .filter(|key| key.account_id() == account_id)
                .map(|key| (key.clone(), None))
                .collect::<Vec<_>>();
            take_retired_slots(&mut state, keys)
        };
        self.close_retired_connections(idle_connections).await;
    }

    /// Retire invalid owners after publication, without interrupting sent responses.
    pub async fn retire_unavailable_egress(
        &self,
        runtime: &CodexEgressRuntime,
    ) -> Result<(), CodexEgressError> {
        let config = runtime.snapshot()?;
        let idle_connections = {
            let mut state = self.lock_state();
            if state
                .egress_config
                .as_ref()
                .is_some_and(|current| current.revision > config.revision)
            {
                return Ok(());
            }
            let keys = state
                .slots
                .keys()
                .filter_map(|key| {
                    key.unavailable_egress(&config)
                        .map(|error| (key.clone(), Some(error)))
                })
                .collect::<Vec<_>>();
            // Fence acquire as well, so an older prepared key cannot recreate a slot.
            state.egress_config = Some(config);
            take_retired_slots(&mut state, keys)
        };
        self.close_retired_connections(idle_connections).await;
        Ok(())
    }

    /// 关闭连接池，取消受管任务、关闭 idle 连接，并让后续 acquire 直接绕过池。
    pub async fn shutdown(&self) {
        self.tasks.close();
        self.shutdown.cancel();
        let idle_connections = {
            let mut state = self.lock_state();
            state.shutting_down = true;
            state
                .slots
                .drain()
                .filter_map(|(_, slot)| match slot {
                    WebSocketPoolSlot::Idle { connection, .. } => Some(*connection),
                    WebSocketPoolSlot::Connecting(connecting) => {
                        connecting.cancel(None);
                        None
                    }
                    WebSocketPoolSlot::Busy(_) | WebSocketPoolSlot::Closing(_) => None,
                })
                .collect::<Vec<_>>()
        };
        self.capacity_changed.send_replace(());
        close_pooled_connections(idle_connections).await;
        self.tasks.wait().await;
    }

    /// 维护池 slot：清扫已死亡或超龄的 idle 连接，以及异常残留的 Busy reservation。
    ///
    /// 保活（ping/pong）与失活检测已下沉到每条连接的 pump 任务内部，此处不再做
    /// 同步 ping 探活，只负责把「后台已标记 closed」或「超过 max_age」的 idle
    /// 连接从池中摘除并关闭，避免它们占用 slot。
    pub async fn maintain_idle_connections(&self) {
        let close = self.take_expired_slots().await;
        self.close_retired_connections(close).await;
    }

    pub(super) fn spawn_maintenance_task(&self) {
        let Some(interval_duration) = self.config.maintenance_interval else {
            return;
        };
        if interval_duration.is_zero() {
            if !self.maintenance_started.swap(true, AcqRel) {
                tracing::warn!("Disabled WebSocket pool maintenance with zero interval");
            }
            return;
        }
        if self.shutdown.is_cancelled() {
            return;
        }
        let Ok(_handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self
            .maintenance_started
            .compare_exchange(false, true, AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let inner = Arc::downgrade(&self.inner);
        let config = self.config;
        let tasks = self.tasks.clone();
        let shutdown = self.shutdown.clone();
        let capacity_changed = self.capacity_changed.clone();
        let account_concurrency = self.account_concurrency.clone();
        let maintenance_started = Arc::clone(&self.maintenance_started);
        let request_tuning = self.request_tuning.clone();
        drop(self.tasks.spawn(async move {
            let mut interval = tokio::time::interval(interval_duration);
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => {}
                }
                let Some(inner) = inner.upgrade() else {
                    break;
                };
                let pool = CodexWebSocketPool {
                    inner,
                    config,
                    tasks: tasks.clone(),
                    shutdown: shutdown.clone(),
                    capacity_changed: capacity_changed.clone(),
                    account_concurrency: account_concurrency.clone(),
                    maintenance_started: Arc::clone(&maintenance_started),
                    request_tuning: request_tuning.clone(),
                };
                if pool.is_shutdown().await {
                    break;
                }
                pool.maintain_idle_connections().await;
            }
        }));
    }

    /// 返回连接池是否已进入关闭状态。
    pub async fn is_shutdown(&self) -> bool {
        self.lock_state().shutting_down
    }

    async fn take_expired_slots(&self) -> Vec<RetiredConnection> {
        let mut close = Vec::new();
        let now = Instant::now();
        let mut state = self.lock_state();
        if state.shutting_down || !self.config.enabled {
            return close;
        }
        let keys = state
            .slots
            .iter()
            .filter_map(|(key, slot)| match slot {
                WebSocketPoolSlot::Idle { connection, .. }
                    if should_close_idle_connection(connection, now, self.max_age())
                        || (key.managed_state_expired()
                            && connection.continuation.latest_response_id().is_none()) =>
                {
                    Some(key.clone())
                }
                WebSocketPoolSlot::Busy(reservation)
                    if now.duration_since(reservation.reserved_at) >= self.max_age() =>
                {
                    Some(key.clone())
                }
                WebSocketPoolSlot::Connecting(connecting)
                    if now.duration_since(connecting.started_at) >= self.max_age() =>
                {
                    Some(key.clone())
                }
                WebSocketPoolSlot::Idle { .. }
                | WebSocketPoolSlot::Busy(_)
                | WebSocketPoolSlot::Connecting(_)
                | WebSocketPoolSlot::Closing(_) => None,
            })
            .collect::<Vec<_>>();
        for key in keys {
            match state.slots.remove(&key) {
                Some(WebSocketPoolSlot::Idle { connection, .. }) => {
                    let observation = if connection.websocket.is_closed() {
                        connection.websocket.observation()
                    } else {
                        connection.websocket.observation().with_exit_reason(
                            if key.managed_state_expired() {
                                "managed_turn_state_expired"
                            } else {
                                "max_age_expired"
                            },
                        )
                    };
                    let reason = match observation.exit_reason() {
                        "managed_turn_state_expired" => Some("managed_turn_state_expired"),
                        "max_age_expired" => Some("max_age_expired"),
                        _ => None,
                    };
                    close.push(Self::retire_connection(
                        &mut state,
                        key,
                        *connection,
                        reason,
                    ));
                }
                Some(WebSocketPoolSlot::Busy(mut reservation)) => {
                    // Still generating: retain its capacity until the response returns.
                    reservation.retire_on_return = true;
                    state
                        .slots
                        .insert(key, WebSocketPoolSlot::Busy(reservation));
                }
                Some(WebSocketPoolSlot::Connecting(connecting)) => {
                    connecting.cancel(None);
                    // Keep the reservation until the opening task observes the
                    // cancellation and releases it. The account slot remains
                    // occupied while a raced opening can still complete.
                    state
                        .slots
                        .insert(key, WebSocketPoolSlot::Connecting(connecting));
                }
                Some(WebSocketPoolSlot::Closing(id)) => {
                    state.slots.insert(key, WebSocketPoolSlot::Closing(id));
                }
                None => {}
            }
        }
        let account_ids = state
            .slots
            .keys()
            .map(|key| key.account_id().to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        for account_id in account_ids {
            close.extend(self.trim_account_idle(&mut state, &account_id, None));
        }
        close
    }
}

fn take_retired_slots(
    state: &mut WebSocketPoolState,
    keys: Vec<(CodexWebSocketPoolKey, Option<CodexEgressError>)>,
) -> Vec<RetiredConnection> {
    let mut idle_connections = Vec::new();
    for (key, local_egress) in keys {
        match state.slots.remove(&key) {
            Some(WebSocketPoolSlot::Idle { connection }) => {
                idle_connections.push(CodexWebSocketPool::retire_connection(
                    state,
                    key,
                    *connection,
                    Some("account_retired"),
                ));
            }
            Some(WebSocketPoolSlot::Connecting(connecting)) => {
                connecting.cancel(local_egress);
                // The opening can race with cancellation and already own a physical socket.
                state
                    .slots
                    .insert(key, WebSocketPoolSlot::Connecting(connecting));
            }
            Some(WebSocketPoolSlot::Closing(id)) => {
                state.slots.insert(key, WebSocketPoolSlot::Closing(id));
            }
            // Keep a busy reservation until its response finishes, but fence its return.
            Some(WebSocketPoolSlot::Busy(mut reservation)) => {
                reservation.retire_on_return = true;
                state
                    .slots
                    .insert(key, WebSocketPoolSlot::Busy(reservation));
            }
            None => {}
        }
    }
    idle_connections
}
