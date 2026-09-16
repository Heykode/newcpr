//! WebSocket 连接池 reservation、handoff 与 lease 生命周期。

use tokio::{sync::watch, task::JoinHandle, time::Instant};
use tokio_util::{
    sync::CancellationToken,
    task::{TaskTracker, task_tracker::TaskTrackerToken},
};
use uuid::Uuid;

use super::state::{CodexWebSocketPoolKey, PooledWebSocketConnection, WebSocketPoolReservation};
use super::{CodexWebSocketPool, WebSocketPoolBypassReason};
use crate::transport::egress::CodexEgressError;
use crate::transport::websocket::pump::{
    PumpTermination, PumpedWebSocket, WebSocketConnectionObservation,
};

pub(crate) enum WebSocketPoolAcquire {
    Reused {
        connection: Box<PooledWebSocketConnection>,
        lease: WebSocketPoolLease,
    },
    Connect(WebSocketPoolConnectLease),
    Wait(WebSocketPoolConnectWaiter),
    ContinuationLost(WebSocketConnectionObservation),
    LocalEgress(CodexEgressError),
    Bypass(WebSocketPoolBypassReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebSocketPoolConnectOutcome {
    Pending,
    Ready,
    Failed,
    LocalEgress(CodexEgressError),
}

pub(crate) struct WebSocketPoolConnectWaiter {
    pub(super) receiver: watch::Receiver<WebSocketPoolConnectOutcome>,
    pub(super) started_at: Instant,
}

impl WebSocketPoolConnectWaiter {
    pub(crate) fn started_at(&self) -> Instant {
        self.started_at
    }

    pub(crate) async fn wait(mut self) -> WebSocketPoolConnectOutcome {
        loop {
            let outcome = *self.receiver.borrow_and_update();
            if outcome != WebSocketPoolConnectOutcome::Pending {
                return outcome;
            }
            if self.receiver.changed().await.is_err() {
                return WebSocketPoolConnectOutcome::Failed;
            }
        }
    }
}

pub(crate) struct WebSocketPoolConnectLease {
    pool: CodexWebSocketPool,
    key: CodexWebSocketPoolKey,
    pub(super) id: Uuid,
    pub(super) started_at: Instant,
    pub(super) outcome: watch::Sender<WebSocketPoolConnectOutcome>,
    pub(super) cancellation: CancellationToken,
    // slot 分配时即注册，封闭 acquire 与后台 task spawn 之间的 shutdown 竞态。
    _task_registration: TaskTrackerToken,
    armed: bool,
}

impl WebSocketPoolConnectLease {
    pub(super) fn reserve(pool: CodexWebSocketPool, key: CodexWebSocketPoolKey) -> Self {
        let (outcome, _) = watch::channel(WebSocketPoolConnectOutcome::Pending);
        let cancellation = pool.shutdown.child_token();
        let task_registration = pool.tasks.token();
        Self {
            pool,
            key,
            id: Uuid::new_v4(),
            started_at: Instant::now(),
            outcome,
            cancellation,
            _task_registration: task_registration,
            armed: true,
        }
    }

    pub(crate) fn started_at(&self) -> Instant {
        self.started_at
    }

    pub(crate) fn key(&self) -> &CodexWebSocketPoolKey {
        &self.key
    }

    pub(crate) fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub(crate) fn local_egress_error(&self) -> Option<CodexEgressError> {
        match *self.outcome.borrow() {
            WebSocketPoolConnectOutcome::LocalEgress(error) => Some(error),
            _ => None,
        }
    }

    pub(crate) async fn connected_reserved(
        mut self,
        connection: PooledWebSocketConnection,
    ) -> Result<
        (Box<PooledWebSocketConnection>, WebSocketPoolLease),
        (JoinHandle<()>, Option<CodexEgressError>),
    > {
        let result = self
            .pool
            .finish_connect_reserved(&self.key, self.id, connection)
            .await;
        if result.is_ok() {
            self.outcome
                .send_replace(WebSocketPoolConnectOutcome::Ready);
        } else {
            self.publish_failure(None);
        }
        self.armed = false;
        result.map_err(|closing| (closing, self.local_egress_error()))
    }

    pub(crate) async fn failed(self) {
        self.failed_with_egress(None).await;
    }

    pub(crate) async fn failed_with_egress(mut self, error: Option<CodexEgressError>) {
        self.publish_failure(error);
        self.cancellation.cancel();
        // 先唤醒共享等待者，pool mutex 清理不得延迟前台 transport 决策。
        self.pool.fail_connect(&self.key, self.id).await;
        self.armed = false;
    }

    fn publish_failure(&self, error: Option<CodexEgressError>) {
        self.outcome.send_if_modified(|outcome| {
            // Retirement may already have published the decisive local failure.
            if matches!(outcome, WebSocketPoolConnectOutcome::LocalEgress(_)) {
                return false;
            }
            *outcome = error.map_or(
                WebSocketPoolConnectOutcome::Failed,
                WebSocketPoolConnectOutcome::LocalEgress,
            );
            true
        });
    }
}

impl Drop for WebSocketPoolConnectLease {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.publish_failure(None);
        self.cancellation.cancel();
        self.pool.fail_connect_now(&self.key, self.id);
    }
}

pub(crate) struct WebSocketPoolLease {
    pool: CodexWebSocketPool,
    key: CodexWebSocketPoolKey,
    pub(super) reservation: WebSocketPoolReservation,
    _task_registration: TaskTrackerToken,
    termination: Option<PumpTermination>,
    armed: bool,
}

impl WebSocketPoolLease {
    pub(super) fn reserve(
        pool: CodexWebSocketPool,
        key: CodexWebSocketPoolKey,
        latest_response_id: Option<&str>,
    ) -> Self {
        let task_registration = pool.tasks.token();
        Self {
            pool,
            key,
            reservation: WebSocketPoolReservation {
                id: Uuid::new_v4(),
                reserved_at: Instant::now(),
                latest_response_id: latest_response_id.map(str::to_string),
                retire_on_return: false,
            },
            _task_registration: task_registration,
            termination: None,
            armed: true,
        }
    }

    pub(super) fn with_websocket(mut self, websocket: &PumpedWebSocket) -> Self {
        self.termination = Some(websocket.termination_handle());
        self
    }

    pub(crate) fn stream_task_context(&self) -> (TaskTracker, CancellationToken) {
        (self.pool.tasks.clone(), self.pool.shutdown.clone())
    }

    pub(crate) async fn put(mut self, connection: PooledWebSocketConnection) {
        self.pool
            .put_reserved(&self.key, self.reservation.id, connection)
            .await;
        self.armed = false;
    }

    pub(crate) fn discard_with_observation(
        mut self,
        websocket: PumpedWebSocket,
        observation: WebSocketConnectionObservation,
    ) -> JoinHandle<()> {
        let closing = self.pool.discard_reserved_with_observation(
            &self.key,
            self.reservation.id,
            websocket,
            observation,
        );
        self.armed = false;
        closing
    }
}

impl Drop for WebSocketPoolLease {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(termination) = self.termination.take() {
            self.pool.discard_reserved_after_termination(
                &self.key,
                self.reservation.id,
                termination,
            );
        } else {
            self.pool
                .discard_reserved_now(&self.key, self.reservation.id);
        }
    }
}
