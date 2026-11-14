//! Independent credential ownership for the Excel entry on a shared account.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::{
    AccountErrorReason, CredentialRevision, CredentialState, PlaintextCredential, ProviderAccount,
    ProviderAccountId, ProviderAccountIdentity, QuotaState,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialTarget {
    #[default]
    Codex,
    Excel,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSelection {
    #[default]
    Codex,
    Excel,
    All,
}

impl CredentialSelection {
    #[must_use]
    pub fn targets(self) -> &'static [CredentialTarget] {
        match self {
            Self::Codex => &[CredentialTarget::Codex],
            Self::Excel => &[CredentialTarget::Excel],
            Self::All => &[CredentialTarget::Codex, CredentialTarget::Excel],
        }
    }
}

/// Safe metadata; token material is available only through the explicit load port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcelCredentialMetadata {
    pub revision: CredentialRevision,
    pub state: CredentialState,
    pub upstream_user_id: String,
    pub upstream_account_id: Option<String>,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub access_token_expires_at: Option<SystemTime>,
    pub has_refresh_token: bool,
    pub next_refresh_at: Option<SystemTime>,
    pub observed_at: SystemTime,
    pub last_error_reason: Option<AccountErrorReason>,
    pub last_error_message: Option<String>,
    pub suspended: bool,
    pub refresh_attempted: bool,
    pub quota: QuotaState,
}

#[derive(Debug, Clone)]
pub struct LoadedExcelCredential {
    pub account: ProviderAccount,
    pub metadata: ExcelCredentialMetadata,
    pub credential: PlaintextCredential,
}

/// A refresh can change Excel material but cannot rebind the shared account identity.
#[derive(Debug, Clone)]
pub struct ExcelCredentialCasUpdate {
    pub account_id: ProviderAccountId,
    pub expected_revision: CredentialRevision,
    pub identity: ProviderAccountIdentity,
    pub credential: PlaintextCredential,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub has_refresh_token: bool,
    pub access_token_expires_at: Option<SystemTime>,
    pub next_refresh_at: Option<SystemTime>,
    pub observed_at: SystemTime,
}
