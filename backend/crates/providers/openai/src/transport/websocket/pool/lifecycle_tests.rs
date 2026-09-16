use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use futures::{Sink, Stream, task::AtomicWaker};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_util::sync::CancellationToken;

use gateway_core::{routing::ConfigRevision, runtime::AccountConcurrencyHandle};

use super::{
    CodexWebSocketConnectionMetadata, CodexWebSocketPool, CodexWebSocketPoolKey,
    PooledWebSocketConnection, WebSocketContinuationState, WebSocketPoolAcquire,
    WebSocketPoolConnectLease, WebSocketPoolConnectOutcome, WebSocketPoolLease,
    state::WebSocketPoolSlot,
};
use crate::transport::{
    egress::CodexEgressError,
    websocket::pump::{PumpKeepalive, PumpLogContext, PumpedWebSocket, WEBSOCKET_CLOSE_TIMEOUT},
};

#[derive(Default)]
struct SocketControl {
    close_started: CancellationToken,
    destroyed: CancellationToken,
    allow_flush: AtomicBool,
    flush_waker: AtomicWaker,
}

impl SocketControl {
    fn release_close(&self) {
        self.allow_flush.store(true, Ordering::SeqCst);
        self.flush_waker.wake();
    }
}

struct ControlledSocket(Arc<SocketControl>);

impl Stream for ControlledSocket {
    type Item = Result<Message, tungstenite::Error>;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Pending
    }
}

impl Sink<Message> for ControlledSocket {
    type Error = tungstenite::Error;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, message: Message) -> Result<(), Self::Error> {
        assert!(
            matches!(message, Message::Close(_)),
            "lifecycle tests send no payload"
        );
        self.0.close_started.cancel();
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.flush_waker.register(cx.waker());
        if self.0.allow_flush.load(Ordering::SeqCst) {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }
}

impl Drop for ControlledSocket {
    fn drop(&mut self) {
        self.0.destroyed.cancel();
    }
}

fn pool() -> CodexWebSocketPool {
    pool_with_limit(1)
}

fn pool_with_limit(limit: u32) -> CodexWebSocketPool {
    let limits = AccountConcurrencyHandle::new(
        ConfigRevision::new(1).expect("valid revision"),
        [(
            "account".to_owned(),
            std::num::NonZeroU32::new(limit).expect("non-zero limit"),
        )]
        .into(),
    );
    CodexWebSocketPool::new(Duration::from_secs(60)).with_account_concurrency(limits)
}

fn key(index: u32) -> CodexWebSocketPoolKey {
    CodexWebSocketPoolKey::new(
        "https://upstream.invalid",
        "account",
        format!("chain-{index}"),
    )
}

fn opening(pool: &CodexWebSocketPool, key: &CodexWebSocketPoolKey) -> WebSocketPoolConnectLease {
    let (decision, close) = pool.acquire_once(key, None);
    assert!(close.is_empty());
    match decision {
        Some(WebSocketPoolAcquire::Connect(lease)) => lease,
        _ => panic!("expected opening capacity"),
    }
}

fn connection() -> (PooledWebSocketConnection, Arc<SocketControl>) {
    let control = Arc::new(SocketControl::default());
    let connection = PooledWebSocketConnection {
        websocket: PumpedWebSocket::new(
            Box::new(ControlledSocket(Arc::clone(&control))),
            PumpKeepalive::disabled(),
            PumpLogContext::new(None, None),
            None,
        ),
        metadata: CodexWebSocketConnectionMetadata {
            turn_state: None,
            set_cookie_headers: Vec::new(),
            rate_limit_headers: Vec::new(),
            response_metadata: Default::default(),
            diagnostics: Default::default(),
        },
        continuation: WebSocketContinuationState::default(),
        created_at: tokio::time::Instant::now(),
    };
    (connection, control)
}

async fn connected(
    pool: &CodexWebSocketPool,
    key: &CodexWebSocketPoolKey,
) -> (
    PooledWebSocketConnection,
    WebSocketPoolLease,
    Arc<SocketControl>,
) {
    let opening = opening(pool, key);
    let (connection, control) = connection();
    match opening.connected_reserved(connection).await {
        Ok((connection, lease)) => (*connection, lease, control),
        Err(_) => panic!("opening must keep its reservation"),
    }
}

fn assert_closing(pool: &CodexWebSocketPool, key: &CodexWebSocketPoolKey) {
    let state = pool.lock_state();
    assert!(matches!(
        state.slots.get(key),
        Some(WebSocketPoolSlot::Closing(_))
    ));
    assert_eq!(CodexWebSocketPool::account_slots(&state, "account"), 1);
}

#[tokio::test(start_paused = true)]
async fn discard_retains_capacity_until_the_socket_is_destroyed() {
    let pool = pool();
    let (connection, lease, control) = connected(&pool, &key(1)).await;
    let observation = connection.websocket.observation();
    let closing = lease.discard_with_observation(connection.websocket, observation);
    control.close_started.cancelled().await;
    assert_closing(&pool, &key(1));
    assert!(!control.destroyed.is_cancelled());

    let next_key = key(2);
    let mut waiting = Box::pin(pool.acquire(&next_key, None));
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    control.release_close();
    let next = timeout(Duration::from_secs(1), waiting)
        .await
        .expect("capacity released");
    assert!(
        control.destroyed.is_cancelled(),
        "new admission must follow socket destruction"
    );
    assert!(matches!(next, WebSocketPoolAcquire::Connect(_)));
    closing.await.expect("close task");
    drop(next);
    pool.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn permanently_pending_close_is_bounded_and_does_not_block_shutdown() {
    let pool = pool();
    let (connection, lease, control) = connected(&pool, &key(1)).await;
    lease.put(connection).await;
    let retiring_pool = pool.clone();
    let retiring = tokio::spawn(async move { retiring_pool.evict_account("account").await });
    control.close_started.cancelled().await;
    assert_closing(&pool, &key(1));
    assert!(!control.destroyed.is_cancelled());

    timeout(
        WEBSOCKET_CLOSE_TIMEOUT + Duration::from_secs(1),
        pool.shutdown(),
    )
    .await
    .expect("shutdown must abort a permanently pending close");
    retiring.await.expect("retirement finishes");
    assert!(control.destroyed.is_cancelled());
    assert!(pool.lock_state().slots.is_empty());
}

#[tokio::test(start_paused = true)]
async fn cancelled_discard_waiter_leaves_a_bounded_managed_close() {
    let pool = pool();
    let (connection, lease, control) = connected(&pool, &key(1)).await;
    let observation = connection.websocket.observation();
    let closing = lease.discard_with_observation(connection.websocket, observation);
    let waiter = tokio::spawn(async move {
        let _ = closing.await;
    });
    control.close_started.cancelled().await;
    waiter.abort();
    assert!(waiter.await.expect_err("cancelled waiter").is_cancelled());
    assert_closing(&pool, &key(1));
    assert!(!control.destroyed.is_cancelled());

    let next_key = key(2);
    let next = timeout(
        WEBSOCKET_CLOSE_TIMEOUT + Duration::from_secs(1),
        pool.acquire(&next_key, None),
    )
    .await
    .expect("detached close must release capacity");
    assert!(control.destroyed.is_cancelled());
    assert!(matches!(next, WebSocketPoolAcquire::Connect(_)));
    drop(next);
    pool.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn cancelled_opening_success_is_closed_before_its_capacity_is_released() {
    let pool = pool();
    let connect_lease = opening(&pool, &key(1));
    let shared = match pool.acquire_once(&key(1), None).0 {
        Some(WebSocketPoolAcquire::Wait(waiter)) => waiter,
        _ => panic!("same opening must be shared"),
    };
    let local_error = CodexEgressError::ConfigurationChanged;
    connect_lease
        .outcome
        .send_replace(WebSocketPoolConnectOutcome::LocalEgress(local_error));
    connect_lease.cancellation_token().cancel();
    let (connection, control) = connection();
    let closing = match connect_lease.connected_reserved(connection).await {
        Err((closing, error)) => {
            assert_eq!(error, Some(local_error));
            closing
        }
        Ok(_) => panic!("cancelled opening cannot deliver a socket"),
    };
    assert_eq!(
        shared.wait().await,
        WebSocketPoolConnectOutcome::LocalEgress(local_error)
    );
    control.close_started.cancelled().await;
    assert_closing(&pool, &key(1));
    assert!(pool.acquire_once(&key(2), None).0.is_none());
    assert!(!control.destroyed.is_cancelled());

    control.release_close();
    closing.await.expect("rejected socket closes");
    assert!(control.destroyed.is_cancelled());
    drop(opening(&pool, &key(2)));
    pool.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn dropped_busy_lease_aborts_the_socket_before_releasing_capacity() {
    let pool = pool();
    let (connection, lease, control) = connected(&pool, &key(1)).await;
    drop(lease);
    assert_closing(&pool, &key(1));
    assert!(!control.destroyed.is_cancelled());
    assert!(pool.acquire_once(&key(2), None).0.is_none());

    let next_key = key(2);
    let next = timeout(Duration::from_secs(1), pool.acquire(&next_key, None))
        .await
        .expect("aborted pump releases capacity");
    assert!(control.destroyed.is_cancelled());
    assert!(matches!(next, WebSocketPoolAcquire::Connect(_)));
    drop(next);
    drop(connection);
    pool.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn idle_shutdown_bounds_close_without_a_retirement_task() {
    let pool = pool();
    let (connection, lease, control) = connected(&pool, &key(1)).await;
    lease.put(connection).await;
    timeout(
        WEBSOCKET_CLOSE_TIMEOUT + Duration::from_secs(1),
        pool.shutdown(),
    )
    .await
    .expect("idle socket close is bounded");
    assert!(control.close_started.is_cancelled());
    assert!(control.destroyed.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn capacity_wait_retires_only_one_idle_owner_while_close_is_pending() {
    let pool = pool_with_limit(3);
    let mut controls = Vec::new();
    for index in 1..=3 {
        let (mut connection, lease, control) = connected(&pool, &key(index)).await;
        connection
            .continuation
            .record_completed(format!("response-{index}"));
        lease.put(connection).await;
        controls.push((index, control));
    }
    let next_key = key(4);
    let mut waiting = Box::pin(pool.acquire(&next_key, None));
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    tokio::task::yield_now().await;
    for _ in 0..3 {
        tokio::time::advance(Duration::from_millis(60)).await;
        assert!(futures::poll!(waiting.as_mut()).is_pending());
        let state = pool.lock_state();
        assert_eq!(
            state
                .slots
                .values()
                .filter(|slot| matches!(slot, WebSocketPoolSlot::Closing(_)))
                .count(),
            1,
            "a pending close already provides the replacement's future capacity",
        );
        assert_eq!(
            state
                .slots
                .values()
                .filter(|slot| matches!(slot, WebSocketPoolSlot::Idle { .. }))
                .count(),
            2,
        );
    }
    for (_, control) in &controls {
        control.release_close();
    }
    let next = timeout(Duration::from_secs(1), waiting)
        .await
        .expect("closing owner releases capacity");
    assert!(matches!(next, WebSocketPoolAcquire::Connect(_)));
    assert_eq!(pool.lock_state().slots.len(), 3);
    drop(next);
    for (index, control) in controls {
        if control.close_started.is_cancelled() {
            continue;
        }
        match pool
            .acquire(&key(index), Some(&format!("response-{index}")))
            .await
        {
            WebSocketPoolAcquire::Reused { connection, lease } => {
                lease.put(*connection).await;
            }
            _ => panic!("unrelated continuation must retain its exact owner"),
        }
    }
    pool.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn account_eviction_counts_raced_opening_until_its_socket_is_destroyed() {
    let pool = pool();
    let first = opening(&pool, &key(1));
    let (connection, control) = connection();
    pool.evict_account("account").await;
    assert_eq!(
        CodexWebSocketPool::account_slots(&pool.lock_state(), "account"),
        1
    );
    let closing = match first.connected_reserved(connection).await {
        Err((closing, _)) => closing,
        Ok(_) => panic!("evicted opening cannot deliver a socket"),
    };
    control.close_started.cancelled().await;
    assert_closing(&pool, &key(1));
    assert!(!control.destroyed.is_cancelled());
    let next_key = key(2);
    let mut waiting = Box::pin(pool.acquire(&next_key, None));
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    control.release_close();
    let next = timeout(Duration::from_secs(1), waiting)
        .await
        .expect("socket destruction releases capacity");
    assert!(control.destroyed.is_cancelled());
    assert!(matches!(next, WebSocketPoolAcquire::Connect(_)));
    closing.await.expect("retired opening closes");
    drop(next);
    pool.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn same_key_after_eviction_waits_for_cancelled_opening_to_release_capacity() {
    let pool = pool_with_limit(2);
    let key = key(1);
    let first = opening(&pool, &key);
    let first_id = first.id;
    let shared = match pool.acquire_once(&key, None).0 {
        Some(WebSocketPoolAcquire::Wait(waiter)) => waiter,
        _ => panic!("active opening must be shared"),
    };
    let (connection, control) = connection();
    pool.evict_account("account").await;
    assert_eq!(shared.wait().await, WebSocketPoolConnectOutcome::Failed);

    let mut waiting = Box::pin(pool.acquire(&key, None));
    assert!(
        futures::poll!(waiting.as_mut()).is_pending(),
        "new requests must not subscribe to an already-cancelled opening",
    );
    let closing = match first.connected_reserved(connection).await {
        Err((closing, _)) => closing,
        Ok(_) => panic!("evicted opening cannot deliver a socket"),
    };
    control.close_started.cancelled().await;
    assert_closing(&pool, &key);
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    assert!(!control.destroyed.is_cancelled());

    control.release_close();
    let next = timeout(Duration::from_secs(1), waiting)
        .await
        .expect("same-key request receives fresh capacity after destruction");
    assert!(control.destroyed.is_cancelled());
    match next {
        WebSocketPoolAcquire::Connect(lease) => {
            assert_ne!(lease.id, first_id);
            drop(lease);
        }
        _ => panic!("same-key request must get a fresh opening"),
    }
    closing.await.expect("evicted socket closes");
    pool.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn cancelled_opening_does_not_trigger_another_idle_retirement() {
    let pool = pool_with_limit(2);
    let (connection, lease, control) = connected(&pool, &key(1)).await;
    lease.put(connection).await;
    let connecting = opening(&pool, &key(2));
    connecting.cancellation_token().cancel();
    let next_key = key(3);
    let mut waiting = Box::pin(pool.acquire(&next_key, None));
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    assert!(matches!(
        pool.lock_state().slots.get(&key(1)),
        Some(WebSocketPoolSlot::Idle { .. })
    ));
    assert!(!control.close_started.is_cancelled());
    drop(connecting);
    let next = timeout(Duration::from_secs(1), waiting)
        .await
        .expect("cancelled opening returns its capacity");
    assert!(matches!(next, WebSocketPoolAcquire::Connect(_)));
    assert!(!control.close_started.is_cancelled());
    drop(next);
    control.release_close();
    pool.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn repeated_retirement_preserves_the_original_local_opening_failure() {
    let pool = pool();
    let connecting = opening(&pool, &key(1));
    let shared = match pool.acquire_once(&key(1), None).0 {
        Some(WebSocketPoolAcquire::Wait(waiter)) => waiter,
        _ => panic!("same opening must be shared"),
    };
    let local_error = CodexEgressError::SourceDisabled;
    connecting
        .outcome
        .send_replace(WebSocketPoolConnectOutcome::LocalEgress(local_error));
    pool.evict_account("account").await;
    pool.evict_account("account").await;
    assert!(pool.acquire_once(&key(1), None).0.is_none());
    tokio::time::advance(Duration::from_secs(61)).await;
    pool.maintain_idle_connections().await;
    assert_eq!(
        shared.wait().await,
        WebSocketPoolConnectOutcome::LocalEgress(local_error),
    );
    assert_eq!(pool.lock_state().slots.len(), 1);
    drop(connecting);
    pool.shutdown().await;
}
