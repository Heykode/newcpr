use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use gateway_store::postgres::{
    PgRetentionRepository, RetentionCycleBudget, RetentionRepository as _, RuntimeRetentionSettings,
};

use super::TestDatabase;

#[test]
fn retention_settings_preserve_independent_windows() {
    let settings = RuntimeRetentionSettings {
        usage_retention_days: 31,
        ops_event_retention_days: 30,
        audit_retention_days: 90,
    };
    assert_eq!(settings.audit_retention_days, 90);
}

#[tokio::test]
async fn retention_cycle_should_stop_at_the_batch_budget() {
    let Some(database) = TestDatabase::create("retention_cycle_budget").await else {
        return;
    };
    let expired_at = Utc::now() - ChronoDuration::days(100);
    sqlx::query(
        "insert into admin_audit_events (
           id, actor_kind, actor_ref, action, entity_kind, entity_ref,
           changed_fields, created_at
         )
         select 'retention-' || value::text, 'system', 'retention', 'cleanup',
                'fixture', 'fixture-' || value::text, array[]::text[], $1
         from generate_series(1, 5) as value",
    )
    .bind(expired_at)
    .execute(&database.pool)
    .await
    .expect("seed expired audit events");

    let budget = RetentionCycleBudget::try_new(2, 3, Duration::from_secs(1), Duration::ZERO)
        .expect("retention cycle budget");
    let repository = PgRetentionRepository::with_cycle_budget(database.pool.clone(), budget);
    let report = repository
        .apply_retention(
            Utc::now(),
            RuntimeRetentionSettings {
                usage_retention_days: 31,
                ops_event_retention_days: 30,
                audit_retention_days: 90,
            },
        )
        .await
        .expect("bounded retention cycle");

    assert_eq!(report.model_requests, 0);
    assert_eq!(report.ops_events, 0);
    assert_eq!(report.admin_audit_events, 2);
    assert_eq!(report.batches, 3);
    assert!(report.budget_exhausted);
    let remaining: i64 =
        sqlx::query_scalar("select count(*) from admin_audit_events where id like 'retention-%'")
            .fetch_one(&database.pool)
            .await
            .expect("count remaining audit events");
    assert_eq!(remaining, 3);
    database.close().await;
}

#[tokio::test]
async fn positive_retention_cleans_only_expired_records_without_date_overflow() {
    let Some(database) = TestDatabase::create("retention_positive_days").await else {
        return;
    };
    let now = Utc::now();
    for (id, age) in [("old", 400), ("month", 40), ("recent", 1)] {
        let at = now - ChronoDuration::days(age);
        sqlx::query("insert into model_requests (
            id,client_api_key_ref,config_revision,protocol,operation,endpoint,client_transport,
            outcome,client_status_code,started_at,deadline_at,completed_at,downstream_committed_at,routing_scope
        ) values ($1,'fixture-key',1,'openai','responses','/v1/responses','http_sse',
            'succeeded',200,$2,$2,$2,$2,'all')")
            .bind(id).bind(at).execute(&database.pool).await.unwrap();
    }
    let repository = PgRetentionRepository::new(database.pool.clone());
    for (days, expected_deleted) in [
        (u32::MAX, 0),
        (10_000_000, 0),
        (3651, 0),
        (365, 1),
        (32, 1),
        (1, 0),
    ] {
        let report = repository
            .apply_retention(
                now,
                RuntimeRetentionSettings {
                    usage_retention_days: days,
                    ops_event_retention_days: days,
                    audit_retention_days: days,
                },
            )
            .await
            .unwrap();
        assert_eq!(report.model_requests, expected_deleted, "days={days}");
    }
    let remaining: Vec<String> = sqlx::query_scalar("select id from model_requests order by id")
        .fetch_all(&database.pool)
        .await
        .unwrap();
    assert_eq!(remaining, ["recent"]);
    database.close().await;
}
