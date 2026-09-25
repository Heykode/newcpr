use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, Utc};
use gateway_admin::model::settings::RequestTuningOverrides;
use gateway_store::postgres::{
    PgRuntimeSettingsRepository, PgRuntimeSnapshotRepository, RuntimeSettingsRepository,
    RuntimeSettingsUpdate, RuntimeSnapshotRepository,
};
use serde_json::{Value, json};

use super::TestDatabase;

fn settings_with_margin(refresh_margin_seconds: u64) -> RuntimeSettingsUpdate {
    RuntimeSettingsUpdate {
        turn_state_probe_proxy_id: None,
        turn_state_probe_concurrency: None,
        admin_api_key: None,
        disable_fast: None,
        turn_state_injection_enabled: None,
        turn_state_models: vec![
            "gpt-6-astra".to_owned(),
            "gpt-5.6-sol".to_owned(),
            "gpt-5.6-terra".to_owned(),
        ],
        responses_max_decompressed_body_bytes: 64 * 1024 * 1024,
        refresh_margin_seconds,
        refresh_concurrency: 2,
        max_concurrent_per_account: 3,
        request_interval_ms: 50,
        rotation_strategy: "smart".to_owned(),
        model_mappings: BTreeMap::from([
            ("gpt-5.4".to_owned(), "gpt-5.5".to_owned()),
            ("grok-latest".to_owned(), "grok-4.5".to_owned()),
        ]),
        min_codex_desktop_version: None,
        min_codex_cli_version: None,
        usage_retention_days: 31,
        ops_event_retention_days: 30,
        audit_retention_days: 90,
        request_tuning: Default::default(),
    }
}

#[test]
fn runtime_settings_validate_probe_concurrency_bounds() {
    for (value, valid) in [
        (None, true),
        (Some(1), true),
        (Some(3), true),
        (Some(10), true),
        (Some(0), false),
        (Some(11), false),
    ] {
        let mut settings = settings_with_margin(3_600);
        settings.turn_state_probe_concurrency = value;
        assert_eq!(settings.validate().is_ok(), valid, "{value:?}");
    }
}

#[tokio::test]
async fn legacy_probe_concurrency_persists_without_changing_runtime_capacity() {
    let Some(database) = TestDatabase::create("probe_concurrency").await else {
        return;
    };
    let repository = PgRuntimeSettingsRepository::new(database.pool.clone());
    let snapshots = PgRuntimeSnapshotRepository::new(database.pool.clone());
    assert_eq!(
        repository
            .load_runtime_settings()
            .await
            .unwrap()
            .turn_state_probe_concurrency,
        3
    );
    for (value, expected) in [(Some(1), 1), (Some(10), 10), (None, 10), (Some(3), 3)] {
        let mut update = settings_with_margin(3_600);
        update.turn_state_probe_concurrency = value;
        repository.update_runtime_settings(update).await.unwrap();
        assert_eq!(
            repository
                .load_runtime_settings()
                .await
                .unwrap()
                .turn_state_probe_concurrency,
            expected
        );
        assert_eq!(
            snapshots
                .load_runtime_snapshot()
                .await
                .unwrap()
                .settings
                .max_concurrent_per_account,
            3
        );
    }
    let revision = repository
        .load_runtime_settings()
        .await
        .unwrap()
        .config_revision;
    for invalid in [0, 11] {
        let mut update = settings_with_margin(3_600);
        update.turn_state_probe_concurrency = Some(invalid);
        assert!(repository.update_runtime_settings(update).await.is_err());
        assert!(
            sqlx::query("update runtime_settings set turn_state_probe_concurrency = $1")
                .bind(i64::from(invalid))
                .execute(&database.pool)
                .await
                .is_err()
        );
    }
    assert_eq!(
        repository
            .load_runtime_settings()
            .await
            .unwrap()
            .config_revision,
        revision
    );
    database.close().await;
}

#[test]
fn runtime_settings_keep_account_rotation_global() {
    let settings = settings_with_margin(3_600);
    assert!(settings.validate().is_ok());
}

#[tokio::test]
async fn legacy_probe_proxy_is_preserved_but_never_resolved_by_runtime_snapshot() {
    let Some(database) = TestDatabase::create("probe_proxy_settings").await else {
        return;
    };
    let repository = PgRuntimeSettingsRepository::new(database.pool.clone());
    assert!(
        repository
            .load_runtime_settings()
            .await
            .unwrap()
            .turn_state_probe_proxy_id
            .is_none()
    );
    sqlx::query(
        "insert into outbound_proxies (id, name, proxy_url, last_test_success)
         values ('probe-proxy', 'Probe proxy', 'http://127.0.0.1:18080', true),
                ('untested-proxy', 'Untested', 'http://127.0.0.1:18081', null)",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let before = repository
        .load_runtime_settings()
        .await
        .unwrap()
        .config_revision;
    for id in ["missing-proxy", "untested-proxy"] {
        let mut update = settings_with_margin(3_600);
        update.turn_state_probe_proxy_id = Some(Some(id.to_owned()));
        assert!(repository.update_runtime_settings(update).await.is_err());
        assert_eq!(
            repository
                .load_runtime_settings()
                .await
                .unwrap()
                .config_revision,
            before
        );
    }
    let mut update = settings_with_margin(3_600);
    update.turn_state_probe_proxy_id = Some(Some("probe-proxy".to_owned()));
    repository.update_runtime_settings(update).await.unwrap();
    repository
        .update_runtime_settings(settings_with_margin(3_600))
        .await
        .unwrap();
    assert_eq!(
        repository
            .load_runtime_settings()
            .await
            .unwrap()
            .turn_state_probe_proxy_id
            .as_deref(),
        Some("probe-proxy")
    );
    let snapshots = PgRuntimeSnapshotRepository::new(database.pool.clone());
    assert_eq!(
        snapshots
            .load_runtime_snapshot()
            .await
            .unwrap()
            .settings
            .max_concurrent_per_account,
        3
    );
    assert!(
        sqlx::query("delete from outbound_proxies where id = 'probe-proxy'")
            .execute(&database.pool)
            .await
            .is_err()
    );
    sqlx::query(
        "update outbound_proxies set proxy_url = 'invalid-retired-proxy' where id = 'probe-proxy'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        snapshots
            .load_runtime_snapshot()
            .await
            .unwrap()
            .settings
            .max_concurrent_per_account,
        3
    );
    let mut update = settings_with_margin(3_600);
    update.turn_state_probe_proxy_id = Some(None);
    repository.update_runtime_settings(update).await.unwrap();
    assert_eq!(
        snapshots
            .load_runtime_snapshot()
            .await
            .unwrap()
            .settings
            .max_concurrent_per_account,
        3
    );
    sqlx::query("delete from outbound_proxies where id = 'probe-proxy'")
        .execute(&database.pool)
        .await
        .unwrap();
    database.close().await;
}

#[test]
fn runtime_settings_reject_invalid_model_mapping() {
    let settings = RuntimeSettingsUpdate {
        model_mappings: BTreeMap::from([("".to_owned(), "gpt-5.5".to_owned())]),
        ..settings_with_margin(3_600)
    };

    assert!(settings.validate().is_err());
}

#[test]
fn runtime_settings_reject_non_semver_client_min() {
    let settings = RuntimeSettingsUpdate {
        min_codex_cli_version: Some("v0.40.0".to_owned()),
        ..settings_with_margin(3_600)
    };

    assert!(settings.validate().is_err());
}

#[tokio::test]
async fn refresh_margin_change_should_preserve_existing_account_refresh_facts() {
    let Some(database) = TestDatabase::create("refresh_margin_reschedule").await else {
        return;
    };
    let expires_at = timestamp_micros(Utc::now() + TimeDelta::hours(2));
    insert_refreshable_account(
        &database.pool,
        "acct_refresh_margin_changed",
        expires_at,
        expires_at - TimeDelta::hours(1),
    )
    .await;
    let repository = PgRuntimeSettingsRepository::new(database.pool.clone());
    let before = account_refresh_facts(&database.pool, "acct_refresh_margin_changed").await;

    repository
        .update_runtime_settings(settings_with_margin(1_800))
        .await
        .expect("update refresh margin");

    assert_eq!(
        account_refresh_facts(&database.pool, "acct_refresh_margin_changed").await,
        before
    );
    database.close().await;
}

#[tokio::test]
async fn client_min_versions_should_round_trip_as_nullable_settings() {
    let Some(database) = TestDatabase::create("client_min_versions").await else {
        return;
    };
    let repository = PgRuntimeSettingsRepository::new(database.pool.clone());
    let mut update = settings_with_margin(3_600);
    update.min_codex_desktop_version = Some("26.825.6671".to_owned());
    update.min_codex_cli_version = Some("0.40.0".to_owned());

    repository
        .update_runtime_settings(update)
        .await
        .expect("update client min versions");
    let settings = repository
        .load_runtime_settings()
        .await
        .expect("load client min versions");

    assert_eq!(
        settings.min_codex_desktop_version.as_deref(),
        Some("26.825.6671")
    );
    assert_eq!(settings.min_codex_cli_version.as_deref(), Some("0.40.0"));
    database.close().await;
}

#[tokio::test]
async fn request_tuning_overrides_should_round_trip() {
    let Some(database) = TestDatabase::create("request_tuning_round_trip").await else {
        return;
    };
    let repository = PgRuntimeSettingsRepository::new(database.pool.clone());
    let mut update = settings_with_margin(3_600);
    update.request_tuning = RequestTuningOverrides {
        openai_request_location: Some(gateway_core::account::RequestLocation::default()),
        openai_location_override_enabled: Some(true),
        max_waiting_per_key: Some(8),
        key_concurrency_wait_timeout_seconds: Some(30),
        max_account_switches: Some(7),
        max_request_attempts: Some(8),
        websocket_max_retries: Some(9),
        websocket_http_fallback_enabled: Some(false),
        websocket_large_request_threshold_bytes: Some(4096),
        websocket_max_age_ms: Some(60_000),
        websocket_stream_idle_timeout_ms: Some(120_000),
        websocket_failure_threshold: Some(3),
        websocket_failure_window_ms: Some(30_000),
        websocket_failure_open_duration_ms: Some(45_000),
        rate_limit_cooldown_seconds: Some(60),
        account_busy_wait_enabled: Some(true),
        account_busy_wait_sticky_max_waiting: Some(4),
        account_busy_wait_sticky_timeout_seconds: Some(121),
        account_busy_wait_fallback_max_waiting: Some(101),
        account_busy_wait_fallback_timeout_seconds: Some(31),
    };
    let expected = update.request_tuning.clone();

    repository
        .update_runtime_settings(update)
        .await
        .expect("update request tuning");
    let settings = repository
        .load_runtime_settings()
        .await
        .expect("load request tuning");

    assert_eq!(settings.request_tuning, expected);
    let persisted: sqlx::types::Json<Value> =
        sqlx::query_scalar("select request_tuning_json from runtime_settings where id = 1")
            .fetch_one(&database.pool)
            .await
            .expect("load persisted tuning JSON");
    assert_eq!(
        persisted.0,
        serde_json::to_value(&expected).expect("serialize live overrides")
    );
    assert!(persisted.0.get("websocketMaxConnecting").is_none());

    let mut disabled = settings_with_margin(3_600);
    disabled.request_tuning = RequestTuningOverrides {
        account_busy_wait_enabled: Some(false),
        ..expected
    };
    let expected_disabled = disabled.request_tuning.clone();
    repository
        .update_runtime_settings(disabled)
        .await
        .expect("disable waiting without clearing numeric settings");
    let reloaded = PgRuntimeSettingsRepository::new(database.pool.clone())
        .load_runtime_settings()
        .await
        .expect("reload disabled waiting in a new repository");
    assert_eq!(reloaded.request_tuning, expected_disabled);
    assert!(reloaded.config_revision > settings.config_revision);
    assert_eq!(reloaded.model_mappings, settings.model_mappings);
    assert_eq!(
        reloaded.max_concurrent_per_account,
        settings.max_concurrent_per_account
    );
    assert_eq!(reloaded.rotation_strategy, settings.rotation_strategy);
    database.close().await;
}

#[tokio::test]
async fn legacy_request_tuning_should_load_in_settings_and_snapshot_and_disappear_on_save() {
    let Some(database) = TestDatabase::create("legacy_tuning").await else {
        return;
    };
    let repository = PgRuntimeSettingsRepository::new(database.pool.clone());
    let snapshots = PgRuntimeSnapshotRepository::new(database.pool.clone());
    for legacy in [
        json!({}),
        json!({"websocketMaxConnecting": 4}),
        json!({"websocketMaxConnecting": null}),
        json!({"websocketMaxConnecting": 4, "maxRequestAttempts": 8}),
        json!({
            "accountBusyWaitEnabled": null,
            "accountBusyWaitStickyMaxWaiting": null,
            "accountBusyWaitStickyTimeoutSeconds": null,
            "accountBusyWaitFallbackMaxWaiting": null,
            "accountBusyWaitFallbackTimeoutSeconds": null
        }),
    ] {
        sqlx::query("update runtime_settings set request_tuning_json = $1 where id = 1")
            .bind(sqlx::types::Json(&legacy))
            .execute(&database.pool)
            .await
            .expect("seed historical tuning JSON");
        let expected = RequestTuningOverrides {
            max_request_attempts: legacy["maxRequestAttempts"]
                .as_u64()
                .map(|value| u32::try_from(value).expect("u32")),
            ..Default::default()
        };
        let loaded = repository
            .load_runtime_settings()
            .await
            .expect("load historical settings");
        assert_eq!(loaded.request_tuning, expected);
        let snapshot = snapshots
            .load_runtime_snapshot()
            .await
            .expect("load historical snapshot");
        assert_eq!(snapshot.settings.request_tuning, expected);

        let mut update = settings_with_margin(3_600);
        update.request_tuning = loaded.request_tuning;
        repository
            .update_runtime_settings(update)
            .await
            .expect("save historical settings");
        let persisted: sqlx::types::Json<Value> =
            sqlx::query_scalar("select request_tuning_json from runtime_settings where id = 1")
                .fetch_one(&database.pool)
                .await
                .expect("load rewritten tuning JSON");
        assert_eq!(
            persisted.0,
            serde_json::to_value(&expected).expect("serialize live overrides")
        );
        assert!(persisted.0.get("websocketMaxConnecting").is_none());
        assert_eq!(
            repository
                .load_runtime_settings()
                .await
                .expect("reload current settings")
                .request_tuning,
            expected
        );
    }
    database.close().await;
}

#[tokio::test]
async fn persisted_request_tuning_should_still_reject_unrelated_unknown_fields() {
    let Some(database) = TestDatabase::create("unknown_tuning").await else {
        return;
    };
    sqlx::query("update runtime_settings set request_tuning_json = $1 where id = 1")
        .bind(sqlx::types::Json(json!({
            "websocketMaxConnecting": 4,
            "websocketMaxConnectng": 4
        })))
        .execute(&database.pool)
        .await
        .expect("seed invalid tuning JSON");
    assert!(
        PgRuntimeSettingsRepository::new(database.pool.clone())
            .load_runtime_settings()
            .await
            .is_err()
    );
    assert!(
        PgRuntimeSnapshotRepository::new(database.pool.clone())
            .load_runtime_snapshot()
            .await
            .is_err()
    );
    database.close().await;
}

#[tokio::test]
async fn decompression_limit_round_trips_to_settings_and_snapshot() {
    let Some(database) = TestDatabase::create("decompression_limit_settings").await else {
        return;
    };
    let repository = PgRuntimeSettingsRepository::new(database.pool.clone());
    let mut update = settings_with_margin(3_600);
    update.responses_max_decompressed_body_bytes = 128 * 1024 * 1024;
    repository
        .update_runtime_settings(update)
        .await
        .expect("update decompression limit");

    let loaded = repository
        .load_runtime_settings()
        .await
        .expect("load decompression limit");
    assert_eq!(
        loaded.responses_max_decompressed_body_bytes,
        128 * 1024 * 1024
    );
    let snapshot = PgRuntimeSnapshotRepository::new(database.pool.clone())
        .load_runtime_snapshot()
        .await
        .expect("load snapshot");
    assert_eq!(
        snapshot.settings.responses_max_decompressed_body_bytes,
        128 * 1024 * 1024
    );

    for invalid in [0, 256 * 1024 * 1024 + 1] {
        let mut update = settings_with_margin(3_600);
        update.responses_max_decompressed_body_bytes = invalid;
        assert!(repository.update_runtime_settings(update).await.is_err());
    }
    database.close().await;
}

#[tokio::test]
async fn disable_fast_persists_and_omitted_updates_preserve_the_policy() {
    let Some(database) = TestDatabase::create("disable_fast_settings").await else {
        return;
    };
    let repository = PgRuntimeSettingsRepository::new(database.pool.clone());
    assert!(
        !repository
            .load_runtime_settings()
            .await
            .unwrap()
            .disable_fast
    );

    let mut update = settings_with_margin(3_600);
    update.disable_fast = Some(true);
    repository
        .update_runtime_settings(update)
        .await
        .expect("enable disable-fast");
    assert!(
        repository
            .load_runtime_settings()
            .await
            .unwrap()
            .disable_fast
    );

    let mut update = settings_with_margin(3_600);
    update.disable_fast = None;
    repository
        .update_runtime_settings(update)
        .await
        .expect("preserve disable-fast");
    assert!(
        repository
            .load_runtime_settings()
            .await
            .unwrap()
            .disable_fast
    );
    assert!(
        PgRuntimeSnapshotRepository::new(database.pool.clone())
            .load_runtime_snapshot()
            .await
            .unwrap()
            .settings
            .disable_fast
    );
    database.close().await;
}

#[tokio::test]
async fn unchanged_refresh_margin_should_preserve_existing_account_refresh_facts() {
    let Some(database) = TestDatabase::create("refresh_margin_unchanged").await else {
        return;
    };
    let expires_at = timestamp_micros(Utc::now() + TimeDelta::hours(2));
    let retry_at = timestamp_micros(Utc::now() + TimeDelta::minutes(5));
    insert_refreshable_account(
        &database.pool,
        "acct_refresh_margin_unchanged",
        expires_at,
        retry_at,
    )
    .await;
    let repository = PgRuntimeSettingsRepository::new(database.pool.clone());
    let before = account_refresh_facts(&database.pool, "acct_refresh_margin_unchanged").await;

    repository
        .update_runtime_settings(settings_with_margin(3_600))
        .await
        .expect("update unrelated runtime settings");

    assert_eq!(
        account_refresh_facts(&database.pool, "acct_refresh_margin_unchanged").await,
        before
    );
    database.close().await;
}

async fn insert_refreshable_account(
    pool: &sqlx::PgPool,
    account_id: &str,
    expires_at: DateTime<Utc>,
    next_refresh_at: DateTime<Utc>,
) {
    sqlx::query(
        "insert into provider_accounts (
           id, provider_kind, name, upstream_user_id, authentication_kind,
           provider_credentials_json, has_refresh_token, access_token_expires_at,
           next_refresh_at, credential_state, credential_observed_at, created_at, updated_at
         ) values ($1, 'openai', $1, $1, 'oauth', '{}'::jsonb, true, $2, $3,
                   'ready', now(), now(), now())",
    )
    .bind(account_id)
    .bind(expires_at)
    .bind(next_refresh_at)
    .execute(pool)
    .await
    .expect("insert refreshable account");
}

#[derive(Debug, PartialEq, Eq)]
struct AccountRefreshFacts {
    access_token_expires_at: DateTime<Utc>,
    next_refresh_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    credential_revision: i64,
}

async fn account_refresh_facts(pool: &sqlx::PgPool, account_id: &str) -> AccountRefreshFacts {
    let (access_token_expires_at, next_refresh_at, updated_at, credential_revision) =
        sqlx::query_as(
            "select access_token_expires_at, next_refresh_at, updated_at, credential_revision
         from provider_accounts
         where id = $1",
        )
        .bind(account_id)
        .fetch_one(pool)
        .await
        .expect("load account refresh facts");
    AccountRefreshFacts {
        access_token_expires_at,
        next_refresh_at,
        updated_at,
        credential_revision,
    }
}

fn timestamp_micros(value: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(value.timestamp_micros()).expect("valid test timestamp")
}
