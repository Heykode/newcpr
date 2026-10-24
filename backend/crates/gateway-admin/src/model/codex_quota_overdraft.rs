use chrono::{DateTime, Utc};

use gateway_core::{
    account::{CredentialRevision, ProviderAccountId},
    routing::{ConfigRevision, ProviderKind, UpstreamModelId},
};

use super::accounts::AccountUsage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexQuotaOverdraftClaim {
    pub account_id: ProviderAccountId,
    pub provider_kind: ProviderKind,
    pub credential_revision: CredentialRevision,
    pub config_revision: ConfigRevision,
    pub account_subject: String,
    pub cycle_key: String,
    pub generation: u64,
    pub owner_token: String,
    pub probe_model: UpstreamModelId,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexQuotaOverdraftOutcome {
    Passed {
        tested_at: DateTime<Utc>,
    },
    Inconclusive {
        reason_code: String,
        tested_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
    },
    Failed {
        reason_code: String,
        tested_at: DateTime<Utc>,
        recover_at: Option<DateTime<Utc>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexQuotaOverdraftState {
    pub account_id: String,
    pub status: String,
    pub quota_window: String,
    pub probe_model: String,
    pub attempts: u32,
    pub attempt_limit: u32,
    pub reason_code: Option<String>,
    pub tested_at: Option<DateTime<Utc>>,
    pub retry_at: Option<DateTime<Utc>>,
    pub recover_at: Option<DateTime<Utc>>,
    pub windows: Vec<CodexQuotaOverdraftWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexQuotaOverdraftWindow {
    pub window_key: String,
    pub active: bool,
    pub started_at: Option<DateTime<Utc>>,
    pub recover_at: Option<DateTime<Utc>>,
    pub stats: Option<AccountUsage>,
}
