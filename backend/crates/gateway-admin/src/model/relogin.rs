//! Durable relogin facts. Secret-bearing storage records never implement Debug.

use super::{AdminError, accounts::AccountRecord};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_BATCH: usize = 500;
pub const MAX_IMPORT_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReloginStatus {
    Pending,
    Queued,
    Running,
    Ready,
    Pushing,
    Uncertain,
    Failed,
}

impl ReloginStatus {
    pub const fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Pushing)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ReloginCredential {
    pub document: Map<String, Value>,
    pub email: String,
    pub user_id: String,
    pub workspace_id: String,
    pub plan_type: String,
    pub expires_at: DateTime<Utc>,
    pub verified_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReloginTarget {
    pub account_id: String,
    pub credential_revision: u64,
    pub user_id: String,
    pub workspace_id: String,
}

impl ReloginTarget {
    pub fn from_account(account: &AccountRecord) -> Result<Self, AdminError> {
        Ok(Self {
            account_id: account.id.clone(),
            credential_revision: account.credential_revision.get(),
            user_id: account
                .upstream_user_id
                .clone()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| AdminError::conflict("原账号缺少上游身份，不能自动替换"))?,
            workspace_id: account
                .upstream_account_id
                .clone()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| AdminError::conflict("原账号缺少工作区，不能自动替换"))?,
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ReloginEntry {
    pub id: String,
    pub revision: u64,
    pub email: String,
    pub password: String,
    pub mfa_secret: String,
    pub automatic: bool,
    pub preferred_workspace_id: Option<String>,
    pub status: ReloginStatus,
    pub message: String,
    pub credential: Option<ReloginCredential>,
    pub target: Option<ReloginTarget>,
    pub automatic_job: bool,
    /// Explicit account-menu confirmation; absent on legacy and library-only jobs.
    #[serde(default)]
    pub manual_push_context: Option<super::MutationContext>,
    pub automatic_attempts: u32,
    pub attempted_target: Option<ReloginTarget>,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub synced_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub imported_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

impl ReloginEntry {
    pub fn import_time(&self) -> Option<DateTime<Utc>> {
        self.imported_at.or_else(|| {
            // Legacy rows have immutable UUIDv7 creation times, not an import timestamp.
            let id = uuid::Uuid::parse_str(self.id.strip_prefix("relogin_")?).ok()?;
            if id.get_version_num() != 7 {
                return None;
            }
            let (seconds, nanos) = id.get_timestamp()?.to_unix();
            DateTime::from_timestamp(i64::try_from(seconds).ok()?, nanos)
        })
    }

    pub fn validate_totp(&self) -> Result<(), AdminError> {
        if self.password.is_empty() || totp_secret(&self.mfa_secret).is_none() {
            return Err(AdminError::invalid("请重新导入邮箱、密码和有效的 2FA 密钥"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReloginSettings {
    pub concurrency: usize,
    pub paused: bool,
}

impl Default for ReloginSettings {
    fn default() -> Self {
        Self {
            concurrency: 1,
            paused: false,
        }
    }
}

impl ReloginSettings {
    pub fn validate(&self) -> Result<(), AdminError> {
        if !(1..=8).contains(&self.concurrency) {
            return Err(AdminError::invalid("重登并发必须为 1 至 8"));
        }
        Ok(())
    }
}

pub struct ReloginInput {
    pub email: String,
    pub password: String,
    pub mfa_secret: String,
}

fn totp_secret(raw: &str) -> Option<String> {
    let secret: String = raw
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace() && *ch != '-')
        .map(|ch| ch.to_ascii_uppercase())
        .collect();
    let secret = secret.trim_end_matches('=').to_owned();
    ((16..=128).contains(&secret.len())
        && matches!(secret.len() % 8, 0 | 2 | 4 | 5 | 7)
        && secret
            .bytes()
            .all(|ch| ch.is_ascii_uppercase() || (b'2'..=b'7').contains(&ch)))
    .then_some(secret)
}

/// Preserve password punctuation and interior separators.
pub fn parse_relogin_import(text: &str) -> Result<Vec<ReloginInput>, AdminError> {
    if text.len() > MAX_IMPORT_BYTES {
        return Err(AdminError::invalid("导入内容超过 512 KiB"));
    }
    let mut seen = BTreeSet::new();
    let mut inputs = Vec::new();
    for (index, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let invalid = || {
            AdminError::invalid(format!(
                "第 {} 行格式不完整、资料有歧义或验证资料不合法",
                index + 1
            ))
        };
        let (email, tail) = line.split_once("----").ok_or_else(invalid)?;
        let (password, last) = tail.rsplit_once("----").ok_or_else(invalid)?;
        // Reject legacy mailbox material even if its token resembles a TOTP secret.
        if password
            .rsplit_once("----")
            .is_some_and(|(_, client)| client.len() == 36 && uuid::Uuid::parse_str(client).is_ok())
        {
            return Err(invalid());
        }
        let mfa_secret = totp_secret(last).ok_or_else(invalid)?;
        let email = email.trim().to_ascii_lowercase();
        if email.len() > 254
            || email.matches('@').count() != 1
            || email.starts_with('@')
            || email.ends_with('@')
            || email.chars().any(char::is_whitespace)
            || email.chars().any(char::is_control)
            || password.is_empty()
            || password.len() > 1024
            || password.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        if !seen.insert(email.clone()) {
            return Err(AdminError::invalid(format!(
                "第 {} 行邮箱重复，请先合并资料",
                index + 1
            )));
        }
        inputs.push(ReloginInput {
            email,
            password: password.to_owned(),
            mfa_secret,
        });
    }
    if inputs.is_empty() || inputs.len() > MAX_BATCH {
        return Err(AdminError::invalid("每次导入必须为 1 至 500 个账号"));
    }
    Ok(inputs)
}

pub fn validate_ids(ids: &[String]) -> Result<(), AdminError> {
    if ids.is_empty()
        || ids.len() > MAX_BATCH
        || ids.iter().any(|id| id.is_empty() || id.len() > 80)
        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(AdminError::invalid("请选择 1 至 500 个不同的账号"));
    }
    Ok(())
}

pub struct ReloginRequest {
    pub email: String,
    pub password: String,
    pub mfa_secret: String,
    pub workspace_id: Option<String>,
    pub outbound_proxy: Option<gateway_core::account::OutboundProxy>,
}
