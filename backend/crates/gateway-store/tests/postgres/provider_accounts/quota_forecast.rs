use super::*;
use gateway_admin::model::quota_forecast_sampling::MAX_FORECAST_HISTORY_POINTS;
use gateway_store::postgres::{
    DiagnosticDimension, ObservabilityPageSize, ObservabilityRange, ObservabilityRepository,
    ProviderAccountUsageQuery, UsageRecordFilter, UsageRecordQuery,
};

#[tokio::test]
async fn quota_forecast_excludes_openai_prewarm_but_preserves_inference_and_audit() {
    let Some(database) = TestDatabase::create("forecast_prewarm").await else {
        return;
    };
    PgProviderAccountRepository::new(database.pool.clone())
        .insert_provider_account(account("acct_prewarm", "user-prewarm"))
        .await
        .unwrap();
    let start = Utc::now() - TimeDelta::minutes(30);
    let end = start + TimeDelta::minutes(20);
    for (id, minute, tokens) in [("normal", 1, 100), ("review", 2, 200)] {
        // 普通成功推理也可以只有输入 Token，不能用 output_tokens = 0 判定预热。
        seed_model_request(
            &database.pool,
            ModelRequestSeed {
                request_id: id,
                account_id: "acct_prewarm",
                provider_kind: "openai",
                model: "gpt-test",
                total_tokens: tokens,
                cost_amount: "1",
                started_at: start + TimeDelta::minutes(minute),
            },
        )
        .await
        .unwrap();
    }
    sqlx::query("update model_requests set request_kind = 'review' where id = 'review'")
        .execute(&database.pool)
        .await
        .unwrap();
    // The first insert must already have unknown cost; a temporary known fee
    // would be permanently accumulated by the immutable accounting trigger.
    sqlx::query(
        "insert into model_requests
         select (jsonb_populate_record(null::model_requests, to_jsonb(mr) ||
           jsonb_build_object(
             'id', 'prewarm', 'request_kind', 'prewarm', 'client_transport', 'websocket',
             'client_status_code', null, 'upstream_transport', 'websocket',
             'service_tier', 'auto', 'cost_source', 'unavailable',
             'cost_amount', null, 'cost_currency', null,
             'input_tokens', 400, 'total_tokens', 400,
             'provider_observation_json', '{\"upstreamServiceTier\":\"auto\"}'::jsonb,
             'started_at', $1::timestamptz, 'deadline_at', $1 + interval '5 minutes',
             'completed_at', $1 + interval '1 second',
             'downstream_committed_at', $1 + interval '1 second'
           ))).*
         from model_requests mr where mr.id = 'normal'",
    )
    .bind(start + TimeDelta::minutes(3))
    .execute(&database.pool)
    .await
    .unwrap();
    let store = admin_account_store(&database.pool);
    let query = AccountUsageWindowQuery {
        account_id: "acct_prewarm".to_owned(),
        key: "week".to_owned(),
        range: TimeRange { start, end },
    };
    let history = store.load_quota_forecast_history(&query).await.unwrap();
    assert_eq!(history.usage.request_count, 2);
    assert_eq!(history.usage.tokens, 300);
    assert_eq!(history.usage.missing_token_count, 0);
    assert_eq!(history.usage.known_cost_count, 2);
    assert_eq!(history.usage.unavailable_cost_count, 0);
    assert_eq!(history.usage.usd, 2.0);
    assert_eq!(history.usage.excluded_request_count, 1);
    assert_eq!(history.pending_request_count, 0);
    // 预热响应携带的额度观测仍可配对，但它不增加累计推理用量。
    assert_eq!(history.points.len(), 1);
    assert_eq!(history.points[0].usage, history.usage);

    let repository = super::super::observability_repository(&database.pool);
    let range = ObservabilityRange::new(start, end).unwrap();
    let account_usage = repository
        .provider_account_usage(
            ProviderAccountUsageQuery::for_accounts(range, vec!["acct_prewarm".to_owned()])
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(account_usage[0].request_count, 2);
    assert_eq!(account_usage[0].cost_coverage.unavailable_count, 0);
    let records = repository
        .list_usage_records(UsageRecordQuery {
            range,
            filter: UsageRecordFilter::default(),
            current_page: 1,
            page_size: ObservabilityPageSize::new(10).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(records.total, 2);
    assert!(records.items.iter().all(|record| record.id != "prewarm"));
    let detail = repository.usage_record_detail("prewarm").await.unwrap();
    assert_eq!(detail.request.request_kind.as_deref(), Some("prewarm"));
    assert_eq!(detail.request.service_tier.as_deref(), Some("auto"));
    assert!(detail.request.cost_amount.is_none());
    let cumulative = store
        .load_account_cumulative_costs(&["acct_prewarm".to_owned()])
        .await
        .unwrap();
    assert_eq!(cumulative["acct_prewarm"][0].amount.as_str(), "2");
    let dashboard = repository.dashboard_summary(range, end).await.unwrap();
    assert_eq!(dashboard.totals.request_count, 3);
    assert_eq!(dashboard.totals.total_tokens, 300);

    for (id, outcome, minute) in [
        ("prewarm_failed", "failed", 6),
        ("prewarm_incomplete", "incomplete", 11),
    ] {
        sqlx::query(
            "insert into model_requests
             select (jsonb_populate_record(null::model_requests, to_jsonb(mr) ||
               jsonb_build_object(
                 'id', $1::text, 'outcome', $2::text,
                 'started_at', $3::timestamptz, 'deadline_at', $3 + interval '5 minutes',
                 'completed_at', $3 + interval '1 second',
                 'downstream_committed_at', $3 + interval '1 second',
                 'provider_observation_json', null
               ))).*
             from model_requests mr where mr.id = 'prewarm'",
        )
        .bind(id)
        .bind(outcome)
        .bind(start + TimeDelta::minutes(minute))
        .execute(&database.pool)
        .await
        .unwrap();
    }
    let health = store
        .load_account_health_timeline(TimeRange { start, end }, &["acct_prewarm".to_owned()])
        .await
        .unwrap();
    let buckets = &health[0].request_buckets;
    assert_eq!(buckets.len(), 4);
    for (bucket, counts) in
        buckets
            .iter()
            .zip([(3, 3, 0, 0), (1, 0, 1, 0), (1, 0, 0, 1), (0, 0, 0, 0)])
    {
        assert_eq!(
            (
                bucket.request_count,
                bucket.success_count,
                bucket.error_count,
                bucket.non_completion_count,
            ),
            counts,
        );
    }
    let diagnostics = repository
        .usage_diagnostics(
            range,
            UsageRecordFilter {
                provider_account_ref: Some("acct_prewarm".to_owned()),
                ..UsageRecordFilter::default()
            },
            DiagnosticDimension::Account,
        )
        .await
        .unwrap();
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.request_count, 5);
    assert_eq!(diagnostic.success_count, 3);
    assert_eq!(diagnostic.failure_count, 1);
    assert_eq!(diagnostic.non_completion_count, 1);
    assert_eq!(diagnostic.total_tokens, 300);
    assert_eq!(diagnostic.cost_coverage.unavailable_count, 0);
    assert_eq!(
        buckets
            .iter()
            .map(|bucket| bucket.request_count)
            .sum::<u64>(),
        diagnostic.request_count,
    );
    assert_eq!(
        buckets.iter().map(|bucket| bucket.error_count).sum::<u64>(),
        diagnostic.failure_count,
    );
    let history = store.load_quota_forecast_history(&query).await.unwrap();
    assert_eq!(history.usage.request_count, 2);
    assert_eq!(history.usage.excluded_request_count, 3);

    // 真正推理的未知费用仍必须阻止费用外推，不能靠放宽覆盖门槛掩盖缺失。
    sqlx::query(
        "insert into model_requests
         select (jsonb_populate_record(null::model_requests, to_jsonb(mr) ||
           jsonb_build_object(
             'id', 'review_unknown', 'request_kind', 'review',
             'input_tokens', 200, 'total_tokens', 200,
             'provider_observation_json', null
           ))).*
         from model_requests mr where mr.id = 'prewarm'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let history = store.load_quota_forecast_history(&query).await.unwrap();
    assert_eq!(history.usage.request_count, 3);
    assert_eq!(history.usage.known_cost_count, 2);
    assert_eq!(history.usage.unavailable_cost_count, 1);
    assert_eq!(history.usage.usd, 2.0);
    assert_eq!(history.usage.excluded_request_count, 3);
    database.close().await;
}

#[tokio::test]
async fn quota_forecast_does_not_apply_openai_prewarm_semantics_to_other_providers() {
    let Some(database) = TestDatabase::create("forecast_xai_kind").await else {
        return;
    };
    let mut candidate = account("acct_xai_kind", "user-xai-kind");
    candidate.provider_kind = "xai".to_owned();
    PgProviderAccountRepository::new(database.pool.clone())
        .insert_provider_account(candidate)
        .await
        .unwrap();
    let start = Utc::now() - TimeDelta::minutes(30);
    seed_model_request(
        &database.pool,
        ModelRequestSeed {
            request_id: "xai_kind",
            account_id: "acct_xai_kind",
            provider_kind: "xai",
            model: "grok-test",
            total_tokens: 100,
            cost_amount: "1",
            started_at: start,
        },
    )
    .await
    .unwrap();
    sqlx::query("update model_requests set request_kind = 'prewarm' where id = 'xai_kind'")
        .execute(&database.pool)
        .await
        .unwrap();
    let history = admin_account_store(&database.pool)
        .load_quota_forecast_history(&AccountUsageWindowQuery {
            account_id: "acct_xai_kind".to_owned(),
            key: "week".to_owned(),
            range: TimeRange {
                start,
                end: start + TimeDelta::minutes(10),
            },
        })
        .await
        .unwrap();
    assert_eq!(history.usage.request_count, 1);
    assert_eq!(history.usage.tokens, 100);
    assert_eq!(history.usage.known_cost_count, 1);
    assert_eq!(history.usage.usd, 1.0);
    assert_eq!(history.usage.excluded_request_count, 0);
    database.close().await;
}

#[tokio::test]
async fn quota_forecast_pairs_completed_usage_and_preserves_missing_pending_and_excluded_counts() {
    let Some(database) = TestDatabase::create("quota_forecast_pairing").await else {
        return;
    };
    PgProviderAccountRepository::new(database.pool.clone())
        .insert_provider_account(account("acct_forecast", "user-forecast"))
        .await
        .unwrap();
    let start = "2026-09-12T00:00:00Z"
        .parse::<chrono::DateTime<Utc>>()
        .unwrap();
    let end = start + TimeDelta::minutes(30);
    for (id, minute, tokens) in [
        ("before", -1, 999),
        ("start", 0, 100),
        ("tie_a", 5, 200),
        ("tie_b", 5, 300),
        ("missing", 10, 400),
        ("eur", 15, 500),
        ("late", 20, 600),
        ("cancelled", 21, 0),
        ("websocket", 22, 700),
        ("http", 23, 800),
        ("after", 30, 999),
    ] {
        seed_model_request(
            &database.pool,
            ModelRequestSeed {
                request_id: id,
                account_id: "acct_forecast",
                provider_kind: "openai",
                model: "gpt-test",
                total_tokens: tokens,
                cost_amount: "1",
                started_at: start + TimeDelta::minutes(minute),
            },
        )
        .await
        .unwrap();
    }
    sqlx::query(
        "update model_requests set provider_observation_json =
         '{\"privateMarker\":\"never-debug-this\",\"rateLimitHeaders\":[]}'::jsonb",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "update model_requests set total_tokens = null, input_tokens = null,
          output_tokens = null, cost_source = 'unavailable', cost_amount = null, cost_currency = null
         where id = 'missing'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query("update model_requests set cost_currency = 'EUR' where id = 'eur'")
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("update model_requests set completed_at = $1 where id = 'late'")
        .bind(end + TimeDelta::seconds(1))
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("update model_requests set outcome = 'cancelled' where id = 'cancelled'")
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query(
        "update model_requests set client_status_code = null,
          client_transport = case when id = 'websocket' then 'websocket' else 'http_sse' end
         where id in ('websocket', 'http')",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let store = admin_account_store(&database.pool);
    let history = store
        .load_quota_forecast_history(&AccountUsageWindowQuery {
            account_id: "acct_forecast".to_owned(),
            key: "week".to_owned(),
            range: TimeRange { start, end },
        })
        .await
        .expect("forecast history");
    assert_eq!(history.usage.request_count, 6);
    assert_eq!(history.usage.tokens, 1_800);
    assert_eq!(history.usage.missing_token_count, 1);
    assert_eq!(history.usage.known_cost_count, 4);
    assert_eq!(history.usage.unavailable_cost_count, 2);
    assert_eq!(history.usage.usd, 4.0);
    assert_eq!(history.usage.excluded_request_count, 2);
    assert_eq!(history.pending_request_count, 1);
    assert!(history.points.iter().all(|point| point.completed_at <= end));
    let tie = history
        .points
        .iter()
        .find(|point| point.completed_at == start + TimeDelta::minutes(5) + TimeDelta::seconds(1))
        .unwrap();
    assert_eq!(tie.usage.tokens, 600);
    assert_eq!(tie.usage.request_count, 3);
    assert!(!format!("{history:?}").contains("never-debug-this"));
    let empty = store
        .load_quota_forecast_history(&AccountUsageWindowQuery {
            account_id: "acct_forecast".to_owned(),
            key: "empty".to_owned(),
            range: TimeRange {
                start: end + TimeDelta::hours(1),
                end: end + TimeDelta::hours(2),
            },
        })
        .await
        .unwrap();
    assert_eq!(empty.usage.request_count, 0);
    assert!(empty.points.is_empty());
    database.close().await;
}

#[tokio::test]
async fn quota_forecast_bounds_documents_without_sampling_away_token_totals() {
    let Some(database) = TestDatabase::create("quota_forecast_bound").await else {
        return;
    };
    PgProviderAccountRepository::new(database.pool.clone())
        .insert_provider_account(account("acct_forecast_bound", "user-forecast-bound"))
        .await
        .unwrap();
    let start = Utc::now() - TimeDelta::hours(6);
    for index in 0..256 {
        seed_model_request(
            &database.pool,
            ModelRequestSeed {
                request_id: &format!("forecast_{index}"),
                account_id: "acct_forecast_bound",
                provider_kind: "openai",
                model: "gpt-test",
                total_tokens: 100,
                cost_amount: "0",
                started_at: start + TimeDelta::minutes(index),
            },
        )
        .await
        .unwrap();
    }
    sqlx::query("update model_requests set provider_observation_json = '{}'::jsonb")
        .execute(&database.pool)
        .await
        .unwrap();
    let history = admin_account_store(&database.pool)
        .load_quota_forecast_history(&AccountUsageWindowQuery {
            account_id: "acct_forecast_bound".to_owned(),
            key: "week".to_owned(),
            range: TimeRange {
                start,
                end: start + TimeDelta::minutes(256),
            },
        })
        .await
        .unwrap();
    assert_eq!(history.usage.request_count, 256);
    assert_eq!(history.usage.tokens, 25_600);
    assert_eq!(history.points.len(), MAX_FORECAST_HISTORY_POINTS);
    assert_eq!(history.points.last().unwrap().usage.tokens, 25_600);
    database.close().await;
}
