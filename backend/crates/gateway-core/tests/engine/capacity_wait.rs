use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gateway_core::engine::{AccountWaitBudget, AccountWaitBudgetError, AccountWaitMode};
use gateway_core::lifecycle::Deadline;
use gateway_core::routing::RequestTuning;

#[test]
fn absent_request_deadline_does_not_extend_shared_account_wait_windows() {
    let wall = SystemTime::now();
    let now = Instant::now();
    let budget = AccountWaitBudget::new_at(Deadline::default(), enabled(), wall, now);
    let first = now + Duration::from_secs(900);
    assert!(!budget.is_exhausted_at(first));
    let sticky = budget.enter_at(AccountWaitMode::Sticky, first).unwrap();
    assert_eq!(sticky.monotonic_deadline(), first + Duration::from_secs(30));
    assert_eq!(sticky.deadline(), wall + Duration::from_secs(930));
    let fallback = budget
        .enter_at(AccountWaitMode::Fallback, first + Duration::from_secs(20))
        .unwrap();
    assert_eq!(fallback, sticky);
    assert_eq!(
        budget.enter_at(AccountWaitMode::Sticky, first + Duration::from_secs(30)),
        Err(AccountWaitBudgetError::Expired)
    );
}

fn enabled() -> RequestTuning {
    RequestTuning {
        account_busy_wait_enabled: true,
        ..RequestTuning::defaults()
    }
}

#[test]
fn account_wait_defaults_and_legacy_json_remain_opt_in() {
    let defaults = RequestTuning::defaults();
    assert!(!defaults.account_busy_wait_enabled);
    assert_eq!(defaults.max_waiting_per_account, 10);
    assert_eq!(defaults.account_concurrency_wait_timeout_seconds, 30);
    let mut legacy = serde_json::to_value(defaults).unwrap();
    legacy.as_object_mut().unwrap().retain(|key, _| {
        !matches!(
            key.as_str(),
            "accountBusyWaitEnabled"
                | "maxWaitingPerAccount"
                | "accountConcurrencyWaitTimeoutSeconds"
        )
    });
    assert_eq!(
        serde_json::from_value::<RequestTuning>(legacy).unwrap(),
        defaults
    );
    let budget = AccountWaitBudget::new(SystemTime::now() + Duration::from_secs(600), defaults);
    assert_eq!(
        budget.enter(AccountWaitMode::Sticky),
        Err(AccountWaitBudgetError::Disabled)
    );
    budget.expire();
    assert!(
        !budget.is_exhausted(),
        "disabled behavior never gains a wait gate"
    );
}

#[test]
fn account_wait_shared_attempts_never_reset_windows() {
    let wall = SystemTime::now();
    let now = Instant::now();
    let budget = Arc::new(AccountWaitBudget::new_at(
        wall + Duration::from_secs(600),
        enabled(),
        wall,
        now,
    ));
    let initial = budget.enter_at(AccountWaitMode::Sticky, now).unwrap();
    let retry = Arc::clone(&budget);
    assert_eq!(
        retry
            .enter_at(AccountWaitMode::Sticky, now + Duration::from_secs(10))
            .unwrap(),
        initial
    );
    let fallback = retry
        .enter_at(AccountWaitMode::Fallback, now + Duration::from_secs(20))
        .unwrap();
    assert_eq!(fallback.monotonic_deadline(), now + Duration::from_secs(30));
    assert_eq!(fallback.deadline(), wall + Duration::from_secs(30));
    assert_eq!(
        retry.enter_at(AccountWaitMode::Fallback, now + Duration::from_secs(30)),
        Err(AccountWaitBudgetError::Expired)
    );
    assert!(budget.is_exhausted_at(now + Duration::from_secs(30)));
}

#[test]
fn account_wait_fallback_cannot_upgrade_to_a_fresh_sticky_window() {
    let wall = SystemTime::now();
    let now = Instant::now();
    let budget = AccountWaitBudget::new_at(wall + Duration::from_secs(600), enabled(), wall, now);
    let fallback = budget.enter_at(AccountWaitMode::Fallback, now).unwrap();
    let sticky = budget
        .enter_at(AccountWaitMode::Sticky, now + Duration::from_secs(20))
        .unwrap();
    assert_eq!(fallback, sticky);
    assert_eq!(
        budget.enter_at(AccountWaitMode::Sticky, now + Duration::from_secs(30)),
        Err(AccountWaitBudgetError::Expired)
    );
}

#[test]
fn shared_account_wait_custom_timeout_applies_to_both_modes_without_renewal() {
    let wall = SystemTime::now();
    let now = Instant::now();
    for (first, second) in [
        (AccountWaitMode::Sticky, AccountWaitMode::Fallback),
        (AccountWaitMode::Fallback, AccountWaitMode::Sticky),
    ] {
        let tuning = RequestTuning {
            max_waiting_per_account: 5,
            account_concurrency_wait_timeout_seconds: 60,
            ..enabled()
        };
        let budget = AccountWaitBudget::new_at(Deadline::default(), tuning, wall, now);
        let initial = budget.enter_at(first, now).unwrap();
        assert_eq!(initial.monotonic_deadline(), now + Duration::from_secs(60));
        assert_eq!(
            budget
                .enter_at(second, now + Duration::from_secs(40))
                .unwrap(),
            initial
        );
        assert_eq!(
            budget.enter_at(second, now + Duration::from_secs(60)),
            Err(AccountWaitBudgetError::Expired)
        );
    }
}

#[test]
fn account_wait_uses_remaining_request_deadline_and_explicit_expiry() {
    let wall = SystemTime::now();
    let now = Instant::now();
    let budget = AccountWaitBudget::new_at(wall + Duration::from_secs(7), enabled(), wall, now);
    let deadline = budget
        .enter_at(AccountWaitMode::Sticky, now + Duration::from_secs(2))
        .unwrap();
    assert_eq!(deadline.deadline(), wall + Duration::from_secs(7));
    assert_eq!(deadline.monotonic_deadline(), now + Duration::from_secs(7));
    budget.expire();
    assert_eq!(
        budget.enter_at(AccountWaitMode::Fallback, now + Duration::from_secs(3)),
        Err(AccountWaitBudgetError::Expired)
    );
}

#[test]
fn account_wait_invalid_configuration_and_past_deadline_fail_closed() {
    let wall = SystemTime::now();
    let now = Instant::now();
    let invalid = RequestTuning {
        account_concurrency_wait_timeout_seconds: 0,
        ..enabled()
    };
    let budget = AccountWaitBudget::new_at(wall + Duration::from_secs(60), invalid, wall, now);
    assert_eq!(
        budget.enter_at(AccountWaitMode::Sticky, now),
        Err(AccountWaitBudgetError::InvalidConfiguration)
    );
    let past = AccountWaitBudget::new_at(wall - Duration::from_secs(1), enabled(), wall, now);
    assert_eq!(
        past.enter_at(AccountWaitMode::Sticky, now),
        Err(AccountWaitBudgetError::Expired)
    );
}
