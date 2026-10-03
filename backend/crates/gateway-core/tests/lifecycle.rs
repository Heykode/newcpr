use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use gateway_core::lifecycle::CancellationToken;
use gateway_core::lifecycle::{Deadline, REQUEST_LEASE_TTL};
use std::time::{Duration, SystemTime};

#[test]
fn ordinary_deadline_is_absent_but_lease_is_bounded() {
    let deadline = Deadline::default();
    assert_eq!(deadline.at(), None);
    assert_eq!(deadline.remaining(), None);
    assert!(!deadline.is_elapsed());
    assert_eq!(deadline.bounded(REQUEST_LEASE_TTL), REQUEST_LEASE_TTL);
    let before = SystemTime::now();
    let lease = deadline.lease_deadline();
    let after = SystemTime::now();
    assert!(lease >= before + REQUEST_LEASE_TTL);
    assert!(lease <= after + REQUEST_LEASE_TTL);
    assert_eq!(Deadline::from_timeout(before, None), Some(deadline));
}

#[test]
fn explicit_deadline_still_caps_waits_and_resource_leases() {
    let now = SystemTime::now();
    let at = now + Duration::from_secs(7);
    let deadline = Deadline::from_timeout(now, Some(Duration::from_secs(7))).unwrap();
    assert_eq!(deadline.at(), Some(at));
    assert_eq!(deadline.lease_deadline(), at);
    assert!(deadline.bounded(REQUEST_LEASE_TTL) <= Duration::from_secs(7));
    assert_eq!(Deadline::default().min(at), deadline);
    let expired = Deadline::from(now - Duration::from_secs(1));
    assert!(expired.is_elapsed());
    assert_eq!(expired.bounded(REQUEST_LEASE_TTL), Duration::ZERO);
}

#[test]
fn cancellation_token_should_wake_current_state() {
    let token = CancellationToken::new();
    token.cancel();

    assert!(token.is_cancelled());
}
use gateway_core::lifecycle::{ConnectionDraining, ConnectionGuard, ConnectionLifecycle};

#[derive(Default)]
struct LifecycleState {
    draining: AtomicBool,
    active: AtomicUsize,
}

struct TestGuard {
    state: Arc<LifecycleState>,
}

impl ConnectionGuard for TestGuard {}

impl Drop for TestGuard {
    fn drop(&mut self) {
        self.state.active.fetch_sub(1, Ordering::AcqRel);
    }
}

struct TestLifecycle {
    state: Arc<LifecycleState>,
    cancellation: CancellationToken,
}

impl TestLifecycle {
    fn new() -> Self {
        Self {
            state: Arc::new(LifecycleState::default()),
            cancellation: CancellationToken::new(),
        }
    }

    fn begin_draining(&self) {
        self.state.draining.store(true, Ordering::Release);
        self.cancellation.cancel();
    }

    fn active(&self) -> usize {
        self.state.active.load(Ordering::Acquire)
    }
}

impl ConnectionLifecycle for TestLifecycle {
    fn try_register(&self) -> Result<Box<dyn ConnectionGuard>, ConnectionDraining> {
        if self.state.draining.load(Ordering::Acquire) {
            return Err(ConnectionDraining);
        }
        self.state.active.fetch_add(1, Ordering::AcqRel);
        if self.state.draining.load(Ordering::Acquire) {
            self.state.active.fetch_sub(1, Ordering::AcqRel);
            return Err(ConnectionDraining);
        }
        Ok(Box::new(TestGuard {
            state: Arc::clone(&self.state),
        }))
    }

    fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    fn is_draining(&self) -> bool {
        self.state.draining.load(Ordering::Acquire)
    }
}

#[test]
fn connection_guard_drop_releases_active_registration() {
    let lifecycle = TestLifecycle::new();
    let guard = lifecycle.try_register().expect("registration before drain");
    drop(guard);

    assert_eq!(lifecycle.active(), 0);
}

#[test]
fn connection_registration_rejects_after_drain_linearization() {
    let lifecycle = TestLifecycle::new();
    lifecycle.begin_draining();

    assert!(matches!(lifecycle.try_register(), Err(ConnectionDraining)));
}

#[test]
fn connection_lifecycle_contract_is_object_safe() {
    let lifecycle = TestLifecycle::new();
    let object: &dyn ConnectionLifecycle = &lifecycle;

    assert!(!object.is_draining());
}
