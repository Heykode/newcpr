use std::time::Duration;

use provider_openai::{quota_failure_refresh_delay, uniform_delay};

#[test]
fn quota_failure_refresh_delay_covers_declared_settlement_window() {
    assert_eq!(quota_failure_refresh_delay(None), Duration::from_secs(2));
    let samples = [1, 1_000, u64::MAX / 3, u64::MAX / 2, u64::MAX - 1, u64::MAX];
    let delays: Vec<_> = samples
        .into_iter()
        .map(|sample| quota_failure_refresh_delay(Some(sample)))
        .collect();
    assert!(
        delays
            .iter()
            .all(|delay| { *delay >= Duration::from_secs(1) && *delay < Duration::from_secs(3) })
    );
    assert_eq!(
        quota_failure_refresh_delay(Some(1_500_000_000)),
        Duration::from_millis(2_500)
    );
    assert!(delays.iter().any(|delay| *delay < Duration::from_secs(2)));
    assert!(delays.iter().any(|delay| *delay >= Duration::from_secs(2)));
}

#[test]
fn uniform_delay_is_half_open_and_has_a_fixed_fallback() {
    let max = Duration::from_secs(600);
    assert_eq!(uniform_delay(None, max), Duration::ZERO);
    assert_eq!(uniform_delay(Some(0), max), Duration::ZERO);
    assert_eq!(
        uniform_delay(Some(u64::MAX), Duration::ZERO),
        Duration::ZERO
    );
    assert!(uniform_delay(Some(u64::MAX), max) < max);
}
