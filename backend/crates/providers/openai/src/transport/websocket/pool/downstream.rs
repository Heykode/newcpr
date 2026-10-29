//! Downstream lifetime supervision retains capacity until the physical socket exits.
use super::capacity::RetiredConnection;
use super::state::{WebSocketPoolSlot, WebSocketPoolState};
use super::{CodexWebSocketPool, CodexWebSocketPoolKey};
use gateway_core::lifecycle::CancellationToken;
use std::sync::Arc;

impl CodexWebSocketPool {
    pub(crate) fn register_downstream(
        &self,
        key: &CodexWebSocketPoolKey,
        cancellation: &CancellationToken,
    ) {
        if key.downstream_connection_id.is_empty() {
            return;
        }
        let connection_id = key.downstream_connection_id.clone();
        {
            let mut state = self.lock_state();
            if state.shutting_down || state.downstream_connections.contains_key(&connection_id) {
                return;
            }
            state
                .downstream_connections
                .insert(connection_id.clone(), cancellation.clone());
        }
        let inner = Arc::downgrade(&self.inner);
        let cancellation = cancellation.clone();
        let shutdown = self.shutdown.clone();
        let config = self.config;
        let tasks = self.tasks.clone();
        let capacity_changed = self.capacity_changed.clone();
        let account_concurrency = self.account_concurrency.clone();
        let maintenance_started = Arc::clone(&self.maintenance_started);
        let request_tuning = self.request_tuning.clone();
        drop(self.tasks.spawn(async move {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return,
                () = cancellation.cancelled() => {}
            }
            let Some(inner) = inner.upgrade() else { return };
            let pool = CodexWebSocketPool {
                inner,
                config,
                tasks,
                shutdown,
                capacity_changed,
                account_concurrency,
                maintenance_started,
                request_tuning,
            };
            let close = {
                let mut state = pool.lock_state();
                let close = state.take_downstream_connections(
                    &connection_id,
                    None,
                    "downstream_connection_closed",
                );
                state.downstream_connections.remove(&connection_id);
                close
            };
            pool.close_retired_connections(close).await;
            pool.capacity_changed.send_replace(());
        }));
    }

    pub(crate) async fn release_downstream_connections(&self, connection_id: &str) {
        let close = self.lock_state().take_downstream_connections(
            connection_id,
            None,
            "downstream_rebound",
        );
        self.close_retired_connections(close).await;
        self.capacity_changed.send_replace(());
    }
}

impl WebSocketPoolState {
    pub(super) fn has_downstream_binding(&self, key: &CodexWebSocketPoolKey) -> bool {
        self.downstream_connections
            .get(&key.downstream_connection_id)
            .is_some_and(|token| !token.is_cancelled())
    }

    pub(super) fn downstream_closed(&self, key: &CodexWebSocketPoolKey) -> bool {
        self.downstream_connections
            .get(&key.downstream_connection_id)
            .is_some_and(CancellationToken::is_cancelled)
    }

    pub(super) fn take_downstream_connections(
        &mut self,
        connection_id: &str,
        keep: Option<&CodexWebSocketPoolKey>,
        reason: &'static str,
    ) -> Vec<RetiredConnection> {
        // A lane identifier alone is not proof of an owned downstream lifetime.
        if connection_id.is_empty() || !self.downstream_connections.contains_key(connection_id) {
            return Vec::new();
        }
        let keys = self
            .slots
            .keys()
            .filter(|key| key.downstream_connection_id == connection_id && Some(*key) != keep)
            .cloned()
            .collect::<Vec<_>>();
        let mut close = Vec::new();
        for key in keys {
            match self.slots.remove(&key) {
                Some(WebSocketPoolSlot::Idle { connection, .. }) => {
                    close.push(CodexWebSocketPool::retire_connection(
                        self,
                        key,
                        *connection,
                        Some(reason),
                    ));
                }
                Some(WebSocketPoolSlot::Connecting(connecting)) => {
                    connecting.cancel(None);
                    self.slots
                        .insert(key, WebSocketPoolSlot::Connecting(connecting));
                }
                Some(WebSocketPoolSlot::Busy(mut reservation)) => {
                    reservation.retire_on_return = true;
                    self.slots.insert(key, WebSocketPoolSlot::Busy(reservation));
                }
                Some(WebSocketPoolSlot::Closing(id)) => {
                    self.slots.insert(key, WebSocketPoolSlot::Closing(id));
                }
                None => {}
            }
        }
        close
    }
}
