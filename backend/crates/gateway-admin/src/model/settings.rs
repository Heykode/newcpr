//! Runtime settings 与明文管理员 API Key 的语义模型。

use std::{collections::BTreeMap, fmt};

use chrono::{DateTime, Utc};
use serde::Deserialize;

use gateway_core::routing::{PublicModelId, UpstreamModelId};

use super::Revision;

/// 客户端模型到上游模型的全局精确映射。
pub type ModelMappings = BTreeMap<PublicModelId, UpstreamModelId>;

/// 管理员明确保存的请求调优覆盖值；`None` 表示继承启动配置或代码默认值。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestTuningOverrides {
    pub max_account_switches: Option<u32>,
    pub max_request_attempts: Option<u32>,
    pub websocket_max_retries: Option<u32>,
    pub websocket_http_fallback_enabled: Option<bool>,
    pub websocket_large_request_threshold_bytes: Option<u64>,
    pub websocket_max_age_ms: Option<u64>,
    pub websocket_stream_idle_timeout_ms: Option<u64>,
    pub websocket_failure_threshold: Option<u32>,
    pub websocket_failure_window_ms: Option<u64>,
    pub websocket_failure_open_duration_ms: Option<u64>,
    pub rate_limit_cooldown_seconds: Option<u64>,
    pub openai_location_override_enabled: Option<bool>,
    pub max_waiting_per_key: Option<u32>,
    pub key_concurrency_wait_timeout_seconds: Option<u64>,
    pub account_busy_wait_enabled: Option<bool>,
    pub account_busy_wait_sticky_max_waiting: Option<u32>,
    pub account_busy_wait_sticky_timeout_seconds: Option<u64>,
    pub account_busy_wait_fallback_max_waiting: Option<u32>,
    pub account_busy_wait_fallback_timeout_seconds: Option<u64>,
}

impl<'de> Deserialize<'de> for RequestTuningOverrides {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            max_account_switches: Option<u32>,
            max_request_attempts: Option<u32>,
            websocket_max_retries: Option<u32>,
            websocket_http_fallback_enabled: Option<bool>,
            websocket_large_request_threshold_bytes: Option<u64>,
            websocket_max_age_ms: Option<u64>,
            // Old API clients and persisted settings may carry this removed limit.
            #[serde(rename = "websocketMaxConnecting")]
            _legacy_websocket_max_connecting: Option<serde::de::IgnoredAny>,
            websocket_stream_idle_timeout_ms: Option<u64>,
            websocket_failure_threshold: Option<u32>,
            websocket_failure_window_ms: Option<u64>,
            websocket_failure_open_duration_ms: Option<u64>,
            rate_limit_cooldown_seconds: Option<u64>,
            openai_location_override_enabled: Option<bool>,
            max_waiting_per_key: Option<u32>,
            key_concurrency_wait_timeout_seconds: Option<u64>,
            account_busy_wait_enabled: Option<bool>,
            account_busy_wait_sticky_max_waiting: Option<u32>,
            account_busy_wait_sticky_timeout_seconds: Option<u64>,
            account_busy_wait_fallback_max_waiting: Option<u32>,
            account_busy_wait_fallback_timeout_seconds: Option<u64>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            max_account_switches: wire.max_account_switches,
            max_request_attempts: wire.max_request_attempts,
            websocket_max_retries: wire.websocket_max_retries,
            websocket_http_fallback_enabled: wire.websocket_http_fallback_enabled,
            websocket_large_request_threshold_bytes: wire.websocket_large_request_threshold_bytes,
            websocket_max_age_ms: wire.websocket_max_age_ms,
            websocket_stream_idle_timeout_ms: wire.websocket_stream_idle_timeout_ms,
            websocket_failure_threshold: wire.websocket_failure_threshold,
            websocket_failure_window_ms: wire.websocket_failure_window_ms,
            websocket_failure_open_duration_ms: wire.websocket_failure_open_duration_ms,
            rate_limit_cooldown_seconds: wire.rate_limit_cooldown_seconds,
            openai_location_override_enabled: wire.openai_location_override_enabled,
            max_waiting_per_key: wire.max_waiting_per_key,
            key_concurrency_wait_timeout_seconds: wire.key_concurrency_wait_timeout_seconds,
            account_busy_wait_enabled: wire.account_busy_wait_enabled,
            account_busy_wait_sticky_max_waiting: wire.account_busy_wait_sticky_max_waiting,
            account_busy_wait_sticky_timeout_seconds: wire.account_busy_wait_sticky_timeout_seconds,
            account_busy_wait_fallback_max_waiting: wire.account_busy_wait_fallback_max_waiting,
            account_busy_wait_fallback_timeout_seconds: wire
                .account_busy_wait_fallback_timeout_seconds,
        })
    }
}

impl RequestTuningOverrides {
    pub const MAX_ACCOUNT_SWITCHES: u32 = 31;
    pub const MAX_REQUEST_ATTEMPTS: u32 = 32;
    pub const MAX_WEBSOCKET_RETRIES: u32 = 100;
    pub const MAX_WEBSOCKET_LARGE_REQUEST_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;
    pub const MAX_WEBSOCKET_MAX_AGE_MS: u64 = 86_400_000;
    pub const MAX_WEBSOCKET_IDLE_TIMEOUT_MS: u64 = 86_400_000;
    pub const MAX_WEBSOCKET_FAILURE_THRESHOLD: u32 = 100;
    pub const MAX_WEBSOCKET_FAILURE_WINDOW_MS: u64 = 86_400_000;
    pub const MAX_WEBSOCKET_FAILURE_OPEN_DURATION_MS: u64 = 86_400_000;
    pub const MAX_RATE_LIMIT_COOLDOWN_SECONDS: u64 = 86_400;
    pub const MAX_ACCOUNT_BUSY_WAIT_MAX_WAITING: u32 = 1_000;
    pub const MAX_ACCOUNT_BUSY_WAIT_TIMEOUT_SECONDS: u64 = 600;

    pub fn validate(&self) -> bool {
        self.max_waiting_per_key.is_none_or(|value| value <= 1024)
            && self
                .key_concurrency_wait_timeout_seconds
                .is_none_or(|value| (1..=600).contains(&value))
            && self
                .max_account_switches
                .is_none_or(|value| value <= Self::MAX_ACCOUNT_SWITCHES)
            && self
                .max_request_attempts
                .is_none_or(|value| (1..=Self::MAX_REQUEST_ATTEMPTS).contains(&value))
            && self
                .websocket_max_retries
                .is_none_or(|value| value <= Self::MAX_WEBSOCKET_RETRIES)
            && self
                .websocket_large_request_threshold_bytes
                .is_none_or(|value| value <= Self::MAX_WEBSOCKET_LARGE_REQUEST_THRESHOLD_BYTES)
            && self
                .websocket_max_age_ms
                .is_none_or(|value| (1..=Self::MAX_WEBSOCKET_MAX_AGE_MS).contains(&value))
            && self
                .websocket_stream_idle_timeout_ms
                .is_none_or(|value| (1..=Self::MAX_WEBSOCKET_IDLE_TIMEOUT_MS).contains(&value))
            && self
                .websocket_failure_threshold
                .is_none_or(|value| (1..=Self::MAX_WEBSOCKET_FAILURE_THRESHOLD).contains(&value))
            && self
                .websocket_failure_window_ms
                .is_none_or(|value| (1..=Self::MAX_WEBSOCKET_FAILURE_WINDOW_MS).contains(&value))
            && self.websocket_failure_open_duration_ms.is_none_or(|value| {
                (1..=Self::MAX_WEBSOCKET_FAILURE_OPEN_DURATION_MS).contains(&value)
            })
            && self
                .rate_limit_cooldown_seconds
                .is_none_or(|value| value <= Self::MAX_RATE_LIMIT_COOLDOWN_SECONDS)
            && [
                self.account_busy_wait_sticky_max_waiting,
                self.account_busy_wait_fallback_max_waiting,
            ]
            .into_iter()
            .all(|value| {
                value.is_none_or(|value| {
                    (1..=Self::MAX_ACCOUNT_BUSY_WAIT_MAX_WAITING).contains(&value)
                })
            })
            && [
                self.account_busy_wait_sticky_timeout_seconds,
                self.account_busy_wait_fallback_timeout_seconds,
            ]
            .into_iter()
            .all(|value| {
                value.is_none_or(|value| {
                    (1..=Self::MAX_ACCOUNT_BUSY_WAIT_TIMEOUT_SECONDS).contains(&value)
                })
            })
    }
}

/// 账号调度策略；由 Core 拥有稳定值与 wire 映射。
pub use gateway_core::account::RotationStrategy;

/// 完整运行设置事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub config_revision: Revision,
    pub model_mappings: ModelMappings,
    pub refresh_margin_seconds: u64,
    pub refresh_concurrency: u32,
    pub max_concurrent_per_account: u32,
    pub request_interval_ms: u64,
    pub rotation_strategy: RotationStrategy,
    pub min_codex_desktop_version: Option<String>,
    pub min_codex_cli_version: Option<String>,
    pub usage_retention_days: u32,
    pub ops_event_retention_days: u32,
    pub audit_retention_days: u32,
    pub request_tuning: RequestTuningOverrides,
    pub updated_at: DateTime<Utc>,
}

/// 原子替换运行设置的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceRuntimeSettings {
    pub model_mappings: ModelMappings,
    pub refresh_margin_seconds: u64,
    pub refresh_concurrency: u32,
    pub max_concurrent_per_account: u32,
    pub request_interval_ms: u64,
    pub rotation_strategy: RotationStrategy,
    pub min_codex_desktop_version: Option<String>,
    pub min_codex_cli_version: Option<String>,
    pub usage_retention_days: u32,
    pub ops_event_retention_days: u32,
    pub audit_retention_days: u32,
    pub request_tuning: RequestTuningOverrides,
}

/// 明文管理员 API Key；按产品约束明文落库，但禁止 Debug 泄漏。
#[derive(Clone, PartialEq, Eq)]
pub struct AdminApiKey(String);

impl AdminApiKey {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn expose_for_auth(&self) -> &str {
        &self.0
    }

    /// 仅供显式 regenerate 响应读取一次。
    #[must_use]
    pub fn expose_for_response(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AdminApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdminApiKey([REDACTED])")
    }
}

/// 管理员 API Key 更新结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminApiKeyMutation {
    pub config_revision: Revision,
    pub exists: bool,
}

/// 新管理员 API Key 的一次性返回结果。
pub struct RegeneratedAdminApiKey {
    pub mutation: AdminApiKeyMutation,
    pub key: AdminApiKey,
}

impl fmt::Debug for RegeneratedAdminApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegeneratedAdminApiKey")
            .field("mutation", &self.mutation)
            .field("key", &"[REDACTED]")
            .finish()
    }
}
