//! Opt-in recovery of paused Excel accounts; no credential or response payloads.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "RecoveryInput")]
pub struct ExcelRecoveryConfig {
    pub enabled: bool,
    pub interval_minutes: u32,
}

impl Default for ExcelRecoveryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: 60,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryInput {
    enabled: bool,
    interval_minutes: u32,
}

impl TryFrom<RecoveryInput> for ExcelRecoveryConfig {
    type Error = &'static str;

    fn try_from(value: RecoveryInput) -> Result<Self, Self::Error> {
        let config = Self {
            enabled: value.enabled,
            interval_minutes: value.interval_minutes,
        };
        config.validate()?;
        Ok(config)
    }
}

impl ExcelRecoveryConfig {
    pub fn validate(self) -> Result<(), &'static str> {
        if !(1..=10080).contains(&self.interval_minutes) {
            return Err("Excel recovery interval must be an integer from 1 to 10080 minutes");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcelRecoveryView {
    #[serde(flatten)]
    pub config: ExcelRecoveryConfig,
    pub next_probe_at: DateTime<Utc>,
    pub last_probe_at: Option<DateTime<Utc>>,
    pub last_result: Option<String>,
    pub last_model: Option<String>,
    pub recovered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct ExcelRecoveryClaim {
    pub account_id: String,
    pub generation: i64,
    pub lease_id: String,
    pub credential_revision: u64,
    pub config_revision: u64,
    pub model: String,
    pub scope: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExcelRecoveryOutcome {
    Recovered,
    RequestFailed,
    ResponseMismatch,
    Cancelled,
}

impl ExcelRecoveryOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recovered => "recovered",
            Self::RequestFailed => "request_failed",
            Self::ResponseMismatch => "response_mismatch",
            Self::Cancelled => "cancelled",
        }
    }
}
