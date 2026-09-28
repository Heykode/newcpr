//! Managed proxy projections contain no subscription URLs or node credentials.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionDownloadMode {
    #[default]
    Auto,
    Proxy,
    Direct,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CountryFilterMode {
    #[default]
    Off,
    Include,
    Exclude,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CountryFilter {
    pub mode: CountryFilterMode,
    pub codes: Vec<String>,
    pub allow_unknown: bool,
    pub dynamic_provider_managed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MihomoAction {
    Install,
    Start,
    Stop,
    SubscriptionAdd,
    SubscriptionUpdate,
    SubscriptionRename,
    SubscriptionRefresh,
    SubscriptionRemove,
    SubscriptionEnable,
    SubscriptionDisable,
    DynamicAppend,
    DynamicReplace,
    DynamicRemove,
    DynamicClear,
    Disable,
    Recover,
    Probe,
    CountryFilter,
    CountryScan,
    CountryProbe,
    DownloadMode,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MihomoCommand {
    pub action: MihomoAction,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub subscriptions: Vec<String>,
    #[serde(default)]
    pub dynamic_proxies: Vec<String>,
    pub country_filter: Option<CountryFilter>,
    pub download_mode: Option<SubscriptionDownloadMode>,
}

impl std::fmt::Debug for MihomoCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MihomoCommand")
            .field("action", &self.action)
            .field("subscriptions", &self.subscriptions.len())
            .field("dynamic_proxies", &self.dynamic_proxies.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MihomoSubscription {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub nodes: usize,
    pub cached: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MihomoNodeCheck {
    pub checked_at: Option<DateTime<Utc>>,
    pub latency_ms: Option<u64>,
    pub success: Option<bool>,
    pub message: Option<String>,
    pub exit_ip: Option<String>,
    pub country_code: Option<String>,
    pub country_name: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub quality: Option<MihomoQualityReport>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MihomoQualityReport {
    pub checked_at: DateTime<Utc>,
    pub score: u32,
    pub grade: String,
    pub summary: String,
    pub passed_count: usize,
    pub warn_count: usize,
    pub failed_count: usize,
    pub challenge_count: usize,
    pub checks: Vec<MihomoQualityItem>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MihomoQualityItem {
    pub name: String,
    pub success: bool,
    pub status: String,
    pub http_status: Option<u16>,
    pub cf_ray: Option<String>,
    pub latency_ms: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MihomoNode {
    pub name: String,
    pub display_name: String,
    pub subscription_ids: Vec<String>,
    pub dynamic: bool,
    pub state: String,
    pub country_code: Option<String>,
    pub country_checked_at: Option<DateTime<Utc>>,
    pub country_error: Option<String>,
    pub country_blocked: bool,
    pub check: Option<MihomoNodeCheck>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MihomoWarmStatus {
    pub target: usize,
    pub ready: usize,
    pub ready_subscription: usize,
    pub ready_dynamic: usize,
    pub eligible: usize,
    pub checking: usize,
    pub cooling: usize,
    pub failure_reasons: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MihomoStatus {
    pub version: String,
    pub installed: bool,
    pub running: bool,
    pub supported: bool,
    pub busy: bool,
    pub phase: String,
    pub error: Option<String>,
    pub endpoint: String,
    pub subscription_download_mode: SubscriptionDownloadMode,
    pub subscription_items: Vec<MihomoSubscription>,
    pub dynamic_proxies: usize,
    pub node_states: Vec<MihomoNode>,
    pub country_filter: CountryFilter,
    pub country_codes: Vec<String>,
    pub bps_warm_pool: MihomoWarmStatus,
    pub bps_ip_warm_pool: MihomoWarmStatus,
    pub codex_warm_pool: MihomoWarmStatus,
    pub codex_ip_warm_pool: MihomoWarmStatus,
}
