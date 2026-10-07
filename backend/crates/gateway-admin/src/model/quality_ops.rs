//! Persisted quality rules and bounded, administrator-only result data.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const QUALITY_MAX_WORKERS: i64 = 10;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityModelChoice {
    pub id: String,
    pub name: String,
    /// None means no capability evidence for this exact account.
    pub reasoning_efforts: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualityModelQuery {
    #[serde(default)]
    pub account_ids: Vec<String>,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub statuses: Vec<String>,
    pub page: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityModelPage {
    pub models: Vec<QualityModelChoice>,
    pub next_page: Option<u32>,
    pub matched_accounts: usize,
    pub known_accounts: usize,
    pub failed_accounts: usize,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualityGroupFilter {
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub statuses: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityGroupRule {
    pub id: String,
    pub revision: i64,
    pub name: String,
    pub filter: QualityGroupFilter,
    pub config: QualityRuleConfig,
    pub rule_count: i64,
    pub excluded_count: i64,
    pub last_synced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityGroupSync {
    pub created: usize,
    pub updated: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityGroupUpdate {
    pub group: QualityGroupRule,
    pub sync: QualityGroupSync,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QualityDetectionMode {
    #[default]
    Answer,
    StateProbe,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QualityFailureAction {
    #[default]
    None,
    DisableScheduling,
    RemoveGroups,
    EnableExcel,
    ApplyAccountTemplate,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualityRuleConfig {
    #[serde(default)]
    pub detection_mode: QualityDetectionMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_id: String,
    pub model: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<u32>,
    pub cron: String,
    pub timezone: String,
    pub repetitions: u8,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub reference_answer: String,
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub judge_group_id: String,
    #[serde(default)]
    pub judge_model: String,
    #[serde(default)]
    pub judge_prompt: String,
    #[serde(default)]
    pub failure_action: QualityFailureAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_template: Option<super::relogin_templates::ReloginTemplate>,
    #[serde(default)]
    pub failure_group_ids: Vec<String>,
    #[serde(default)]
    pub auto_restore: bool,
    #[serde(default)]
    pub disable_excel_on_native_recovery: bool,
    #[serde(default = "default_excel_failure_threshold")]
    pub excel_failure_threshold: u8,
    /// Omitted legacy configs retain single-success restoration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excel_recovery_threshold: Option<u8>,
}

const fn default_excel_failure_threshold() -> u8 {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityRule {
    pub id: String,
    pub revision: i64,
    pub config: QualityRuleConfig,
    pub next_run_at: DateTime<Utc>,
    pub running: bool,
    pub pending: bool,
    pub last_status: Option<String>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_action: Option<String>,
    #[serde(default)]
    pub excel_failure_streak: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_template: Option<QualityRuleTemplateRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualityRuleTemplateRef {
    pub id: String,
    pub revision: i64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityRuleTemplate {
    pub id: String,
    pub revision: i64,
    pub name: String,
    pub config: QualityRuleConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualityTemplateTarget {
    pub account_id: String,
    pub rule_id: Option<String>,
    pub revision: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityTemplateApplyResult {
    pub account_id: String,
    pub rule_id: Option<String>,
    pub success: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QualityMonitoring {
    pub rule_id: String,
    pub revision: i64,
    pub enabled: bool,
    pub running: bool,
    pub pending: bool,
    pub next_run_at: DateTime<Utc>,
    pub last_status: Option<String>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_action: Option<String>,
    pub source_template: Option<QualityRuleTemplateRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityVerdict {
    Correct,
    Incorrect,
    Unknown,
    RequestError,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<gateway_core::operation::quality_probe::StateProbeReport>,
    pub index: u8,
    pub answer: String,
    pub verdict: QualityVerdict,
    pub reason: String,
    pub elapsed_ms: u64,
    pub returned_model: Option<String>,
    pub judge_account_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityRun {
    pub detection_mode: QualityDetectionMode,
    pub id: String,
    pub rule_id: String,
    pub account_id: String,
    pub model: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub correct: u32,
    pub incorrect: u32,
    pub unknown: u32,
    pub request_errors: u32,
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<QualityRuleConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<QualityAnswer>,
}

#[derive(Debug, Clone)]
pub struct QualityClaim {
    pub action_scope: serde_json::Value,
    pub rule: QualityRule,
    pub run_id: String,
    pub lease_token: String,
    pub account_identity: (Option<String>, Option<String>),
}
