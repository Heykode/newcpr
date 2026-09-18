//! `runtime_settings` 单例与 config revision 的 PostgreSQL owner。

use std::num::NonZeroU32;
use std::time::Duration;
use std::{collections::BTreeMap, fmt};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};

use gateway_core::account::RotationStrategy;
use gateway_core::policy::CodexClientVersion;
use gateway_core::provider_ports::{
    ProviderRefreshPolicy, ProviderRuntimePolicyPort, ProviderStoreError, ProviderStoreErrorKind,
};

use crate::{Revision, StoreError, StoreResult, postgres_unavailable};
use gateway_admin::model::settings::RequestTuningOverrides;

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub config_revision: Revision,
    pub admin_api_key: Option<String>,
    pub disable_fast: bool,
    pub turn_state_injection_enabled: bool,
    pub turn_state_models: Vec<String>,
    pub responses_max_decompressed_body_bytes: u64,
    pub refresh_margin_seconds: u64,
    pub refresh_concurrency: u32,
    pub max_concurrent_per_account: u32,
    pub request_interval_ms: u64,
    pub rotation_strategy: String,
    pub model_mappings: BTreeMap<String, String>,
    pub min_codex_desktop_version: Option<String>,
    pub min_codex_cli_version: Option<String>,
    pub usage_retention_days: u32,
    pub ops_event_retention_days: u32,
    pub audit_retention_days: u32,
    pub request_tuning: RequestTuningOverrides,
    pub updated_at: DateTime<Utc>,
}

impl fmt::Debug for RuntimeSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeSettings")
            .field("config_revision", &self.config_revision)
            .field(
                "admin_api_key",
                &self.admin_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("disable_fast", &self.disable_fast)
            .field(
                "turn_state_injection_enabled",
                &self.turn_state_injection_enabled,
            )
            .field("turn_state_models", &self.turn_state_models)
            .field(
                "responses_max_decompressed_body_bytes",
                &self.responses_max_decompressed_body_bytes,
            )
            .field("refresh_margin_seconds", &self.refresh_margin_seconds)
            .field("refresh_concurrency", &self.refresh_concurrency)
            .field(
                "max_concurrent_per_account",
                &self.max_concurrent_per_account,
            )
            .field("request_interval_ms", &self.request_interval_ms)
            .field("rotation_strategy", &self.rotation_strategy)
            .field("model_mappings", &self.model_mappings)
            .field("min_codex_desktop_version", &self.min_codex_desktop_version)
            .field("min_codex_cli_version", &self.min_codex_cli_version)
            .field("usage_retention_days", &self.usage_retention_days)
            .field("ops_event_retention_days", &self.ops_event_retention_days)
            .field("audit_retention_days", &self.audit_retention_days)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

#[derive(Clone)]
pub struct RuntimeSettingsUpdate {
    pub admin_api_key: Option<String>,
    pub disable_fast: Option<bool>,
    pub turn_state_injection_enabled: Option<bool>,
    pub turn_state_models: Vec<String>,
    pub responses_max_decompressed_body_bytes: u64,
    pub refresh_margin_seconds: u64,
    pub refresh_concurrency: u32,
    pub max_concurrent_per_account: u32,
    pub request_interval_ms: u64,
    pub rotation_strategy: String,
    pub model_mappings: BTreeMap<String, String>,
    pub min_codex_desktop_version: Option<String>,
    pub min_codex_cli_version: Option<String>,
    pub usage_retention_days: u32,
    pub ops_event_retention_days: u32,
    pub audit_retention_days: u32,
    pub request_tuning: RequestTuningOverrides,
}

impl fmt::Debug for RuntimeSettingsUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeSettingsUpdate")
            .field(
                "admin_api_key",
                &self.admin_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("disable_fast", &self.disable_fast)
            .field(
                "turn_state_injection_enabled",
                &self.turn_state_injection_enabled,
            )
            .field("turn_state_models", &self.turn_state_models)
            .field(
                "responses_max_decompressed_body_bytes",
                &self.responses_max_decompressed_body_bytes,
            )
            .field("rotation_strategy", &self.rotation_strategy)
            .field("model_mappings", &self.model_mappings)
            .finish_non_exhaustive()
    }
}

impl RuntimeSettingsUpdate {
    pub fn validate(&self) -> StoreResult<()> {
        if self.refresh_margin_seconds == 0
            || self.refresh_concurrency == 0
            || self.max_concurrent_per_account == 0
            || self.responses_max_decompressed_body_bytes == 0
            || self.responses_max_decompressed_body_bytes
                > gateway_admin::model::settings::MAX_RESPONSES_MAX_DECOMPRESSED_BODY_BYTES
            || self.usage_retention_days < 31
            || self.ops_event_retention_days == 0
            || self.audit_retention_days == 0
            || !valid_model_mappings(&self.model_mappings)
            || !valid_turn_state_models(&self.turn_state_models)
            || !valid_client_version(self.min_codex_desktop_version.as_deref())
            || !valid_client_version(self.min_codex_cli_version.as_deref())
            || RotationStrategy::parse(&self.rotation_strategy).is_none()
            || !self.request_tuning.validate()
        {
            return Err(StoreError::InvalidData {
                entity: "runtime settings",
                message: "settings violate the frozen runtime constraints".to_owned(),
            });
        }
        Ok(())
    }
}

#[async_trait]
pub trait RuntimeSettingsRepository: Send + Sync {
    async fn load_runtime_settings(&self) -> StoreResult<RuntimeSettings>;

    async fn update_runtime_settings(&self, update: RuntimeSettingsUpdate)
    -> StoreResult<Revision>;
}

#[derive(Clone)]
pub struct PgRuntimeSettingsRepository {
    pool: PgPool,
}

impl PgRuntimeSettingsRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RuntimeSettingsRepository for PgRuntimeSettingsRepository {
    async fn load_runtime_settings(&self) -> StoreResult<RuntimeSettings> {
        load_runtime_settings_from_pool(&self.pool).await
    }

    async fn update_runtime_settings(
        &self,
        update: RuntimeSettingsUpdate,
    ) -> StoreResult<Revision> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin runtime settings update"))?;
        let revision = update_runtime_settings_in_transaction(&mut transaction, &update).await?;
        transaction
            .commit()
            .await
            .map_err(|_| postgres_unavailable("commit runtime settings update"))?;
        Ok(revision)
    }
}

pub(crate) async fn load_runtime_settings_from_pool(pool: &PgPool) -> StoreResult<RuntimeSettings> {
    let row = sqlx::query_as::<_, RuntimeSettingsRow>(
            "select config_revision, admin_api_key, disable_fast, turn_state_injection_enabled, turn_state_models, responses_max_decompressed_body_bytes, refresh_margin_seconds,
                    refresh_concurrency, max_concurrent_per_account, request_interval_ms,
                    rotation_strategy, model_mappings_json, usage_retention_days, ops_event_retention_days,
                    audit_retention_days, min_codex_desktop_version,
                    min_codex_cli_version, request_tuning_json, updated_at
             from runtime_settings where id = 1",
        )
    .fetch_optional(pool)
    .await
    .map_err(|_| postgres_unavailable("load runtime settings"))?
    .ok_or_else(|| StoreError::NotFound {
        entity: "runtime settings",
        id: "1".to_owned(),
    })?;
    runtime_settings_from_row(row)
}

impl ProviderRuntimePolicyPort for PgRuntimeSettingsRepository {
    fn load_user_agent_override<'a>(
        &'a self,
        provider_kind: &'a gateway_core::routing::ProviderKind,
    ) -> futures::future::BoxFuture<
        'a,
        Result<gateway_core::provider_ports::ProviderUserAgentOverride, ProviderStoreError>,
    > {
        Box::pin(async move {
            super::outbound_user_agent::load_user_agent_override(&self.pool, provider_kind)
                .await
                .map_err(|_| provider_unavailable("load outbound user-agent"))
        })
    }

    fn load_refresh_policy(
        &self,
    ) -> futures::future::BoxFuture<'_, Result<ProviderRefreshPolicy, ProviderStoreError>> {
        Box::pin(async move {
            let settings = RuntimeSettingsRepository::load_runtime_settings(self)
                .await
                .map_err(|_| provider_unavailable("load refresh policy"))?;
            let concurrency = NonZeroU32::new(settings.refresh_concurrency)
                .ok_or_else(|| provider_invalid("decode refresh policy"))?;
            ProviderRefreshPolicy::try_new(
                Duration::from_secs(settings.refresh_margin_seconds),
                concurrency,
            )
        })
    }
}

pub(crate) async fn load_runtime_settings_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
) -> StoreResult<RuntimeSettings> {
    let row = sqlx::query_as::<_, RuntimeSettingsRow>(
        "select config_revision, admin_api_key, disable_fast, turn_state_injection_enabled, turn_state_models, responses_max_decompressed_body_bytes, refresh_margin_seconds,
                refresh_concurrency, max_concurrent_per_account, request_interval_ms,
                rotation_strategy, model_mappings_json, usage_retention_days, ops_event_retention_days,
                audit_retention_days, min_codex_desktop_version,
                min_codex_cli_version, request_tuning_json, updated_at
         from runtime_settings where id = 1",
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("load runtime settings in transaction"))?
    .ok_or_else(|| StoreError::NotFound {
        entity: "runtime settings",
        id: "1".to_owned(),
    })?;
    runtime_settings_from_row(row)
}

pub(crate) async fn update_runtime_settings_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    update: &RuntimeSettingsUpdate,
) -> StoreResult<Revision> {
    update.validate()?;
    let refresh_margin_seconds =
        i64::try_from(update.refresh_margin_seconds).map_err(|_| invalid_numeric())?;
    let next = sqlx::query_scalar::<_, i64>(
        "update runtime_settings
             set config_revision = config_revision + 1,
	                 admin_api_key = $1,
	                 refresh_margin_seconds = $2,
	                 refresh_concurrency = $3,
	                 max_concurrent_per_account = $4,
	                 request_interval_ms = $5,
	                 rotation_strategy = $6,
	                 model_mappings_json = $7,
	                 usage_retention_days = $8,
	                 ops_event_retention_days = $9,
	                 audit_retention_days = $10,
	                 min_codex_desktop_version = $11,
	                 min_codex_cli_version = $12,
	                 request_tuning_json = $13,
	                 responses_max_decompressed_body_bytes = $14,
	                 disable_fast = coalesce($15, disable_fast),
	                 turn_state_injection_enabled = coalesce($16, turn_state_injection_enabled),
	                 turn_state_models = $17,
	                 updated_at = now()
	             where id = 1
	             returning config_revision",
    )
    .bind(update.admin_api_key.as_deref())
    .bind(refresh_margin_seconds)
    .bind(i64::from(update.refresh_concurrency))
    .bind(i64::from(update.max_concurrent_per_account))
    .bind(i64::try_from(update.request_interval_ms).map_err(|_| invalid_numeric())?)
    .bind(&update.rotation_strategy)
    .bind(sqlx::types::Json(&update.model_mappings))
    .bind(i64::from(update.usage_retention_days))
    .bind(i64::from(update.ops_event_retention_days))
    .bind(i64::from(update.audit_retention_days))
    .bind(update.min_codex_desktop_version.as_deref())
    .bind(update.min_codex_cli_version.as_deref())
    .bind(sqlx::types::Json(&update.request_tuning))
    .bind(
        i64::try_from(update.responses_max_decompressed_body_bytes)
            .map_err(|_| invalid_numeric())?,
    )
    .bind(update.disable_fast)
    .bind(update.turn_state_injection_enabled)
    .bind(&update.turn_state_models)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("update runtime settings in transaction"))?
    .ok_or_else(|| StoreError::NotFound {
        entity: "runtime settings",
        id: "1".to_owned(),
    })?;
    Revision::new(u64::try_from(next).map_err(|_| invalid_numeric())?)
}

pub(crate) async fn bump_config_revision_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
) -> StoreResult<Revision> {
    let next = sqlx::query_scalar::<_, i64>(
        "update runtime_settings
         set config_revision = config_revision + 1, updated_at = now()
         where id = 1
         returning config_revision",
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("bump config revision in transaction"))?
    .ok_or_else(|| StoreError::NotFound {
        entity: "runtime settings",
        id: "1".to_owned(),
    })?;
    Revision::new(u64::try_from(next).map_err(|_| invalid_numeric())?)
}

/// 更新 admin_api_key 字段，config revision 由调用方 bump。
pub(crate) async fn update_admin_api_key_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    admin_api_key: Option<String>,
) -> StoreResult<()> {
    sqlx::query(
        "update runtime_settings
         set admin_api_key = $1,
             updated_at = now()
         where id = 1",
    )
    .bind(admin_api_key.as_deref())
    .execute(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("update admin api key in transaction"))?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct RuntimeSettingsRow {
    config_revision: i64,
    admin_api_key: Option<String>,
    disable_fast: bool,
    turn_state_injection_enabled: bool,
    turn_state_models: Vec<String>,
    responses_max_decompressed_body_bytes: i64,
    refresh_margin_seconds: i64,
    refresh_concurrency: i64,
    max_concurrent_per_account: i64,
    request_interval_ms: i64,
    rotation_strategy: String,
    model_mappings_json: sqlx::types::Json<BTreeMap<String, String>>,
    usage_retention_days: i64,
    ops_event_retention_days: i64,
    audit_retention_days: i64,
    min_codex_desktop_version: Option<String>,
    min_codex_cli_version: Option<String>,
    request_tuning_json: Option<sqlx::types::Json<RequestTuningOverrides>>,
    updated_at: DateTime<Utc>,
}

fn runtime_settings_from_row(row: RuntimeSettingsRow) -> StoreResult<RuntimeSettings> {
    Ok(RuntimeSettings {
        config_revision: Revision::new(to_u64(row.config_revision)?)?,
        admin_api_key: row.admin_api_key,
        disable_fast: row.disable_fast,
        turn_state_injection_enabled: row.turn_state_injection_enabled,
        turn_state_models: row.turn_state_models,
        responses_max_decompressed_body_bytes: to_u64(row.responses_max_decompressed_body_bytes)?,
        refresh_margin_seconds: to_u64(row.refresh_margin_seconds)?,
        refresh_concurrency: to_u32(row.refresh_concurrency)?,
        max_concurrent_per_account: to_u32(row.max_concurrent_per_account)?,
        request_interval_ms: to_u64(row.request_interval_ms)?,
        rotation_strategy: row.rotation_strategy,
        model_mappings: row.model_mappings_json.0,
        usage_retention_days: to_u32(row.usage_retention_days)?,
        ops_event_retention_days: to_u32(row.ops_event_retention_days)?,
        audit_retention_days: to_u32(row.audit_retention_days)?,
        min_codex_desktop_version: row.min_codex_desktop_version,
        min_codex_cli_version: row.min_codex_cli_version,
        request_tuning: row
            .request_tuning_json
            .map_or_else(RequestTuningOverrides::default, |value| value.0),
        updated_at: row.updated_at,
    })
}

fn to_u64(value: i64) -> StoreResult<u64> {
    u64::try_from(value).map_err(|_| invalid_numeric())
}

fn to_u32(value: i64) -> StoreResult<u32> {
    u32::try_from(value).map_err(|_| invalid_numeric())
}

fn invalid_numeric() -> StoreError {
    StoreError::InvalidData {
        entity: "runtime settings",
        message: "numeric field is outside its supported range".to_owned(),
    }
}

fn provider_unavailable(operation: &'static str) -> ProviderStoreError {
    ProviderStoreError::new(ProviderStoreErrorKind::Unavailable, operation)
}

fn provider_invalid(operation: &'static str) -> ProviderStoreError {
    ProviderStoreError::new(ProviderStoreErrorKind::InvalidData, operation)
}

fn valid_model_mappings(mappings: &BTreeMap<String, String>) -> bool {
    mappings.len() <= 512
        && mappings.iter().all(|(requested, upstream)| {
            valid_model_name(requested, 256) && valid_model_name(upstream, 256)
        })
}

fn valid_turn_state_models(models: &[String]) -> bool {
    !models.is_empty()
        && models.len() <= 64
        && models.iter().all(|model| {
            model == model.trim()
                && model == &model.to_ascii_lowercase()
                && valid_model_name(model, 256)
        })
        && models
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == models.len()
}

fn valid_model_name(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn valid_client_version(value: Option<&str>) -> bool {
    value.is_none_or(|value| CodexClientVersion::parse(value).is_ok())
}
