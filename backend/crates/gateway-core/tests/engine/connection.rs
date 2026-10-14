use std::cell::Cell;
use std::time::{Duration, Instant};

use gateway_core::engine::connection::ConnectionBudget;

thread_local! { static NOW: Cell<Instant> = Cell::new(Instant::now()); }
fn now() -> Instant {
    NOW.with(Cell::get)
}

#[test]
fn openings_share_one_budget_and_elapsed_window() {
    let budget = ConnectionBudget::with_clock(now);
    let clone = budget.clone();
    assert_eq!(budget.begin().unwrap(), Some(Duration::from_secs(30)));
    NOW.with(|now| now.set(now.get() + Duration::from_secs(28)));
    assert_eq!(clone.begin().unwrap(), Some(Duration::from_secs(2)));
    NOW.with(|now| now.set(now.get() + Duration::from_secs(2)));
    assert!(budget.begin().is_err());
    assert!(clone.retry_delay(0, "req_budget").is_none());
}

#[test]
fn four_openings_are_bounded_across_clones() {
    let budget = ConnectionBudget::with_clock(now);
    for _ in 0..4 {
        assert!(budget.clone().begin().is_ok());
    }
    assert!(budget.begin().is_err());
    assert!(budget.retry_delay(0, "req_budget").is_none());
}

#[test]
fn sending_disables_recovery_without_limiting_generation() {
    let budget = ConnectionBudget::with_clock(now);
    budget.begin().unwrap();
    let shared = budget.clone();
    shared.complete();
    drop(shared);
    NOW.with(|now| now.set(now.get() + Duration::from_secs(3600)));
    assert_eq!(budget.begin().unwrap(), None);
    assert!(budget.retry_delay(0, "req_budget").is_none());
}

#[test]
fn backoff_is_bounded_and_has_request_scoped_jitter() {
    let budget = ConnectionBudget::with_clock(now);
    budget.begin().unwrap();
    for retry in 0..3 {
        let delay = budget.retry_delay(retry, "req_budget").unwrap();
        assert!(delay >= Duration::from_millis(400 << retry));
        assert!(delay <= Duration::from_millis(600 << retry));
    }
    assert!(budget.retry_delay(3, "req_budget").is_none());
}
