use std::num::NonZeroU32;
use std::sync::Arc;

use async_trait::async_trait;
use gateway_core::engine::AttemptContext;
use gateway_core::engine::provider::{
    Provider, ProviderCatalogGeneration, ProviderModelCapabilities, ProviderRegistry,
    ProviderRequest, ProviderStream,
};
use gateway_core::error::{ProviderError, ProviderErrorKind};
use gateway_core::policy::{ClientApiKeyId, PlaintextClientApiKey, RateLimits};
use gateway_core::routing::snapshot::RuntimeSnapshotCompiler;
use gateway_core::upstream::UpstreamSendState;
use gateway_store::postgres::{
    ClientApiKeySnapshot, PgRuntimeSnapshotRepository, RuntimeSnapshotRepository,
};

use super::TestDatabase;

#[test]
fn snapshot_client_policy_contains_only_common_limits() {
    let policy = ClientApiKeySnapshot {
        id: ClientApiKeyId::new("key-1").expect("client key ID"),
        plaintext_key: PlaintextClientApiKey::new("sk_snapshot_secret").expect("plaintext key"),
        group_ids: Vec::new(),
        limits: RateLimits {
            max_concurrency: 3,
            requests_per_minute: 60,
        },
    };
    assert_eq!(policy.limits.max_concurrency, 3);
    assert!(policy.group_ids.is_empty());
    assert!(!format!("{policy:?}").contains("sk_snapshot_secret"));
}

#[tokio::test]
async fn runtime_snapshot_loads_enabled_plaintext_key_without_debug_exposure() {
    let Some(database) = TestDatabase::create("client_snapshot").await else {
        return;
    };
    let plaintext = format!("sk_{}", "s".repeat(43));
    sqlx::query(
        "insert into client_api_keys (
           id, name, key, enabled, max_concurrency, requests_per_minute, created_at, updated_at
         ) values ('key_snapshot', 'snapshot', $1, true, 2, 60, now(), now())",
    )
    .bind(&plaintext)
    .execute(&database.pool)
    .await
    .expect("seed client API key");
    let snapshot = PgRuntimeSnapshotRepository::new(database.pool.clone())
        .load_runtime_snapshot()
        .await
        .expect("load runtime snapshot");
    assert_eq!(snapshot.client_api_keys.len(), 1);
    assert!(snapshot.client_api_keys[0].group_ids.is_empty());
    assert_eq!(
        snapshot.client_api_keys[0].plaintext_key.expose_for_auth(),
        plaintext
    );
    assert!(!format!("{snapshot:?}").contains(&plaintext));
    database.close().await;
}

#[tokio::test]
async fn runtime_snapshot_projects_live_account_concurrency_including_disabled_accounts() {
    let Some(database) = TestDatabase::create("snapshot_capacity").await else {
        return;
    };
    sqlx::query(
        "update runtime_settings set config_revision = 2, max_concurrent_per_account = 5 where id = 1",
    )
    .execute(&database.pool)
    .await
    .expect("set snapshot default");
    for (id, enabled, limit) in [
        ("acct_inherited", true, None),
        ("acct_disabled", false, Some(3_i64)),
        ("acct_maximum", true, Some(i64::from(u32::MAX))),
        ("acct_deleted", true, Some(9)),
    ] {
        sqlx::query(
            "insert into provider_accounts (
               id, provider_kind, name, authentication_kind, provider_credentials_json,
               has_refresh_token, enabled, concurrency_limit, credential_observed_at,
               created_at, updated_at
             ) values ($1, 'openai', $1, 'api_key', '{}', false, $2, $3, now(), now(), now())",
        )
        .bind(id)
        .bind(enabled)
        .bind(limit)
        .execute(&database.pool)
        .await
        .expect("seed snapshot account");
    }
    sqlx::query("delete from provider_accounts where id = 'acct_deleted'")
        .execute(&database.pool)
        .await
        .expect("delete account before snapshot");
    let repository = Arc::new(PgRuntimeSnapshotRepository::new(database.pool.clone()));
    let raw = repository
        .load_runtime_snapshot()
        .await
        .expect("load SQL projection");
    assert_eq!(raw.config_revision, raw.observed_current_revision);
    assert_eq!(
        raw.provider_accounts
            .iter()
            .map(|account| (account.id.as_str(), account.concurrency_limit))
            .collect::<Vec<_>>(),
        vec![
            ("acct_disabled", Some(3)),
            ("acct_inherited", None),
            ("acct_maximum", Some(u32::MAX))
        ],
    );

    let providers = ProviderRegistry::new([Arc::new(SnapshotTestProvider) as Arc<dyn Provider>])
        .expect("provider registry");
    let compiler = RuntimeSnapshotCompiler::new(repository, Arc::new(providers));
    let first = compiler
        .compile()
        .await
        .expect("compile real store facts")
        .account_concurrency();
    assert_eq!(first.revision().get(), 2);
    assert_eq!(first.limit_for("acct_inherited"), NonZeroU32::new(5));
    assert_eq!(first.limit_for("acct_disabled"), NonZeroU32::new(3));
    assert_eq!(first.limit_for("acct_maximum"), NonZeroU32::new(u32::MAX));
    assert_eq!(first.limit_for("acct_deleted"), None);

    let mut transaction = database.pool.begin().await.expect("begin capacity update");
    sqlx::query(
        "update runtime_settings set config_revision = 3, max_concurrent_per_account = 1 where id = 1",
    )
    .execute(&mut *transaction)
    .await
    .expect("lower default capacity");
    sqlx::query("update provider_accounts set concurrency_limit = null where id = 'acct_disabled'")
        .execute(&mut *transaction)
        .await
        .expect("clear disabled account override");
    sqlx::query("delete from provider_accounts where id = 'acct_maximum'")
        .execute(&mut *transaction)
        .await
        .expect("delete previously published account");
    transaction.commit().await.expect("commit capacity update");

    let updated = compiler
        .compile()
        .await
        .expect("compile updated store facts")
        .account_concurrency();
    assert_eq!(updated.revision().get(), 3);
    assert_eq!(updated.limit_for("acct_inherited"), NonZeroU32::new(1));
    assert_eq!(updated.limit_for("acct_disabled"), NonZeroU32::new(1));
    assert_eq!(updated.limit_for("acct_maximum"), None);
    assert_eq!(first.limit_for("acct_inherited"), NonZeroU32::new(5));
    database.close().await;
}

struct SnapshotTestProvider;

#[tokio::test]
async fn runtime_snapshot_compiles_account_busy_wait_defaults_overrides_and_frozen_values() {
    use gateway_admin::model::settings::RequestTuningOverrides;
    use gateway_core::routing::RequestTuning;
    use serde_json::json;

    let Some(database) = TestDatabase::create("snapshot_wait").await else {
        return;
    };
    let repository = Arc::new(PgRuntimeSnapshotRepository::new(database.pool.clone()));
    let providers = ProviderRegistry::new([Arc::new(SnapshotTestProvider) as Arc<dyn Provider>])
        .expect("provider registry");
    let compiler = RuntimeSnapshotCompiler::new(repository, Arc::new(providers));
    let defaults = compiler.compile().await.expect("compile old settings");
    assert_eq!(defaults.request_tuning(), RequestTuning::default());
    let expected = RequestTuning {
        account_busy_wait_enabled: true,
        account_busy_wait_sticky_max_waiting: 7,
        account_busy_wait_sticky_timeout_seconds: 123,
        account_busy_wait_fallback_max_waiting: 111,
        account_busy_wait_fallback_timeout_seconds: 33,
        max_request_attempts: 8,
        websocket_http_fallback_enabled: false,
        ..RequestTuning::default()
    };
    let overrides = RequestTuningOverrides {
        account_busy_wait_enabled: Some(expected.account_busy_wait_enabled),
        account_busy_wait_sticky_max_waiting: Some(expected.account_busy_wait_sticky_max_waiting),
        account_busy_wait_sticky_timeout_seconds: Some(
            expected.account_busy_wait_sticky_timeout_seconds,
        ),
        account_busy_wait_fallback_max_waiting: Some(
            expected.account_busy_wait_fallback_max_waiting,
        ),
        account_busy_wait_fallback_timeout_seconds: Some(
            expected.account_busy_wait_fallback_timeout_seconds,
        ),
        max_request_attempts: Some(expected.max_request_attempts),
        websocket_http_fallback_enabled: Some(expected.websocket_http_fallback_enabled),
        ..Default::default()
    };
    sqlx::query(
        "update runtime_settings set request_tuning_json = $1, config_revision = config_revision + 1 where id = 1",
    )
    .bind(sqlx::types::Json(overrides))
    .execute(&database.pool)
    .await
    .expect("persist wait overrides");
    let enabled = compiler
        .compile()
        .await
        .expect("compile enabled wait settings");
    assert_eq!(enabled.request_tuning(), expected);

    sqlx::query(
        "update runtime_settings set request_tuning_json = $1, config_revision = config_revision + 1 where id = 1",
    )
    .bind(sqlx::types::Json(RequestTuningOverrides {
        account_busy_wait_enabled: Some(false),
        ..overrides
    }))
    .execute(&database.pool)
    .await
    .expect("disable waiting without clearing values");
    let disabled = compiler
        .compile()
        .await
        .expect("compile disabled wait settings");
    assert_eq!(
        disabled.request_tuning(),
        RequestTuning {
            account_busy_wait_enabled: false,
            ..expected
        }
    );
    assert_eq!(enabled.request_tuning(), expected);
    assert_eq!(defaults.request_tuning(), RequestTuning::default());

    for inherited in [
        json!({}),
        json!({
            "accountBusyWaitEnabled": null,
            "accountBusyWaitStickyMaxWaiting": null,
            "accountBusyWaitStickyTimeoutSeconds": null,
            "accountBusyWaitFallbackMaxWaiting": null,
            "accountBusyWaitFallbackTimeoutSeconds": null
        }),
    ] {
        sqlx::query(
            "update runtime_settings set request_tuning_json = $1, config_revision = config_revision + 1 where id = 1",
        )
        .bind(sqlx::types::Json(inherited))
        .execute(&database.pool)
        .await
        .expect("restore inherited settings");
        assert_eq!(
            compiler
                .compile()
                .await
                .expect("compile inherited wait settings")
                .request_tuning(),
            RequestTuning::default()
        );
    }
    database.close().await;
}

#[async_trait]
impl Provider for SnapshotTestProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::new(0)
    }

    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        Ok(Vec::new())
    }

    async fn execute(
        &self,
        _: ProviderRequest,
        _: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unavailable,
            UpstreamSendState::NotSent,
        ))
    }
}
