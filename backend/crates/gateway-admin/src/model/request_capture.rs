//! Administrator-only, opt-in global or scoped error capture.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::AdminError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestCaptureConfig {
    pub enabled: bool,
    #[serde(default)]
    pub global_errors: bool,
    #[serde(default)]
    pub include_media: bool,
    pub quota_mib: u32,
    #[serde(default)]
    pub quota_policy: CaptureQuotaPolicy,
    pub retention_days: u16,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureQuotaPolicy {
    #[default]
    Stop,
    Overwrite,
}

impl Default for RequestCaptureConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            global_errors: false,
            include_media: false,
            quota_mib: 1024,
            quota_policy: CaptureQuotaPolicy::Stop,
            retention_days: 7,
        }
    }
}

impl RequestCaptureConfig {
    pub fn validate(&self) -> Result<(), AdminError> {
        if !(1..=102_400).contains(&self.quota_mib) || self.retention_days > 30 {
            return Err(AdminError::invalid("采集配额或保留天数超出范围"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureScope {
    Global,
    Key,
    Account,
    Group,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateCaptureTask {
    pub scope: CaptureScope,
    pub target_id: String,
    pub minutes: u16,
    #[serde(default)]
    pub include_media: bool,
}

impl CreateCaptureTask {
    pub fn validate(&self) -> Result<(), AdminError> {
        if !(1..=1440).contains(&self.minutes)
            || self.scope == CaptureScope::Global
            || self.target_id.is_empty()
            || self.target_id.len() > 128
            || self.target_id.chars().any(char::is_control)
        {
            return Err(AdminError::invalid("采集范围或时长不合法"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureTask {
    pub id: String,
    pub scope: CaptureScope,
    pub target_id: String,
    pub include_media: bool,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: CaptureTaskStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureTaskStatus {
    Running,
    Stopped,
    Expired,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRecord {
    pub id: String,
    pub task_id: String,
    pub request_id: String,
    pub bytes: u64,
    pub incomplete: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestCaptureStatus {
    pub config: RequestCaptureConfig,
    pub instance_id: String,
    pub tasks: Vec<CaptureTask>,
    pub records: Vec<CaptureRecord>,
    pub skipped: u64,
    pub active_sessions: usize,
    pub buffered_bytes: usize,
    pub storage_fault: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestCaptureSettings {
    pub config: RequestCaptureConfig,
    pub global_active: bool,
    pub storage_fault: bool,
    pub skipped: u64,
    pub stored_bytes: u64,
    pub record_count: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClearCaptures {
    pub confirmed: bool,
    pub cutoff_at: Option<DateTime<Utc>>,
}

impl ClearCaptures {
    pub fn validate(&self) -> Result<(), AdminError> {
        if !self.confirmed || self.cutoff_at.is_some_and(|cutoff| cutoff > Utc::now()) {
            return Err(AdminError::invalid(
                "请确认仅清理已有采集材料，截止时间不能在未来",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureClearResult {
    pub cutoff_at: DateTime<Utc>,
    pub removed_records: u64,
    pub removed_bytes: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturePage {
    pub text: String,
    pub next_offset: Option<u64>,
}
