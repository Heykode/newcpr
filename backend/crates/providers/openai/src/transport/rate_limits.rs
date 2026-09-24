//! Quota samples retain their capture time across buffering and persistence.

use std::time::SystemTime;

use gateway_protocol::openai::events::{ParsedRateLimits, parse_rate_limit_headers};

#[derive(Debug, Clone, PartialEq)]
pub struct CodexRateLimitObservation {
    pub rate_limits: ParsedRateLimits,
    pub observed_at: SystemTime,
}

impl CodexRateLimitObservation {
    pub fn from_headers(headers: &[(String, String)], observed_at: SystemTime) -> Option<Self> {
        Some(Self {
            rate_limits: parse_rate_limit_headers(headers)?,
            observed_at,
        })
    }
}
