//! Request-local opening recovery, independent of account routing budgets.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_OPENINGS: u32 = 4;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
struct State {
    started: Option<Instant>,
    openings: u32,
    completed: bool,
}

#[derive(Debug, Clone)]
pub struct ConnectionBudget {
    state: Arc<Mutex<State>>,
    now: fn() -> Instant,
}

impl Default for ConnectionBudget {
    fn default() -> Self {
        Self::with_clock(Instant::now)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("connection recovery budget exhausted")]
pub struct ConnectionBudgetExhausted;

impl ConnectionBudget {
    #[must_use]
    pub fn with_clock(now: fn() -> Instant) -> Self {
        Self {
            state: Arc::default(),
            now,
        }
    }

    /// Count HTTP sends and new WS handshakes once, including handshakes that
    /// continue in the pool after foreground fallback. WS reuse does not count.
    pub fn begin(&self) -> Result<Option<Duration>, ConnectionBudgetExhausted> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.completed {
            return Ok(None);
        }
        let started = *state.started.get_or_insert_with(self.now);
        let remaining =
            STARTUP_TIMEOUT.saturating_sub((self.now)().saturating_duration_since(started));
        if state.openings >= MAX_OPENINGS || remaining.is_zero() {
            return Err(ConnectionBudgetExhausted);
        }
        state.openings += 1;
        Ok(Some(remaining))
    }

    /// Sending (including an ambiguous send) permanently closes this recovery window.
    pub fn complete(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .completed = true;
    }

    #[must_use]
    pub fn remaining(&self) -> Option<Duration> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.completed || state.openings >= MAX_OPENINGS {
            return None;
        }
        let remaining = state.started.map_or(STARTUP_TIMEOUT, |started| {
            STARTUP_TIMEOUT.saturating_sub((self.now)().saturating_duration_since(started))
        });
        (!remaining.is_zero()).then_some(remaining)
    }

    #[must_use]
    pub fn retry_delay(&self, retries: u32, request_id: &str) -> Option<Duration> {
        if retries >= MAX_OPENINGS - 1 {
            return None;
        }
        let remaining = self.remaining()?;
        let hash = request_id.bytes().fold(u64::from(retries), |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(u64::from(byte))
        });
        let delay = Duration::from_millis((500_u64 << retries) * (80 + hash % 41) / 100);
        (delay < remaining).then_some(delay)
    }
}
