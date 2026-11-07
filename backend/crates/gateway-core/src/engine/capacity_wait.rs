//! A request's account wait windows are shared across every routing attempt.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use crate::lifecycle::Deadline;
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
    request_deadline: Option<Instant>,
    windows: Mutex<WaitWindows>,
    concurrency: crate::concurrency::ConcurrencyWaitBudget,
}

impl AccountWaitBudget {
    #[must_use]
    pub fn concurrency_wait_budget(&self) -> crate::concurrency::ConcurrencyWaitBudget {
        self.concurrency.clone()
    }

    #[must_use]
    pub fn new(request_deadline: impl Into<Deadline>, tuning: RequestTuning) -> Self {
        Self::new_at(request_deadline, tuning, SystemTime::now(), Instant::now())
    }

    /// Supply one clock observation when coordinating an existing request.
    #[must_use]
    pub fn new_at(
        request_deadline: impl Into<Deadline>,
        tuning: RequestTuning,
        wall_now: SystemTime,
        monotonic_now: Instant,
    ) -> Self {
        let request_deadline = request_deadline.into().at().map(|at| {
            monotonic_now
                .checked_add(at.duration_since(wall_now).unwrap_or_default())
                .unwrap_or(monotonic_now)
        });
        Self {
            tuning,
            wall_origin: wall_now,
            monotonic_origin: monotonic_now,
            request_deadline,
            windows: Mutex::new(WaitWindows::default()),
            concurrency: crate::concurrency::ConcurrencyWaitBudget::default(),
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
        self.enter_window(mode, now)
    }

    /// Strict descendants must follow their session owner even when optional
    /// capacity waiting is disabled. Reuse the same bounded sticky window.
    pub fn enter_session_affinity(&self) -> Result<AccountWaitDeadline, AccountWaitBudgetError> {
        self.enter_window(AccountWaitMode::Sticky, Instant::now())
    }

    fn enter_window(
        &self,
        mode: AccountWaitMode,
        now: Instant,
    ) -> Result<AccountWaitDeadline, AccountWaitBudgetError> {
        if self.tuning.max_waiting_per_account == 0
            || self.tuning.account_concurrency_wait_timeout_seconds == 0
        {
            return Err(AccountWaitBudgetError::InvalidConfiguration);
        }
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self
            .request_deadline
            .is_some_and(|deadline| now >= deadline)
            || windows.expired_at(now)
        {
            windows.expired = true;
            return Err(AccountWaitBudgetError::Expired);
        }
        let shared_seconds = self.tuning.account_concurrency_wait_timeout_seconds;
        let shared_candidate = self
            .concurrency
            .start_at(now, Duration::from_secs(shared_seconds));
        let shared = *windows.shared.get_or_insert_with(|| {
            self.request_deadline
                .map_or(shared_candidate, |end| shared_candidate.min(end))
        });
        let slot = match mode {
            AccountWaitMode::Sticky => &mut windows.sticky,
            AccountWaitMode::Fallback => &mut windows.fallback,
        };
        slot.get_or_insert_with(|| {
            now.checked_add(Duration::from_secs(shared_seconds))
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
        let windows = self
            .windows
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // A preceding Key queue alone must not enable disabled account waiting.
        // With optional waiting disabled, only strict affinity can enter sticky.
        (self.tuning.account_busy_wait_enabled || windows.sticky.is_some())
            && (self
                .request_deadline
                .is_some_and(|deadline| now >= deadline)
                || windows.expired_at(now)
                || self
                    .concurrency
                    .existing_deadline()
                    .is_some_and(|deadline| now >= deadline))
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
    fn required_session_wait_is_bounded_when_optional_capacity_wait_is_disabled() {
        let budget = AccountWaitBudget::new(Deadline::default(), RequestTuning::default());
        assert_eq!(
            budget.enter(AccountWaitMode::Sticky),
            Err(AccountWaitBudgetError::Disabled)
        );
        let first = budget.enter_session_affinity().unwrap();
        assert_eq!(budget.enter_session_affinity().unwrap(), first);
        assert!(budget.is_exhausted_at(first.monotonic_deadline()));
        budget.expire();
        assert!(budget.is_exhausted());
        assert_eq!(
            budget.enter_session_affinity(),
            Err(AccountWaitBudgetError::Expired)
        );
    }

    #[test]
    fn account_wait_starts_after_key_admission_and_is_shared_across_retries() {
        let now = Instant::now();
        let budget = AccountWaitBudget::new_at(
            Deadline::default(),
            RequestTuning {
                account_busy_wait_enabled: true,
                account_concurrency_wait_timeout_seconds: 30,
                ..Default::default()
            },
            SystemTime::now(),
            now,
        );
        let account_start = now + Duration::from_secs(20);
        let deadline = account_start + Duration::from_secs(30);
        assert_eq!(
            budget
                .enter_at(AccountWaitMode::Sticky, account_start)
                .unwrap()
                .monotonic_deadline(),
            deadline
        );
        assert_eq!(
            budget
                .enter_at(
                    AccountWaitMode::Fallback,
                    account_start + Duration::from_secs(5)
                )
                .unwrap()
                .monotonic_deadline(),
            deadline
        );
        assert_eq!(budget.concurrency.existing_deadline(), Some(deadline));
        assert!(budget.is_exhausted_at(deadline));
    }
}
