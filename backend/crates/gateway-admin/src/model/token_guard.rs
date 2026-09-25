//! Credential guard configuration and safe observations; never credential material.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::AdminError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenGuardConfig {
    pub enabled: bool,
    pub group_ids: Vec<String>,
    pub model: String,
    pub interval_seconds: u32,
    pub timeout_seconds: u32,
    pub concurrency: u16,
    pub max_per_cycle: u16,
}

impl Default for TokenGuardConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            group_ids: Vec::new(),
            model: "gpt-6-astra".into(),
            interval_seconds: 300,
            timeout_seconds: 60,
            concurrency: 6,
            max_per_cycle: 12,
        }
    }
}

impl TokenGuardConfig {
    pub fn validate(&self) -> Result<(), AdminError> {
        if !(30..=86_400).contains(&self.interval_seconds)
            || !(5..=900).contains(&self.timeout_seconds)
            || !(1..=16).contains(&self.concurrency)
            || !(1..=100).contains(&self.max_per_cycle)
            || self.group_ids.len() > 100
            || self
                .group_ids
                .iter()
                .any(|id| gateway_core::routing::AccountGroupId::new(id).is_err())
            || self
                .group_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.group_ids.len()
            || gateway_core::routing::UpstreamModelId::new(&self.model).is_err()
        {
            return Err(AdminError::invalid("凭证守护参数超出允许范围"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenGuardOutcome {
    Healthy,
    AuthRequired,
    Transient,
    Skipped,
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenGuardEvent {
    pub account_id: String,
    pub observed_revision: u64,
    pub outcome: TokenGuardOutcome,
    pub reason: TokenGuardReason,
    pub latency_ms: u64,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenGuardReason {
    Completed,
    CredentialExpired,
    CredentialInvalid,
    AccountBanned,
    AccountDisabled,
    QuotaExhausted,
    AccountChanged,
    RequestFailed,
    Timeout,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenGuardStatus {
    pub config: TokenGuardConfig,
    pub running: bool,
    pub queued: bool,
    pub events: Vec<TokenGuardEvent>,
}
