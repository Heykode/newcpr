use chrono::{DateTime, Duration, Utc};

use super::*;

async fn set_failure(pool: &sqlx::PgPool, id: &str, state: &str, reason: &str, at: DateTime<Utc>) {
    sqlx::query("update provider_accounts set credential_state=$2, last_error_reason=$3, credential_observed_at=$4, updated_at=greatest(updated_at,$4) where id=$1")
        .bind(id).bind(state).bind(reason).bind(at).execute(pool).await.unwrap();
}

#[tokio::test]
async fn lifecycle_uses_cpr_terminal_facts_and_retracts_recovered_samples() {
    let Some(db) = TestDatabase::create("monitor_lifecycle").await else {
        return;
    };
    let now = Utc::now();
    let groups = PgAccountGroupRepository::new(db.pool.clone());
    for id in ["acct_ban", "acct_revoked", "acct_expiring", "acct_invalid"] {
        seed_account(&db.pool, id, "openai", id).await;
    }
    sqlx::query(
        "update provider_accounts set plan_type='plus', created_at=$1, access_token_expires_at=$2",
    )
    .bind(now - Duration::days(10))
    .bind(now + Duration::days(30))
    .execute(&db.pool)
    .await
    .unwrap();
    set_failure(
        &db.pool,
        "acct_ban",
        "banned",
        "account_banned",
        now - Duration::minutes(1),
    )
    .await;
    set_failure(
        &db.pool,
        "acct_revoked",
        "expired",
        "credential_expired",
        now,
    )
    .await;
    set_failure(
        &db.pool,
        "acct_invalid",
        "invalid",
        "credential_invalid",
        now,
    )
    .await;
    // Merely expired access tokens, even with 401 in display text, are not terminal facts.
    sqlx::query("update provider_accounts set access_token_expires_at=$1, last_error_message='401 Unauthorized' where id='acct_expiring'")
        .bind(now - Duration::minutes(1)).execute(&db.pool).await.unwrap();
    let facts = groups.load_group_monitor(now).await.unwrap();
    assert_eq!(facts.lifespans[0].samples, 1);
    let confirmed = now + Duration::minutes(2);
    let facts = groups.load_group_monitor(confirmed).await.unwrap();
    assert_eq!(facts.lifespans[0].samples, 3);
    let count: i64 = sqlx::query_scalar("select count(*) from monitor_account_lifecycles")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 4);
    let later = confirmed + Duration::minutes(1);
    sqlx::query("update provider_accounts set credential_state='ready', last_error_reason=null, last_error_message=null, credential_observed_at=$1, updated_at=$1 where id='acct_revoked'")
        .bind(later).execute(&db.pool).await.unwrap();
    assert_eq!(
        groups.load_group_monitor(later).await.unwrap().lifespans[0].samples,
        3,
        "import alone is not recovery"
    );
    seed_group_cost_snapshot(&db.pool, "req_recovered", "acct_revoked", MIXED_GROUP, "1").await;
    sqlx::query(
        "update model_requests set started_at=$1, completed_at=$2 where id='req_recovered'",
    )
    .bind(later - Duration::seconds(1))
    .bind(later + Duration::seconds(2))
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        groups
            .load_group_monitor(later + Duration::seconds(3))
            .await
            .unwrap()
            .lifespans[0]
            .samples,
        3,
        "old in-flight request is not new-credential recovery"
    );
    sqlx::query(
        "update model_requests set started_at=$1, completed_at=$2 where id='req_recovered'",
    )
    .bind(later + Duration::seconds(1))
    .bind(later + Duration::seconds(2))
    .execute(&db.pool)
    .await
    .unwrap();
    let recovered = later + Duration::seconds(10);
    assert_eq!(
        groups
            .load_group_monitor(recovered)
            .await
            .unwrap()
            .lifespans[0]
            .samples,
        2
    );
    assert!(sqlx::query_scalar::<_, Option<DateTime<Utc>>>("select dead_at from monitor_account_lifecycles where identity_key=jsonb_build_array('upstream'::text,'openai'::text,'acct_revoked-user'::text,''::text)::text")
        .fetch_one(&db.pool).await.unwrap().is_none());
    // An older sampling request cannot reintroduce the previous failure.
    groups.load_group_monitor(confirmed).await.unwrap();
    assert!(sqlx::query_scalar::<_, Option<DateTime<Utc>>>("select dead_at from monitor_account_lifecycles where identity_key=jsonb_build_array('upstream'::text,'openai'::text,'acct_revoked-user'::text,''::text)::text")
        .fetch_one(&db.pool).await.unwrap().is_none());
    set_failure(
        &db.pool,
        "acct_revoked",
        "banned",
        "account_banned",
        recovered + Duration::days(1),
    )
    .await;
    let facts = groups
        .load_group_monitor(recovered + Duration::days(1))
        .await
        .unwrap();
    assert_eq!(facts.lifespans[0].samples, 3);
    let original: DateTime<Utc> = sqlx::query_scalar("select account_created_at from monitor_account_lifecycles where identity_key=jsonb_build_array('upstream'::text,'openai'::text,'acct_revoked-user'::text,''::text)::text")
        .fetch_one(&db.pool).await.unwrap();
    assert!(
        (original - (now - Duration::days(10)))
            .num_milliseconds()
            .abs()
            < 1
    );
    db.close().await;
}

#[tokio::test]
async fn lifecycle_does_not_merge_different_workspaces_or_plain_401_failures() {
    let Some(db) = TestDatabase::create("monitor_lifecycle_scope").await else {
        return;
    };
    let now = Utc::now();
    let groups = PgAccountGroupRepository::new(db.pool.clone());
    for id in ["acct_space_a", "acct_space_b"] {
        seed_account(&db.pool, id, "openai", id).await;
        sqlx::query("update provider_accounts set upstream_user_id='same-user', upstream_account_id=$1, email='same@example.test', plan_type='plus', created_at=$2, last_error_message='401 Unauthorized' where id=$1")
            .bind(id).bind(now - Duration::days(5)).execute(&db.pool).await.unwrap();
    }
    set_failure(
        &db.pool,
        "acct_space_a",
        "banned",
        "account_banned",
        now - Duration::minutes(1),
    )
    .await;
    let facts = groups.load_group_monitor(now).await.unwrap();
    assert_eq!(facts.lifespans[0].samples, 1);
    seed_group_cost_snapshot(
        &db.pool,
        "req_other_space",
        "acct_space_b",
        MIXED_GROUP,
        "1",
    )
    .await;
    sqlx::query(
        "update model_requests set started_at=$1, completed_at=$2 where id='req_other_space'",
    )
    .bind(now + Duration::seconds(1))
    .bind(now + Duration::seconds(2))
    .execute(&db.pool)
    .await
    .unwrap();
    let facts = groups
        .load_group_monitor(now + Duration::minutes(3))
        .await
        .unwrap();
    assert_eq!(facts.lifespans[0].samples, 1);
    let identities: i64 = sqlx::query_scalar("select count(*) from monitor_account_lifecycles")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(identities, 2);
    db.close().await;
}

#[tokio::test]
async fn lifecycle_keeps_latest_five_and_preserves_deleted_identity_without_counting_deletions() {
    let Some(db) = TestDatabase::create("monitor_lifecycle_history").await else {
        return;
    };
    let now = Utc::now();
    let groups = PgAccountGroupRepository::new(db.pool.clone());
    for index in 0..6 {
        let id = format!("acct_sample_{index}");
        seed_account(&db.pool, &id, "openai", &id).await;
        let death = now - Duration::minutes(6 - index);
        sqlx::query("update provider_accounts set plan_type='plus', created_at=$2 where id=$1")
            .bind(&id)
            .bind(death - Duration::hours(index + 1))
            .execute(&db.pool)
            .await
            .unwrap();
        set_failure(&db.pool, &id, "banned", "account_banned", death).await;
    }
    sqlx::query("insert into provider_device_identities (provider_kind,upstream_user_id,upstream_account_id,installation_id) values ('openai','acct_sample_5-user','','life-stable')")
        .execute(&db.pool).await.unwrap();
    let facts = groups.load_group_monitor(now).await.unwrap();
    assert_eq!(facts.lifespans[0].samples, 5);
    assert!((facts.lifespans[0].minutes - 240.0).abs() < 0.001);
    sqlx::query("delete from provider_accounts where id='acct_sample_5'")
        .execute(&db.pool)
        .await
        .unwrap();
    let facts = groups
        .load_group_monitor(now + Duration::seconds(10))
        .await
        .unwrap();
    assert_eq!(facts.lifespans[0].samples, 5);
    seed_account(&db.pool, "acct_reimported", "openai", "Reimported").await;
    sqlx::query("update provider_accounts set upstream_user_id='acct_sample_5-user', plan_type='plus', created_at=$1, credential_observed_at=$1 where id='acct_reimported'")
        .bind(now).execute(&db.pool).await.unwrap();
    let facts = groups
        .load_group_monitor(now + Duration::seconds(20))
        .await
        .unwrap();
    assert_eq!(
        facts.lifespans[0].samples, 5,
        "unverified reimport retains death"
    );
    seed_group_cost_snapshot(
        &db.pool,
        "req_reimported_ok",
        "acct_reimported",
        MIXED_GROUP,
        "1",
    )
    .await;
    sqlx::query(
        "update model_requests set started_at=$1, completed_at=$2 where id='req_reimported_ok'",
    )
    .bind(now + Duration::seconds(21))
    .bind(now + Duration::seconds(22))
    .execute(&db.pool)
    .await
    .unwrap();
    let facts = groups
        .load_group_monitor(now + Duration::seconds(30))
        .await
        .unwrap();
    assert_eq!(facts.lifespans[0].samples, 5);
    assert!(
        (facts.lifespans[0].minutes - 180.0).abs() < 0.001,
        "recovered newest is replaced by the older sample"
    );
    let original: DateTime<Utc> = sqlx::query_scalar("select account_created_at from monitor_account_lifecycles where identity_key=jsonb_build_array('upstream'::text,'openai'::text,'acct_sample_5-user'::text,''::text)::text")
        .fetch_one(&db.pool).await.unwrap();
    assert!(
        (original - (now - Duration::minutes(1) - Duration::hours(6)))
            .num_milliseconds()
            .abs()
            < 1
    );
    let before: i64 = sqlx::query_scalar(
        "select count(*) from monitor_account_lifecycles where dead_at is not null",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("delete from provider_accounts where id='acct_reimported'")
        .execute(&db.pool)
        .await
        .unwrap();
    groups
        .load_group_monitor(now + Duration::seconds(40))
        .await
        .unwrap();
    let after: i64 = sqlx::query_scalar(
        "select count(*) from monitor_account_lifecycles where dead_at is not null",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(before, after, "manual deletion is not death");
    db.close().await;
}
