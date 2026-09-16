//! A request's account wait windows are shared across every routing attempt.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use crate::routing::RequestTuning;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountWaitMode {
    Sticky,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccountWaitBudgetError {
    #[error("account waiting is disabled")]
    Disabled,
    #[error("account wait budget expired")]
    Expired,
    #[error("account wait configuration is invalid")]
    InvalidConfiguration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountWaitDeadline {
    deadline: SystemTime,
    monotonic_deadline: Instant,
}

impl AccountWaitDeadline {
    #[must_use]
    pub const fn deadline(self) -> SystemTime {
        self.deadline
    }

    #[must_use]
    pub const fn monotonic_deadline(self) -> Instant {
        self.monotonic_deadline
    }

    #[must_use]
    pub fn remaining(self) -> Duration {
        self.monotonic_deadline
            .saturating_duration_since(Instant::now())
    }
}

#[derive(Debug, Default)]
struct WaitWindows {
    shared: Option<Instant>,
    sticky: Option<Instant>,
    fallback: Option<Instant>,
    expired: bool,
}

impl WaitWindows {
    fn expired_at(&self, now: Instant) -> bool {
        self.expired
            || [self.shared, self.sticky, self.fallback]
                .into_iter()
                .flatten()
                .any(|deadline| now >= deadline)
    }
}

#[derive(Debug)]
pub struct AccountWaitBudget {
    tuning: RequestTuning,
    wall_origin: SystemTime,
    monotonic_origin: Instant,
    request_deadline: Instant,
    windows: Mutex<WaitWindows>,
}

impl AccountWaitBudget {
    /// A preceding Key queue constrains all later account queues and retries.
    pub(super) fn constrain_deadline(&self, deadline: Instant) {
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        windows.shared = Some(
            windows
                .shared
                .map_or(deadline, |current| current.min(deadline))
                .min(self.request_deadline),
        );
    }

    #[must_use]
    pub fn new(request_deadline: SystemTime, tuning: RequestTuning) -> Self {
        Self::new_at(request_deadline, tuning, SystemTime::now(), Instant::now())
    }

    /// Supply one clock observation when coordinating an existing request.
    #[must_use]
    pub fn new_at(
        request_deadline: SystemTime,
        tuning: RequestTuning,
        wall_now: SystemTime,
        monotonic_now: Instant,
    ) -> Self {
        let remaining = request_deadline
            .duration_since(wall_now)
            .unwrap_or_default();
        Self {
            tuning,
            wall_origin: wall_now,
            monotonic_origin: monotonic_now,
            request_deadline: monotonic_now
                .checked_add(remaining)
                .unwrap_or(monotonic_now),
            windows: Mutex::new(WaitWindows::default()),
        }
    }

    pub fn enter(
        &self,
        mode: AccountWaitMode,
    ) -> Result<AccountWaitDeadline, AccountWaitBudgetError> {
        self.enter_at(mode, Instant::now())
    }

    /// The supplied observation uses this request's monotonic clock.
    pub fn enter_at(
        &self,
        mode: AccountWaitMode,
        now: Instant,
    ) -> Result<AccountWaitDeadline, AccountWaitBudgetError> {
        if !self.tuning.account_busy_wait_enabled {
            return Err(AccountWaitBudgetError::Disabled);
        }
        if self.tuning.account_busy_wait_sticky_max_waiting == 0
            || self.tuning.account_busy_wait_fallback_max_waiting == 0
            || self.tuning.account_busy_wait_sticky_timeout_seconds == 0
            || self.tuning.account_busy_wait_fallback_timeout_seconds == 0
        {
            return Err(AccountWaitBudgetError::InvalidConfiguration);
        }
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if now >= self.request_deadline || windows.expired_at(now) {
            windows.expired = true;
            return Err(AccountWaitBudgetError::Expired);
        }
        let shared_seconds = self
            .tuning
            .account_busy_wait_sticky_timeout_seconds
            .max(self.tuning.account_busy_wait_fallback_timeout_seconds);
        let shared = *windows.shared.get_or_insert_with(|| {
            now.checked_add(Duration::from_secs(shared_seconds))
                .unwrap_or(self.request_deadline)
                .min(self.request_deadline)
        });
        let (slot, seconds) = match mode {
            AccountWaitMode::Sticky => (
                &mut windows.sticky,
                self.tuning.account_busy_wait_sticky_timeout_seconds,
            ),
            AccountWaitMode::Fallback => (
                &mut windows.fallback,
                self.tuning.account_busy_wait_fallback_timeout_seconds,
            ),
        };
        slot.get_or_insert_with(|| {
            now.checked_add(Duration::from_secs(seconds))
                .unwrap_or(shared)
                .min(shared)
        });
        let monotonic_deadline = [windows.sticky, windows.fallback]
            .into_iter()
            .flatten()
            .fold(shared, Instant::min);
        let deadline = self
            .wall_origin
            .checked_add(monotonic_deadline.saturating_duration_since(self.monotonic_origin))
            .ok_or(AccountWaitBudgetError::InvalidConfiguration)?;
        Ok(AccountWaitDeadline {
            deadline,
            monotonic_deadline,
        })
    }

    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.is_exhausted_at(Instant::now())
    }

    #[must_use]
    pub fn is_exhausted_at(&self, now: Instant) -> bool {
        self.tuning.account_busy_wait_enabled
            && (now >= self.request_deadline
                || self
                    .windows
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .expired_at(now))
    }

    pub fn expire(&self) {
        self.windows
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .expired = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_queue_deadline_caps_later_sticky_and_fallback_waits() {
        let wall = SystemTime::now();
        let now = Instant::now();
        let budget = AccountWaitBudget::new_at(
            wall + Duration::from_secs(600),
            RequestTuning {
                account_busy_wait_enabled: true,
                ..Default::default()
            },
            wall,
            now,
        );
        let deadline = now + Duration::from_secs(30);
        budget.constrain_deadline(deadline);
        for mode in [AccountWaitMode::Sticky, AccountWaitMode::Fallback] {
            assert_eq!(
                budget
                    .enter_at(mode, now + Duration::from_secs(20))
                    .unwrap()
                    .monotonic_deadline(),
                deadline
            );
            assert_eq!(
                budget
                    .enter_at(mode, now + Duration::from_secs(25))
                    .unwrap()
                    .monotonic_deadline(),
                deadline
            );
        }
        assert!(budget.is_exhausted_at(deadline));
        assert_eq!(
            budget.enter_at(AccountWaitMode::Sticky, deadline),
            Err(AccountWaitBudgetError::Expired)
        );
    }
}
