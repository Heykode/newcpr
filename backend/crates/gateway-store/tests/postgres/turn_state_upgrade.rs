use gateway_store::postgres::{PgProviderAccountRepository, ProviderAccountRepository};

use super::{TestDatabase, provider_accounts::account};

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
    PgProviderAccountRepository::new(database.pool.clone())
        .insert_provider_account(account("upgrade-owner", "upgrade-user"))
        .await
        .unwrap();
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
    super::TEST_MIGRATOR.run(&database.pool).await.unwrap();
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
        Err(sqlx::migrate::MigrateError::VersionMissing(28 | 29))
    ));
    super::TEST_MIGRATOR.run(&database.pool).await.unwrap();
    database.close().await;
}
