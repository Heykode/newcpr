//! Process-local bounded FIFO positions; Redis remains the admission authority.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::{FutureExt, future::poll_fn, pin_mut, select_biased, task::AtomicWaker};
use futures_timer::Delay;
use std::task::Poll;

use crate::error::{GatewayError, GatewayErrorKind};
use crate::policy::ClientApiKeyId;

#[derive(Default)]
struct State {
    queues: HashMap<ClientApiKeyId, VecDeque<Arc<AtomicWaker>>>,
    total: usize,
}

#[derive(Default)]
pub(super) struct KeyWaitQueue(Arc<Mutex<State>>);

impl KeyWaitQueue {
    pub fn has_waiters(&self, key: &ClientApiKeyId) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .queues
            .contains_key(key)
    }

    pub fn enqueue(
        &self,
        key: &ClientApiKeyId,
        limit: u32,
        deadline: Instant,
    ) -> Result<KeyWaitTicket, GatewayError> {
        if Instant::now() >= deadline {
            return Err(timeout());
        }
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.total >= 1024 || state.queues.get(key).map_or(0, VecDeque::len) >= limit as usize {
            return Err(GatewayError::new(
                GatewayErrorKind::ConcurrencyQueueFull,
                "client key concurrency wait queue is full",
            ));
        }
        let marker = Arc::new(AtomicWaker::new());
        state
            .queues
            .entry(key.clone())
            .or_default()
            .push_back(Arc::clone(&marker));
        state.total += 1;
        Ok(KeyWaitTicket {
            state: Arc::clone(&self.0),
            key: key.clone(),
            marker,
            deadline,
        })
    }
}

pub(super) struct KeyWaitTicket {
    state: Arc<Mutex<State>>,
    key: ClientApiKeyId,
    marker: Arc<AtomicWaker>,
    deadline: Instant,
}

impl KeyWaitTicket {
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    pub async fn retry(&self) -> Result<(), GatewayError> {
        // Poll Redis only at the head. Already-waiting followers advance immediately
        // when the preceding ticket leaves, rather than paying a delay per position.
        Delay::new(Duration::from_millis(100).min(self.remaining())).await;
        if self.remaining().is_zero() {
            return Err(timeout());
        }
        let head = poll_fn(|cx| {
            self.marker.register(cx.waker());
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state
                .queues
                .get(&self.key)
                .and_then(VecDeque::front)
                .is_some_and(|head| Arc::ptr_eq(head, &self.marker))
            {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .fuse();
        let deadline = Delay::new(self.remaining()).fuse();
        pin_mut!(head, deadline);
        select_biased! {
            _ = deadline => Err(timeout()),
            _ = head => if self.remaining().is_zero() { Err(timeout()) } else { Ok(()) },
        }
    }
}

impl Drop for KeyWaitTicket {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = None;
        if let Some(queue) = state.queues.get_mut(&self.key) {
            queue.retain(|marker| !Arc::ptr_eq(marker, &self.marker));
            next = queue.front().cloned();
            if queue.is_empty() {
                state.queues.remove(&self.key);
            }
            state.total -= 1;
        }
        drop(state);
        if let Some(next) = next {
            next.wake();
        }
    }
}

pub(super) fn timeout() -> GatewayError {
    GatewayError::new(
        GatewayErrorKind::ConcurrencyQueueTimeout,
        "client key concurrency wait deadline elapsed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_capacity_and_cancelled_positions_are_reclaimed() {
        let queue = KeyWaitQueue::default();
        let key = ClientApiKeyId::new("synthetic-key").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let first = queue.enqueue(&key, 2, deadline).unwrap();
        let second = queue.enqueue(&key, 2, deadline).unwrap();
        assert!(
            matches!(queue.enqueue(&key, 2, deadline), Err(error) if error.kind() == GatewayErrorKind::ConcurrencyQueueFull)
        );
        drop(second);
        let third = queue.enqueue(&key, 2, deadline).unwrap();
        drop(first);
        futures::executor::block_on(third.retry()).unwrap();
        drop(third);
        assert!(!queue.has_waiters(&key));
        assert_eq!(queue.0.lock().unwrap().total, 0);
    }

    #[test]
    fn expired_positions_timeout_and_do_not_block_the_next_request() {
        let queue = KeyWaitQueue::default();
        let key = ClientApiKeyId::new("synthetic-key").unwrap();
        let ticket = queue
            .enqueue(&key, 1, Instant::now() + Duration::from_millis(5))
            .unwrap();
        assert_eq!(
            futures::executor::block_on(ticket.retry())
                .unwrap_err()
                .kind(),
            GatewayErrorKind::ConcurrencyQueueTimeout
        );
        drop(ticket);
        assert!(!queue.has_waiters(&key));
    }

    #[test]
    fn waiting_followers_are_woken_in_fifo_order_and_global_capacity_is_bounded() {
        let queue = KeyWaitQueue::default();
        let key = ClientApiKeyId::new("synthetic-key").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let first = queue.enqueue(&key, 1024, deadline).unwrap();
        let second = queue.enqueue(&key, 1024, deadline).unwrap();
        let third = queue.enqueue(&key, 1024, deadline).unwrap();
        futures::executor::block_on(async {
            let followers = async {
                second.retry().await.unwrap();
                drop(second);
                third.retry().await.unwrap();
            };
            let release_head = async {
                Delay::new(Duration::from_millis(150)).await;
                drop(first);
            };
            futures::join!(followers, release_head);
        });
        drop(third);
        let tickets: Vec<_> = (0..1024)
            .map(|_| queue.enqueue(&key, 1024, deadline).unwrap())
            .collect();
        let other = ClientApiKeyId::new("another-key").unwrap();
        assert!(
            matches!(queue.enqueue(&other, 1, deadline), Err(error) if error.kind() == GatewayErrorKind::ConcurrencyQueueFull)
        );
        drop(tickets);
        assert!(!queue.has_waiters(&key));
        assert!(queue.enqueue(&other, 1, deadline).is_ok());
    }
}
