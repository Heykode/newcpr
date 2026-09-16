use chrono::{Duration, Utc};
use gateway_admin::{
    model::quota_learning::{QuotaLearningObservation, QuotaLearningSource},
    ports::store::AccountStore,
};

use super::{TestDatabase, admin_account_store};

async fn seed_account(pool: &sqlx::PgPool, id: &str) {
    sqlx::query(
        "insert into provider_accounts (
           id, provider_kind, name, email, upstream_user_id, plan_type,
           authentication_kind, provider_credentials_json, credential_revision,
           has_refresh_token, enabled, credential_state, credential_observed_at,
           created_at, updated_at
         ) values (
           $1, 'openai', $1, null, $1 || '-user', 'plus',
           'oauth', '{}'::jsonb, 1, false, true, 'ready', now(), now(), now()
         )",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("seed learning account");
}

fn observation(
    account_id: &str,
    reset_at: chrono::DateTime<Utc>,
    percent: f64,
    cost: f64,
) -> QuotaLearningObservation {
    QuotaLearningObservation {
        account_id: account_id.to_owned(),
        provider_kind: "openai".to_owned(),
        plan_type: "Plus".to_owned(),
        window_key: "weekly".to_owned(),
        window_minutes: 10_080,
        used_percent: percent,
        reset_at,
        observed_at: Utc::now(),
        observed_cost_usd: Some(cost),
    }
}

fn observation_without_cost(
    account_id: &str,
    reset_at: chrono::DateTime<Utc>,
    percent: f64,
) -> QuotaLearningObservation {
    let mut result = observation(account_id, reset_at, percent, 0.0);
    result.observed_cost_usd = None;
    result
}

#[tokio::test]
async fn quota_learning_keeps_plan_average_after_account_deletion() {
    let Some(database) = TestDatabase::create("quota_learning").await else {
        return;
    };
    for id in ["acct_learn_a", "acct_learn_b", "acct_learn_c"] {
        seed_account(&database.pool, id).await;
    }
    let store = admin_account_store(&database.pool);
    let reset_at = Utc::now() + Duration::days(6);
    seed_account(&database.pool, "acct_learn_oldest").await;
    store
        .record_quota_learning(&[observation("acct_learn_oldest", reset_at, 0.0, 0.0)])
        .await
        .unwrap();
    store
        .record_quota_learning(&[observation("acct_learn_oldest", reset_at, 10.0, 5.0)])
        .await
        .unwrap();

    for (id, cost) in [
        ("acct_learn_a", 5.0),
        ("acct_learn_b", 6.0),
        ("acct_learn_c", 7.5),
    ] {
        store
            .record_quota_learning(&[observation(id, reset_at, 10.0, 0.0)])
            .await
            .expect("record learning baseline");
        let result = store
            .record_quota_learning(&[observation(id, reset_at, 15.0, cost)])
            .await
            .expect("record learning binding");
        assert_eq!(result[0].source, QuotaLearningSource::Personal);
    }

    sqlx::query("delete from provider_accounts where id like 'acct_learn_%'")
        .execute(&database.pool)
        .await
        .expect("delete learning accounts");
    let sample_count: i64 = sqlx::query_scalar(
        "select count(*) from quota_learning_plan_samples
          where provider_kind = 'openai' and lower(plan_type) = 'plus'
            and window_key = 'weekly'",
    )
    .fetch_one(&database.pool)
    .await
    .expect("count retained plan samples");
    assert_eq!(sample_count, 3);
    let personal_count: i64 = sqlx::query_scalar("select count(*) from quota_learning_accounts")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(personal_count, 0);

    seed_account(&database.pool, "acct_learn_new").await;
    let result = admin_account_store(&database.pool)
        .record_quota_learning(&[observation_without_cost("acct_learn_new", reset_at, 20.0)])
        .await
        .expect("load plan average");
    assert_eq!(result[0].source, QuotaLearningSource::PlanAverage);
    assert_eq!(result[0].sample_count, 3);
    assert!((result[0].effective_limit_usd.unwrap() - 123.3333333333).abs() < 0.000001);

    database.close().await;
}

#[tokio::test]
async fn quota_learning_requires_both_thresholds_and_deduplicates_concurrent_reads() {
    let Some(database) = TestDatabase::create("learning_thresholds").await else {
        return;
    };
    let id = "acct_threshold";
    seed_account(&database.pool, id).await;
    let store = admin_account_store(&database.pool);
    let reset = Utc::now() + Duration::days(6);
    for (percent, cost) in [(20.0, 0.0), (25.0, 4.99), (22.0, 5.0)] {
        let estimate = store
            .record_quota_learning(&[observation(id, reset, percent, cost)])
            .await
            .unwrap();
        assert_eq!(estimate[0].source, QuotaLearningSource::Learning);
    }
    // The percentage regression establishes a fresh baseline: 22%, $5.
    let observations = [observation(id, reset, 25.0, 11.0)];
    let (first, second) = tokio::join!(
        store.record_quota_learning(&observations),
        store.record_quota_learning(&observations),
    );
    for result in [first.unwrap(), second.unwrap()] {
        assert_eq!(result[0].effective_limit_usd, Some(200.0));
        assert_eq!(result[0].source, QuotaLearningSource::Personal);
        assert_eq!(result[0].sample_count, 1);
    }
    let rows: i64 = sqlx::query_scalar("select count(*) from quota_learning_plan_samples")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);
    let before: chrono::DateTime<Utc> =
        sqlx::query_scalar("select updated_at from quota_learning_accounts")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    store.record_quota_learning(&observations).await.unwrap();
    let after: chrono::DateTime<Utc> =
        sqlx::query_scalar("select updated_at from quota_learning_accounts")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(before, after, "duplicate snapshots must not rewrite state");
    database.close().await;
}

#[tokio::test]
async fn quota_learning_new_plans_and_window_durations_are_dynamic_and_isolated() {
    let Some(database) = TestDatabase::create("learning_new_plan").await else {
        return;
    };
    for id in ["acct_future", "acct_future_new", "acct_other"] {
        seed_account(&database.pool, id).await;
    }
    let store = admin_account_store(&database.pool);
    let reset = Utc::now() + Duration::days(6);
    let mut baseline = observation("acct_future", reset, 20.0, 0.0);
    baseline.plan_type = "  Future-Pro_2027  ".to_owned();
    store
        .record_quota_learning(&[baseline.clone()])
        .await
        .unwrap();
    let mut bound = baseline.clone();
    bound.observed_at = Utc::now();
    bound.used_percent = 25.0;
    bound.observed_cost_usd = Some(6.0);
    store.record_quota_learning(&[bound]).await.unwrap();

    let mut inherited = baseline.clone();
    inherited.account_id = "acct_future_new".to_owned();
    inherited.plan_type = "future-pro_2027".to_owned();
    inherited.observed_at = Utc::now();
    inherited.observed_cost_usd = None;
    let result = store
        .record_quota_learning(&[inherited.clone()])
        .await
        .unwrap();
    assert_eq!(result[0].effective_limit_usd, Some(120.0));
    assert_eq!(result[0].source, QuotaLearningSource::PlanAverage);
    assert_eq!(result[0].sample_count, 1);

    for (provider, plan, minutes) in [
        ("openai", "plus", 10_080),
        ("other-provider", "future-pro_2027", 10_080),
        ("openai", "future-pro_2027", 20_160),
    ] {
        let mut other = inherited.clone();
        other.account_id = "acct_other".to_owned();
        other.provider_kind = provider.to_owned();
        other.plan_type = plan.to_owned();
        other.window_minutes = minutes;
        other.observed_at = Utc::now();
        let result = store.record_quota_learning(&[other]).await.unwrap();
        assert_eq!(result[0].effective_limit_usd, None);
        assert_eq!(result[0].sample_count, 0);
    }
    for plan in ["", " ", " Unknown "] {
        inherited.plan_type = plan.to_owned();
        assert!(
            store
                .record_quota_learning(&[inherited.clone()])
                .await
                .unwrap()
                .is_empty()
        );
    }
    database.close().await;
}

#[tokio::test]
async fn quota_learning_normal_rollover_keeps_binding_and_refreshes_baseline() {
    let Some(database) = TestDatabase::create("learning_rollover").await else {
        return;
    };
    let id = "acct_rollover";
    seed_account(&database.pool, id).await;
    let store = admin_account_store(&database.pool);
    let now = Utc::now();
    let old_reset = now - Duration::hours(1);
    let mut baseline = observation(id, old_reset, 20.0, 0.0);
    baseline.observed_at = now - Duration::hours(3);
    store
        .record_quota_learning(&[baseline.clone()])
        .await
        .unwrap();
    baseline.used_percent = 25.0;
    baseline.observed_cost_usd = Some(5.0);
    baseline.observed_at = now - Duration::hours(2);
    store.record_quota_learning(&[baseline]).await.unwrap();
    let current = observation(id, old_reset + Duration::days(7), 0.0, 0.0);
    let result = store.record_quota_learning(&[current]).await.unwrap();
    assert_eq!(result[0].source, QuotaLearningSource::Personal);
    assert_eq!(result[0].effective_limit_usd, Some(100.0));
    assert_eq!(result[0].sample_count, 1);
    let (cost, percent): (Option<f64>, f64) =
        sqlx::query_as("select baseline_cost_usd, baseline_percent from quota_learning_accounts")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!((cost, percent), (Some(0.0), 0.0));
    database.close().await;
}

#[tokio::test]
async fn quota_learning_early_reset_relearns_all_windows_and_preserves_plan_samples() {
    let Some(database) = TestDatabase::create("learning_reset").await else {
        return;
    };
    let id = "acct_reset";
    seed_account(&database.pool, id).await;
    let store = admin_account_store(&database.pool);
    let reset = Utc::now() + Duration::days(6);
    let windows = |percent, cost| {
        let weekly = observation(id, reset, percent, cost);
        let mut short = weekly.clone();
        short.window_key = "primary".to_owned();
        short.window_minutes = 300;
        short.reset_at = Utc::now() + Duration::hours(4);
        [weekly, short]
    };
    store
        .record_quota_learning(&windows(20.0, 0.0))
        .await
        .unwrap();
    store
        .record_quota_learning(&windows(25.0, 5.0))
        .await
        .unwrap();
    let mut reset_observations = windows(0.0, 5.0);
    for item in &mut reset_observations {
        item.observed_cost_usd = None;
    }
    let result = store
        .record_quota_learning(&reset_observations)
        .await
        .unwrap();
    assert!(
        result
            .iter()
            .all(|item| item.source == QuotaLearningSource::PlanAverage)
    );
    assert!(
        result
            .iter()
            .all(|item| item.effective_limit_usd == Some(100.0))
    );
    let personal: i64 = sqlx::query_scalar(
        "select count(*) from quota_learning_accounts where bound_usd is not null",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(personal, 0);

    store
        .record_quota_learning(&windows(0.0, 5.0))
        .await
        .unwrap();
    let result = store
        .record_quota_learning(&windows(3.0, 11.0))
        .await
        .unwrap();
    assert!(
        result
            .iter()
            .all(|item| item.source == QuotaLearningSource::Personal)
    );
    assert!(
        result
            .iter()
            .all(|item| item.effective_limit_usd == Some(200.0))
    );
    assert!(result.iter().all(|item| item.sample_count == 1));
    database.close().await;
}

#[tokio::test]
async fn quota_learning_changed_plan_without_cost_does_not_reuse_old_personal_binding() {
    let Some(database) = TestDatabase::create("learning_plan_change").await else {
        return;
    };
    let id = "acct_plan_change";
    seed_account(&database.pool, id).await;
    let store = admin_account_store(&database.pool);
    let reset = Utc::now() + Duration::days(6);
    store
        .record_quota_learning(&[observation(id, reset, 20.0, 0.0)])
        .await
        .unwrap();
    let bound = observation(id, reset, 25.0, 5.0);
    store
        .record_quota_learning(std::slice::from_ref(&bound))
        .await
        .unwrap();
    let mut changed = observation_without_cost(id, reset, 25.0);
    changed.plan_type = "future-business".to_owned();
    let result = store
        .record_quota_learning(&[changed.clone()])
        .await
        .unwrap();
    assert_eq!(result[0].source, QuotaLearningSource::Learning);
    assert_eq!(result[0].effective_limit_usd, None);
    let mut late_other_window = bound.clone();
    late_other_window.window_key = "primary".to_owned();
    late_other_window.window_minutes = 300;
    let stale = store
        .record_quota_learning(&[bound, late_other_window.clone()])
        .await
        .unwrap();
    assert!(stale.is_empty());
    late_other_window.observed_at = changed.observed_at;
    assert!(
        store
            .record_quota_learning(&[late_other_window])
            .await
            .unwrap()
            .is_empty()
    );
    let plan: String = sqlx::query_scalar("select plan_type from quota_learning_accounts")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(plan, "future-business");
    let sample_plan: String =
        sqlx::query_scalar("select plan_type from quota_learning_plan_samples")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(sample_plan, "Plus");
    database.close().await;
}
