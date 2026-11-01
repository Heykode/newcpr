use gateway_store::postgres::{PgRuntimeSettingsRepository, RuntimeSettingsRepository};
use serde_json::{Value, json};

use super::TestDatabase;

#[tokio::test]
async fn shared_account_wait_migration_retires_split_values_and_preserves_other_settings() {
    let old = sqlx::migrate::Migrator {
        migrations: super::TEST_MIGRATOR
            .iter()
            .filter(|migration| migration.version < 83)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    for tuning in [
        Value::Null,
        json!({"accountBusyWaitEnabled": false, "accountBusyWaitStickyMaxWaiting":5,
            "accountBusyWaitStickyTimeoutSeconds":40,"accountBusyWaitFallbackMaxWaiting":10,
            "accountBusyWaitFallbackTimeoutSeconds":60,"websocketMaxRetries":3,
            "maxWaitingPerKey":7,"keyConcurrencyWaitTimeoutSeconds":45}),
        json!({"accountBusyWaitEnabled":true,"maxWaitingPerAccount":5,
            "accountConcurrencyWaitTimeoutSeconds":60}),
    ] {
        let Some(database) = TestDatabase::create_with_migrator("shared_account_wait", &old).await
        else {
            return;
        };
        sqlx::query("update runtime_settings set request_tuning_json=$1 where id=1")
            .bind(if tuning.is_null() {
                None
            } else {
                Some(tuning.clone())
            })
            .execute(&database.pool)
            .await
            .unwrap();
        let before: Value = sqlx::query_scalar(
            "select to_jsonb(s) - 'request_tuning_json' from runtime_settings s where id=1",
        )
        .fetch_one(&database.pool)
        .await
        .unwrap();
        super::TEST_MIGRATOR.run(&database.pool).await.unwrap();
        let after: Value =
            sqlx::query_scalar("select to_jsonb(s) from runtime_settings s where id=1")
                .fetch_one(&database.pool)
                .await
                .unwrap();
        let mut unrelated = after.clone();
        unrelated
            .as_object_mut()
            .unwrap()
            .remove("request_tuning_json");
        assert_eq!(before, unrelated);
        let mut expected = tuning.as_object().cloned().unwrap_or_default();
        for key in [
            "accountBusyWaitStickyMaxWaiting",
            "accountBusyWaitStickyTimeoutSeconds",
            "accountBusyWaitFallbackMaxWaiting",
            "accountBusyWaitFallbackTimeoutSeconds",
        ] {
            expected.remove(key);
        }
        expected.entry("maxWaitingPerAccount").or_insert(json!(10));
        expected
            .entry("accountConcurrencyWaitTimeoutSeconds")
            .or_insert(json!(30));
        assert_eq!(
            after["request_tuning_json"],
            Value::Object(expected.clone())
        );
        let settings = PgRuntimeSettingsRepository::new(database.pool.clone())
            .load_runtime_settings()
            .await
            .unwrap();
        assert_eq!(
            settings.request_tuning.max_waiting_per_account,
            Some(u32::try_from(expected["maxWaitingPerAccount"].as_u64().unwrap()).unwrap())
        );
        assert_eq!(
            settings
                .request_tuning
                .account_concurrency_wait_timeout_seconds,
            expected["accountConcurrencyWaitTimeoutSeconds"].as_u64()
        );
        sqlx::raw_sql(include_str!(
            "../../../../migrations/0083_shared_account_wait.sql"
        ))
        .execute(&database.pool)
        .await
        .unwrap();
        let repeated: Value =
            sqlx::query_scalar("select to_jsonb(s) from runtime_settings s where id=1")
                .fetch_one(&database.pool)
                .await
                .unwrap();
        assert_eq!(after, repeated);
        database.close().await;
    }
}
