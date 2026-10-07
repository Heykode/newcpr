//! Usage observation timing helpers.

use std::time::Duration;

pub(crate) fn random_u64() -> Option<u64> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes)
        .ok()
        .map(|()| u64::from_le_bytes(bytes))
}

#[doc(hidden)]
pub fn uniform_delay(sample: Option<u64>, max: Duration) -> Duration {
    let Some(sample) = sample else {
        return Duration::ZERO;
    };
    if max.is_zero() {
        return Duration::ZERO;
    }
    let nanos = (u128::from(sample) % max.as_nanos()).min(u64::MAX as u128);
    Duration::from_nanos(nanos as u64)
}

const QUOTA_FAILURE_REFRESH_DELAY: Duration = Duration::from_secs(2);
const QUOTA_FAILURE_REFRESH_MIN_DELAY: Duration = Duration::from_secs(1);
const QUOTA_FAILURE_REFRESH_MAX_DELAY: Duration = Duration::from_secs(3);

/// Delay quota usage rechecks over the declared half-open interval `[1s, 3s)`.
///
/// The fixed two-second delay remains the fallback when the system random source
/// is unavailable.
#[doc(hidden)]
pub fn quota_failure_refresh_delay(sample: Option<u64>) -> Duration {
    match sample {
        Some(sample) => {
            QUOTA_FAILURE_REFRESH_MIN_DELAY
                + uniform_delay(
                    Some(sample),
                    QUOTA_FAILURE_REFRESH_MAX_DELAY - QUOTA_FAILURE_REFRESH_MIN_DELAY,
                )
        }
        None => QUOTA_FAILURE_REFRESH_DELAY,
    }
}
