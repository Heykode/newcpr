use super::TestDatabase;

pub(super) fn before_observed_lease() -> sqlx::migrate::Migrator {
    sqlx::migrate::Migrator {
        migrations: super::TEST_MIGRATOR
            .iter()
            .filter(|migration| migration.version < 36)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    }
}

async fn insert_legacy_account(database: &TestDatabase) {
    // Use the old schema, not the latest repository's required columns.
    sqlx::query(
        "insert into provider_accounts (
            id, provider_kind, name, upstream_user_id, authentication_kind,
            provider_credentials_json, has_refresh_token,
            credential_observed_at, created_at, updated_at
        ) values (
            'upgrade-owner', 'openai', 'Upgrade', 'upgrade-user', 'oauth',
            '{}'::jsonb, false, now(), now(), now()
        )",
    )
    .execute(&database.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn model_access_and_excel_upgrade_default_without_touching_identity_or_inheriting_state() {
    let old = sqlx::migrate::Migrator {
        migrations: super::TEST_MIGRATOR
            .iter()
            .filter(|migration| migration.version <= 34)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let Some(database) = TestDatabase::create_with_migrator("model_access_upgrade", &old).await
    else {
        return;
    };
    insert_legacy_account(&database).await;
    sqlx::query("update provider_accounts set turn_state_injection_enabled=true")
        .execute(&database.pool)
        .await
        .unwrap();
    let before: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    super::TEST_MIGRATOR.run(&database.pool).await.unwrap();
    super::TEST_MIGRATOR.run(&database.pool).await.unwrap();
    let mut after: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(
        after.as_object_mut().unwrap().remove("model_access_json"),
        Some(serde_json::json!({"mode":"all","models":[]})),
    );
    assert_eq!(
        after.as_object_mut().unwrap().remove("responses_upstream"),
        Some(serde_json::json!("codex")),
    );
    assert_eq!(
        after.as_object_mut().unwrap().remove("excel_models"),
        Some(serde_json::json!(["gpt-5.6-sol"]))
    );
    assert_eq!(
        after
            .as_object_mut()
            .unwrap()
            .remove("excel_models_follow_global"),
        Some(serde_json::json!(false))
    );
    assert_eq!(
        after
            .as_object_mut()
            .unwrap()
            .remove("excel_cache_creation_as_input"),
        Some(serde_json::json!(false))
    );
    assert_eq!(before, after);
    database.close().await;
}

#[tokio::test]
async fn probe_concurrency_upgrade_defaults_to_three_without_touching_state_or_settings() {
    let old = sqlx::migrate::Migrator {
        migrations: super::TEST_MIGRATOR
            .iter()
            .filter(|migration| migration.version <= 30)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let Some(database) =
        TestDatabase::create_with_migrator("probe_concurrency_upgrade", &old).await
    else {
        return;
    };
    insert_legacy_account(&database).await;
    sqlx::query(
        "insert into provider_turn_states (
            provider_account_id, upstream_model, credential_revision, normal_length,
            active_state, active_issued_at, active_expires_at, refresh_status,
            probe_attempts, probe_total_attempts
         ) values (
            'upgrade-owner', 'model-a', 1, 332, 'synthetic-active',
            now() - interval '10 minutes', now() + interval '50 minutes', 'ready', 3, 42
         )",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let state_before: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(provider_turn_states) from provider_turn_states")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    let settings_before: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(runtime_settings) from runtime_settings")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    // The later lease migration deliberately shortens clocks; isolate this contract.
    before_observed_lease().run(&database.pool).await.unwrap();
    before_observed_lease().run(&database.pool).await.unwrap();
    let state_after: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(provider_turn_states) from provider_turn_states")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    let mut settings_after: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(runtime_settings) from runtime_settings")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(
        settings_after
            .as_object_mut()
            .unwrap()
            .remove("turn_state_probe_concurrency"),
        Some(serde_json::json!(3))
    );
    assert_eq!(settings_after, settings_before);
    assert_eq!(state_after, state_before);
    database.close().await;
}

#[tokio::test]
async fn lifecycle_upgrade_preserves_existing_state_and_rejects_old_migrator() {
    let old = sqlx::migrate::Migrator {
        migrations: super::TEST_MIGRATOR
            .iter()
            .filter(|migration| migration.version <= 27)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let Some(database) = TestDatabase::create_with_migrator("state_upgrade", &old).await else {
        return;
    };
    insert_legacy_account(&database).await;
    sqlx::query(
        "insert into provider_turn_states (
             provider_account_id, upstream_model, credential_revision, normal_length,
             active_state, active_issued_at, active_expires_at,
             standby_state, standby_issued_at, standby_expires_at, refresh_status
         ) values (
             'upgrade-owner', 'model-a', 1, 332, 'synthetic-active',
             now() - interval '10 minutes', now() + interval '50 minutes',
             'synthetic-candidate', now(), now() + interval '1 hour', 'refreshing'
         )",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let before: (String, String, String, String) = sqlx::query_as(
        "select active_state, standby_state, active_expires_at::text,
                standby_expires_at::text from provider_turn_states",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    before_observed_lease().run(&database.pool).await.unwrap();
    let after: (String, String, String, String) = sqlx::query_as(
        "select active_state, standby_state, active_expires_at::text,
                standby_expires_at::text from provider_turn_states",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    let progress: (String, i64, Option<String>, Option<i64>) = sqlx::query_as(
        "select refresh_status, probe_attempts, last_probe_reason, successful_probe_attempt
         from provider_turn_states",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(progress, ("missing".into(), 0, None, None));
    let proxy: Option<String> =
        sqlx::query_scalar("select turn_state_probe_proxy_id from runtime_settings")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(proxy, None);
    // A failed SQLx migration can retain its session advisory lock. Production
    // closes the migration pool on error; discard this test connection likewise.
    let mut connection = database.pool.acquire().await.unwrap();
    let downgrade = old.run(&mut *connection).await;
    connection.close().await.unwrap();
    assert!(matches!(
        downgrade,
        Err(sqlx::migrate::MigrateError::VersionMissing(version)) if version > 27
    ));
    super::TEST_MIGRATOR.run(&database.pool).await.unwrap();
    database.close().await;
}
