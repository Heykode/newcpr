//! Account capacity includes idle, busy, opening and retiring connections.

use tokio::{task::JoinHandle, time::Instant};
use uuid::Uuid;

use super::super::pump::{PumpTermination, PumpedWebSocket, WebSocketConnectionObservation};
use super::{
    CodexWebSocketPool, CodexWebSocketPoolKey,
    state::{PooledWebSocketConnection, WebSocketPoolSlot, WebSocketPoolState},
};

pub(super) struct RetiredConnection {
    key: CodexWebSocketPoolKey,
    id: Uuid,
    websocket: PumpedWebSocket,
}

impl CodexWebSocketPool {
    pub(super) fn account_limit(&self, account_id: &str) -> Option<usize> {
        match &self.account_concurrency {
            // A pool created directly by transport tests/tools has no runtime
            // account directory. It must retain the historical unlimited
            // standalone behavior; production pools always attach the handle.
            None => Some(usize::MAX),
            Some(handle) => handle
                .load()
                .ok()
                .flatten()
                .and_then(|snapshot| snapshot.limit_for(account_id))
                .map(|limit| limit.get() as usize),
        }
    }

    pub(super) fn account_slots(state: &WebSocketPoolState, account_id: &str) -> usize {
        state
            .slots
            .keys()
            .filter(|key| key.account_id() == account_id)
            .count()
    }

    pub(super) fn account_retained_slots(state: &WebSocketPoolState, account_id: &str) -> usize {
        state
            .slots
            .iter()
            .filter(|(key, slot)| {
                key.account_id() == account_id
                    && match slot {
                        WebSocketPoolSlot::Closing(_) => false,
                        WebSocketPoolSlot::Connecting(connecting) => {
                            !connecting.cancellation.is_cancelled()
                        }
                        _ => true,
                    }
            })
            .count()
    }

    pub(super) fn retire_connection(
        state: &mut WebSocketPoolState,
        key: CodexWebSocketPoolKey,
        connection: PooledWebSocketConnection,
        reason: Option<&'static str>,
    ) -> RetiredConnection {
        let observation = connection.websocket.observation();
        let observation = match reason {
            Some(reason) => observation.with_exit_reason(reason),
            None => observation,
        };
        Self::retire_websocket(
            state,
            key,
            connection.websocket,
            connection.continuation.latest_response_id(),
            observation,
        )
    }

    pub(super) fn retire_websocket(
        state: &mut WebSocketPoolState,
        key: CodexWebSocketPoolKey,
        websocket: PumpedWebSocket,
        latest_response_id: Option<&str>,
        observation: WebSocketConnectionObservation,
    ) -> RetiredConnection {
        state.remember_continuation_loss(&key, latest_response_id, observation, Instant::now());
        let id = Uuid::new_v4();
        state
            .slots
            .insert(key.clone(), WebSocketPoolSlot::Closing(id));
        RetiredConnection { key, id, websocket }
    }

    pub(super) fn retire_oldest_idle(
        state: &mut WebSocketPoolState,
        account_id: &str,
        keep: Option<&CodexWebSocketPoolKey>,
    ) -> Option<RetiredConnection> {
        let key = state
            .slots
            .iter()
            .filter_map(|(key, slot)| match slot {
                WebSocketPoolSlot::Idle { connection }
                    if key.account_id() == account_id && keep != Some(key) =>
                {
                    Some((key.clone(), connection.created_at))
                }
                _ => None,
            })
            .min_by_key(|(_, created_at)| *created_at)
            .map(|(key, _)| key)?;
        let Some(WebSocketPoolSlot::Idle { connection }) = state.slots.remove(&key) else {
            return None;
        };
        Some(Self::retire_connection(
            state,
            key,
            *connection,
            Some("account_capacity_reclaimed"),
        ))
    }

    pub(super) fn start_closing(&self, connections: Vec<RetiredConnection>) -> Vec<JoinHandle<()>> {
        connections
            .into_iter()
            .map(|retired| self.start_closing_one(retired))
            .collect()
    }

    pub(super) fn start_closing_one(&self, retired: RetiredConnection) -> JoinHandle<()> {
        self.start_closing_socket(retired.websocket, Some((retired.key, retired.id)))
    }

    pub(super) async fn close_retired_connections(&self, connections: Vec<RetiredConnection>) {
        for task in self.start_closing(connections) {
            let _ = task.await;
        }
    }

    pub(super) fn start_closing_socket(
        &self,
        websocket: PumpedWebSocket,
        reservation: Option<(CodexWebSocketPoolKey, Uuid)>,
    ) -> JoinHandle<()> {
        let pool = self.clone();
        // Caller cancellation only drops the join handle; the registered close keeps ownership.
        self.tasks.spawn(async move {
            websocket.close().await;
            pool.finish_closing(reservation);
        })
    }

    pub(super) fn start_aborting_socket(
        &self,
        termination: PumpTermination,
        reservation: Option<(CodexWebSocketPoolKey, Uuid)>,
    ) {
        termination.abort();
        let pool = self.clone();
        drop(self.tasks.spawn(async move {
            termination.wait().await;
            pool.finish_closing(reservation);
        }));
    }

    fn finish_closing(&self, reservation: Option<(CodexWebSocketPoolKey, Uuid)>) {
        let Some((key, id)) = reservation else {
            return;
        };
        let mut state = self.lock_state();
        if matches!(state.slots.get(&key), Some(WebSocketPoolSlot::Closing(current)) if *current == id)
        {
            state.slots.remove(&key);
        }
        self.capacity_changed.send_replace(());
    }

    pub(super) fn trim_account_idle(
        &self,
        state: &mut WebSocketPoolState,
        account_id: &str,
        keep: Option<&CodexWebSocketPoolKey>,
    ) -> Vec<RetiredConnection> {
        let limit = self.account_limit(account_id).unwrap_or(0);
        let retained = Self::account_retained_slots(state, account_id);
        let mut close = Vec::new();
        for _ in limit..retained {
            if let Some(retired) = Self::retire_oldest_idle(state, account_id, keep) {
                close.push(retired);
            } else {
                break;
            }
        }
        close
    }
}
