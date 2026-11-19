//! Official analytics regressions adapted to NewCPR's scalar observation schema.
use super::{TestDatabase, drain_analytics, observability_query_budget, observability_repository};
use chrono::{DateTime, TimeDelta, Utc};
use gateway_store::postgres::{
    DiagnosticDimension, ObservabilityRange, ObservabilityRepository, PgRequestAnalyticsRepository,
    UsageRecordFilter,
};

pub(super) async fn seed(pool: &sqlx::PgPool, id: &str, start: DateTime<Utc>, groups: &[&str]) {
    sqlx::query("insert into model_requests(id,client_api_key_ref,config_revision,protocol,operation,endpoint,client_transport,requested_model_id,outcome,client_status_code,downstream_committed_at,input_tokens,output_tokens,total_tokens,started_at,deadline_at,completed_at,routing_scope,routing_group_refs,routing_group_names_snapshot,first_token_ms,latency_ms) values($1,'key_analytics',1,'openai','responses','/v1/responses','http_sse','model_a','succeeded',200,$2+interval '1 minute',100,20,120,$2,$2+interval '2 minutes',$2+interval '1 minute',case when cardinality($3::text[])>0 then 'groups' else 'all' end,$3,to_jsonb($3::text[]),50,100)")
        .bind(id).bind(start).bind(groups).execute(pool).await.expect("seed analytic request");
}

#[tokio::test]
async fn projection_replay_late_ops_and_deletion_preserve_exact_contributions() {
    let Some(database) = TestDatabase::create("analytics_lifecycle").await else {
        return;
    };
    let start = DateTime::parse_from_rfc3339("2026-09-01T10:01:00Z")
        .unwrap()
        .with_timezone(&Utc);
    seed(&database.pool, "req_analytics_a", start, &["group_a"]).await;
    let repo = observability_repository(&database.pool);
    let range = ObservabilityRange::new(start - TimeDelta::minutes(1), start + TimeDelta::hours(2))
        .unwrap();
    assert!(
        repo.usage_summary(range, Default::default()).await.is_err(),
        "building must not masquerade as zero"
    );
    drain_analytics(&database.pool).await;
    let original = repo.usage_summary(range, Default::default()).await.unwrap();
    assert_eq!(original.requests.request_count, 1);
    assert_eq!(original.requests.total_tokens, 120);
    assert_eq!(original.attempts.failure_count, 0);
    sqlx::query("insert into ops_events(id,model_request_id,attempt_index,level,component,operation,failure_kind,status_code,message,created_at) values('ops_analytics_late','req_analytics_a',1,'warning','routing','responses','rate_limited',429,'late observation',$1)")
        .bind(start+TimeDelta::seconds(10)).execute(&database.pool).await.unwrap();
    drain_analytics(&database.pool).await;
    let with_ops = repo.usage_summary(range, Default::default()).await.unwrap();
    assert_eq!(with_ops.attempts.failure_count, 1);
    assert_eq!(with_ops.attempts.rate_limited_count, 1);
    sqlx::query("select enqueue_request_analytics('req_analytics_a')")
        .execute(&database.pool)
        .await
        .unwrap();
    drain_analytics(&database.pool).await;
    assert_eq!(
        repo.usage_summary(range, Default::default())
            .await
            .unwrap()
            .attempts,
        with_ops.attempts
    );
    sqlx::query("delete from model_requests where id='req_analytics_a'")
        .execute(&database.pool)
        .await
        .unwrap();
    let pending = repo.usage_summary(range, Default::default()).await.unwrap();
    assert_eq!(pending.requests.request_count, 1);
    assert_eq!(pending.attempts.failure_count, 1);
    assert_eq!(
        pending.analytics.state,
        gateway_admin::model::observability::AnalyticsState::Lagging
    );
    drain_analytics(&database.pool).await;
    let deleted = repo.usage_summary(range, Default::default()).await.unwrap();
    assert_eq!(deleted.requests.request_count, 0);
    assert_eq!(deleted.attempts.failure_count, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from request_analytics_15m")
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        0
    );
    database.close().await;
}

#[tokio::test]
async fn rolled_back_source_changes_and_late_commits_do_not_lose_queue_work() {
    let Some(database) = TestDatabase::create("analytics_transactions").await else {
        return;
    };
    let start = Utc::now() - TimeDelta::hours(1);
    seed(&database.pool, "req_analytics_tx", start, &[]).await;
    drain_analytics(&database.pool).await;
    let mut transaction = database.pool.begin().await.unwrap();
    sqlx::query("update model_requests set total_tokens=900 where id='req_analytics_tx'")
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    let worker =
        PgRequestAnalyticsRepository::new(database.pool.clone(), observability_query_budget());
    assert_eq!(worker.status().await.unwrap().pending_requests, 0);
    let mut transaction = database.pool.begin().await.unwrap();
    sqlx::query("update model_requests set total_tokens=240 where id='req_analytics_tx'")
        .execute(&mut *transaction)
        .await
        .unwrap();
    // 序列已分配但尚未提交，Worker 不得把最大版本误认为全部处理完成
    worker.maintain(100).await.unwrap();
    transaction.commit().await.unwrap();
    assert_eq!(worker.status().await.unwrap().pending_requests, 1);
    drain_analytics(&database.pool).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select total_tokens from request_analytics_facts where id='req_analytics_tx'"
        )
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        240
    );
    database.close().await;
}

#[tokio::test]
async fn worker_skips_locked_versions_and_preserves_a_concurrent_source_deletion() {
    use sqlx::Connection;
    for backfill in [false, true] {
        let Some(database) = TestDatabase::create("analytics_locked_ack").await else {
            return;
        };
        let start = Utc::now() - TimeDelta::hours(1);
        seed(&database.pool, "locked_ack", start, &[]).await;
        drain_analytics(&database.pool).await;
        sqlx::query("update model_requests set total_tokens=240 where id='locked_ack'")
            .execute(&database.pool)
            .await
            .unwrap();
        if backfill {
            sqlx::query("update request_analytics_build_state set backfill_complete=false,backfill_cursor_started_at=null,backfill_cursor_id=null")
                .execute(&database.pool).await.unwrap();
        }
        let mut source = sqlx::PgConnection::connect_with(
            &database
                .pool
                .connect_options()
                .as_ref()
                .clone()
                .options([("search_path", database.schema.as_str())]),
        )
        .await
        .unwrap();
        sqlx::raw_sql("begin;update model_requests set total_tokens=360 where id='locked_ack'")
            .execute(&mut source)
            .await
            .unwrap();
        let new_version: i64 = sqlx::query_scalar("select change_version from request_analytics_pending_requests where request_id='locked_ack'")
            .fetch_one(&mut source).await.unwrap();
        let worker =
            PgRequestAnalyticsRepository::new(database.pool.clone(), observability_query_budget());
        tokio::time::timeout(std::time::Duration::from_secs(2), worker.maintain(10))
            .await
            .expect("ACK must skip the source transaction's row lock")
            .unwrap();
        sqlx::raw_sql("delete from model_requests where id='locked_ack';commit")
            .execute(&mut source)
            .await
            .unwrap();
        let pending: i64 = sqlx::query_scalar("select change_version from request_analytics_pending_requests where request_id='locked_ack'")
            .fetch_one(&database.pool).await.expect("new source version cannot be acknowledged by old projection");
        assert!(pending > new_version);
        drain_analytics(&database.pool).await;
        assert_eq!(worker.status().await.unwrap().pending_requests, 0);
        let remaining: i64 = sqlx::query_scalar("select count(*) from request_analytics_facts")
            .fetch_one(&database.pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
        database.close().await;
    }
}

#[tokio::test]
async fn committed_queue_versions_skip_old_ack_and_replay_without_losing_contributions() {
    use sqlx::Connection;
    for backfill in [false, true] {
        let Some(database) = TestDatabase::create("analytics_committed_ack").await else {
            return;
        };
        let start = Utc::now() - TimeDelta::hours(1);
        seed(&database.pool, "committed_ack", start, &[]).await;
        drain_analytics(&database.pool).await;
        sqlx::query("update model_requests set total_tokens=240 where id='committed_ack'")
            .execute(&database.pool)
            .await
            .unwrap();
        if backfill {
            sqlx::query("update request_analytics_build_state set backfill_complete=false,backfill_cursor_started_at=null,backfill_cursor_id=null")
                .execute(&database.pool).await.unwrap();
        }
        let mut publication = sqlx::PgConnection::connect_with(
            &database
                .pool
                .connect_options()
                .as_ref()
                .clone()
                .options([("search_path", database.schema.as_str())]),
        )
        .await
        .unwrap();
        let publication_pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
            .fetch_one(&mut publication)
            .await
            .unwrap();
        // 只锁发布行、不修改版本，让 Worker 建立旧快照后停在 ACK 之前
        sqlx::raw_sql("begin;select singleton from request_analytics_build_state for update")
            .execute(&mut publication)
            .await
            .unwrap();
        let worker =
            PgRequestAnalyticsRepository::new(database.pool.clone(), observability_query_budget());
        let running = tokio::spawn(async move { worker.maintain(10).await });
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let waiting: bool = sqlx::query_scalar("select exists(select 1 from pg_stat_activity where wait_event_type='Lock' and $1=any(pg_blocking_pids(pid)) and query like 'update request_analytics_build_state%')")
                    .bind(publication_pid).fetch_one(&database.pool).await.unwrap();
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("worker reached publication with its old queue snapshot");
        // 源事务在 ACK 前已经提交，区别于 SKIP LOCKED 可以跳过的未提交版本
        sqlx::query("update model_requests set total_tokens=360 where id='committed_ack'")
            .execute(&database.pool)
            .await
            .unwrap();
        let committed_version: i64 = sqlx::query_scalar("select change_version from request_analytics_pending_requests where request_id='committed_ack'")
            .fetch_one(&database.pool).await.unwrap();
        sqlx::query("commit")
            .execute(&mut publication)
            .await
            .unwrap();
        // 已提交的新版本不能让整批投影回滚，旧版本 ACK 不匹配并保留给下一轮重放
        let maintenance = running
            .await
            .unwrap()
            .expect("committed queue versions cannot abort the projected batch");
        assert_eq!(maintenance.projected_requests, 1);
        let preserved: (i64, i64) = sqlx::query_as("select pending.change_version,facts.total_tokens from request_analytics_pending_requests pending join request_analytics_facts facts on facts.id=pending.request_id where pending.request_id='committed_ack'")
            .fetch_one(&database.pool).await.unwrap();
        assert_eq!(preserved, (committed_version, 240));
        drain_analytics(&database.pool).await;
        let summary = observability_repository(&database.pool)
            .usage_summary(
                ObservabilityRange::new(
                    start - TimeDelta::minutes(1),
                    start + TimeDelta::minutes(1),
                )
                .unwrap(),
                Default::default(),
            )
            .await
            .unwrap();
        assert_eq!(summary.requests.request_count, 1);
        assert_eq!(summary.requests.total_tokens, 360);
        database.close().await;
    }
}

#[tokio::test]
async fn source_deletions_do_not_block_worker_publication() {
    use sqlx::Connection;
    for backfill in [false, true] {
        let Some(database) = TestDatabase::create("analytics_delete_publication").await else {
            return;
        };
        let start = Utc::now() - TimeDelta::hours(1);
        seed(&database.pool, "delete_a", start, &[]).await;
        seed(&database.pool, "delete_b", start, &[]).await;
        drain_analytics(&database.pool).await;
        sqlx::query("update model_requests set total_tokens=240 where id='delete_b'")
            .execute(&database.pool)
            .await
            .unwrap();
        if backfill {
            sqlx::query("update request_analytics_build_state set backfill_complete=false,backfill_cursor_started_at=null,backfill_cursor_id=null")
                .execute(&database.pool).await.unwrap();
        }
        let mut source = sqlx::PgConnection::connect_with(
            &database
                .pool
                .connect_options()
                .as_ref()
                .clone()
                .options([("search_path", database.schema.as_str())]),
        )
        .await
        .unwrap();
        sqlx::raw_sql("begin;delete from model_requests where id='delete_a'")
            .execute(&mut source)
            .await
            .unwrap();
        let worker =
            PgRequestAnalyticsRepository::new(database.pool.clone(), observability_query_budget());
        // 源删除只登记待处理版本，不持有统计发布行，Worker 可以独立完成发布
        tokio::time::timeout(std::time::Duration::from_secs(2), worker.maintain(10))
            .await
            .expect("source deletion must not block analytics publication")
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sqlx::query("delete from model_requests where id='delete_b'").execute(&mut source),
        )
        .await
        .expect("source deletion cannot wait for worker ACK")
        .expect("source deletion cannot be a deadlock victim");
        sqlx::query("commit").execute(&mut source).await.unwrap();
        drain_analytics(&database.pool).await;
        let remaining: i64 = sqlx::query_scalar("select count(*) from request_analytics_facts")
            .fetch_one(&database.pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
        database.close().await;
    }
}

#[tokio::test]
async fn backfill_coverage_uses_contiguous_cursor_instead_of_old_corrected_facts() {
    let Some(database) = TestDatabase::create("analytics_partial_coverage").await else {
        return;
    };
    let now = DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap();
    for (id, age) in [
        ("coverage_old", 480),
        ("coverage_gap", 240),
        ("coverage_recent_b", 2),
        ("coverage_recent_a", 1),
    ] {
        seed(&database.pool, id, now - TimeDelta::hours(age), &[]).await;
    }
    // 清空待处理队列后只修正最早的请求，验证中间的时间段仍需回填
    sqlx::query("delete from request_analytics_pending_requests")
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("update model_requests set total_tokens=900 where id='coverage_old'")
        .execute(&database.pool)
        .await
        .unwrap();
    let worker =
        PgRequestAnalyticsRepository::new(database.pool.clone(), observability_query_budget());
    worker.maintain(2).await.unwrap();
    let status = worker.status().await.unwrap();
    assert!(status.building);
    assert_eq!(
        status.coverage_start,
        Some(now - TimeDelta::hours(2) + TimeDelta::microseconds(1))
    );
    let repo = observability_repository(&database.pool);
    let recent = repo
        .usage_summary(
            ObservabilityRange::new(now - TimeDelta::minutes(90), now).unwrap(),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(recent.requests.request_count, 1);
    assert_eq!(
        recent.analytics.state,
        gateway_admin::model::observability::AnalyticsState::Building
    );
    for range in [
        ObservabilityRange::new(now - TimeDelta::days(30), now - TimeDelta::days(15)).unwrap(),
        ObservabilityRange::new(now - TimeDelta::days(15), now).unwrap(),
    ] {
        assert!(
            repo.usage_summary(range, Default::default()).await.is_err(),
            "a corrected old fact cannot bridge an unprocessed historical gap"
        );
    }
    drain_analytics(&database.pool).await;
    let all = repo
        .usage_summary(
            ObservabilityRange::new(now - TimeDelta::days(30), now).unwrap(),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(all.requests.request_count, 4);
    assert_eq!(all.requests.total_tokens, 1260);
    database.close().await;
}

#[tokio::test]
async fn clean_complete_hours_do_not_wait_for_unused_quarter_storage() {
    let Some(database) = TestDatabase::create("analytics_unused_quarters").await else {
        return;
    };
    let start = DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    seed(
        &database.pool,
        "hour_only",
        start + TimeDelta::minutes(1),
        &[],
    )
    .await;
    drain_analytics(&database.pool).await;
    let range = ObservabilityRange::new(start, start + TimeDelta::hours(1)).unwrap();
    let mut blocker = database.pool.begin().await.unwrap();
    // 模拟另一个事务维护不需要的十五分钟表，完整小时查询不应等待该表的锁
    sqlx::query("lock table request_analytics_15m in access exclusive mode")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let repo = observability_repository(&database.pool);
    let summary = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        repo.usage_summary(range, Default::default()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(summary.requests.request_count, 1);
    let diagnostics = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        repo.usage_diagnostics(range, Default::default(), DiagnosticDimension::ApiKey, None),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(diagnostics.total_request_count, 1);
    assert_eq!(diagnostics.items[0].request_count, 1);
    blocker.rollback().await.unwrap();
    database.close().await;
}

#[tokio::test]
async fn daily_analytics_uses_deployment_timezone_through_dst_boundary() {
    let Some(database) = TestDatabase::create("analytics_timezone").await else {
        return;
    };
    let utc = |text: &str| {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    };
    seed(
        &database.pool,
        "before_midnight",
        utc("2026-11-02T04:30:00Z"),
        &[],
    )
    .await;
    seed(
        &database.pool,
        "after_midnight",
        utc("2026-11-02T05:30:00Z"),
        &[],
    )
    .await;
    drain_analytics(&database.pool).await;
    let repo =
        observability_repository(&database.pool).with_timezone("America/New_York".parse().unwrap());
    let range =
        ObservabilityRange::new(utc("2026-10-15T00:00:00Z"), utc("2026-11-20T00:00:00Z")).unwrap();
    let points = repo.usage_trend(range, Default::default()).await.unwrap();
    let active: Vec<_> = points
        .iter()
        .filter(|point| point.metrics.request_count > 0)
        .collect();
    assert_eq!(active.len(), 2);
    assert_eq!(active[0].bucket_start, utc("2026-11-01T04:00:00Z"));
    assert_eq!(active[1].bucket_start, utc("2026-11-02T05:00:00Z"));
    assert_eq!(
        active[1].bucket_start - active[0].bucket_start,
        TimeDelta::hours(25)
    );
    assert!(active.iter().all(|point| point.metrics.request_count == 1));
    database.close().await;
}

#[tokio::test]
async fn frozen_billing_projects_without_a_live_catalog_and_never_rewrites_ledgers() {
    use gateway_admin::{ProviderUsageBillingProjector, ports::provider::ProviderAdminRegistry};
    use std::sync::Arc;
    let Some(database) = TestDatabase::create("analytics_frozen_billing").await else {
        return;
    };
    let start = Utc::now() - TimeDelta::hours(1);
    seed(&database.pool, "frozen_bill", start, &[]).await;
    let saved = serde_json::json!({
        "version": 1, "currency": "USD", "input": "0.8", "output": "0.2", "cacheRead": "0", "cacheWrite": "0",
        "standard": "1", "total": "1.25", "inputPrice": "0", "outputPrice": "0", "cacheReadPrice": "0", "cacheWritePrice": "0", "serviceTier": null, "multiplierPercent": 125
    });
    sqlx::query("update model_requests set provider_kind='openai',provider_account_ref='billing_account',upstream_model_id='retired_model',cost_source='calculated',cost_currency='USD',cost_amount=1.25,billing_snapshot_json=$1 where id='frozen_bill'")
        .bind(saved).execute(&database.pool).await.unwrap();
    let ledger = || {
        sqlx::query_scalar::<_, String>(
            "select coalesce(jsonb_agg(to_jsonb(a) order by provider_account_ref,currency),'[]'::jsonb)::text from account_cumulative_costs a",
        )
    };
    let before = ledger().fetch_one(&database.pool).await.unwrap();
    let worker =
        PgRequestAnalyticsRepository::new(database.pool.clone(), observability_query_budget())
            .with_billing_projector(Arc::new(ProviderUsageBillingProjector::new(
                ProviderAdminRegistry::new([]).unwrap(),
            )));
    for _ in 0..10 {
        worker.maintain(100).await.unwrap();
        let status = worker.status().await.unwrap();
        if !status.building && status.pending_requests == 0 && status.pending_buckets == 0 {
            break;
        }
    }
    let amounts: (String,String) = sqlx::query_as("select standard_cost_amount::text,no_cache_cost_amount::text from request_analytics_facts where id='frozen_bill'").fetch_one(&database.pool).await.unwrap();
    assert_eq!(amounts, ("1.0000000000".into(), "1.2500000000".into()));
    sqlx::query("select enqueue_request_analytics('frozen_bill')")
        .execute(&database.pool)
        .await
        .unwrap();
    worker.maintain(100).await.unwrap();
    assert_eq!(ledger().fetch_one(&database.pool).await.unwrap(), before);
    database.close().await;
}

#[tokio::test]
async fn usage_summary_preserves_counts_latency_samples_and_costs_across_read_paths() {
    let Some(database) = TestDatabase::create("analytics_summary_projection").await else {
        return;
    };
    let start = DateTime::parse_from_rfc3339("2026-09-01T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    for (id, minute, latency, currency, amount, key, provider, outcome) in [
        (
            "summary_zero",
            5,
            Some(0_i64),
            Some("USD"),
            Some("0"),
            "key_analytics",
            "openai",
            "succeeded",
        ),
        (
            "summary_usd",
            20,
            Some(101),
            Some("USD"),
            Some("1.25"),
            "key_analytics",
            "openai",
            "succeeded",
        ),
        (
            "summary_cny",
            35,
            None,
            Some("CNY"),
            Some("2.50"),
            "key_analytics",
            "openai",
            "succeeded",
        ),
        (
            "summary_unknown",
            50,
            Some(303),
            None,
            None,
            "key_analytics",
            "openai",
            "succeeded",
        ),
        (
            "summary_cancelled",
            65,
            Some(999),
            Some("USD"),
            Some("777"),
            "key_analytics",
            "openai",
            "cancelled",
        ),
        (
            "summary_outside",
            130,
            Some(999),
            Some("USD"),
            Some("999"),
            "key_analytics",
            "openai",
            "succeeded",
        ),
        (
            "summary_other_key",
            5,
            Some(999),
            Some("USD"),
            Some("99"),
            "key_other",
            "openai",
            "succeeded",
        ),
        (
            "summary_other_provider",
            5,
            Some(999),
            Some("USD"),
            Some("99"),
            "key_analytics",
            "xai",
            "succeeded",
        ),
    ] {
        seed(&database.pool, id, start + TimeDelta::minutes(minute), &[]).await;
        sqlx::query("update model_requests set provider_kind=$2,provider_account_ref='account_summary',client_api_key_ref=$3,upstream_transport='http_sse',attempt_count=1,upstream_send_state='sent',outcome=$4,client_status_code=case when $4='cancelled' then 499 else 200 end,cost_source=case when $5::text is null then 'unavailable' else 'provider_reported' end,cost_currency=$5,cost_amount=$6::text::numeric,first_token_ms=null,latency_ms=$7 where id=$1")
            .bind(id).bind(provider).bind(key).bind(outcome).bind(currency).bind(amount).bind(latency)
            .execute(&database.pool).await.unwrap();
    }
    drain_analytics(&database.pool).await;
    let repo = observability_repository(&database.pool);
    let full = ObservabilityRange::new(start, start + TimeDelta::hours(2)).unwrap();
    let edges = ObservabilityRange::new(
        start + TimeDelta::minutes(4),
        start + TimeDelta::minutes(66),
    )
    .unwrap();
    let filter = UsageRecordFilter {
        provider_kind: Some("openai".into()),
        client_api_key_ref: Some("key_analytics".into()),
        ..Default::default()
    };
    let exact_filter = UsageRecordFilter {
        transport: Some("http_sse".into()),
        ..filter.clone()
    };
    for (range, filter, approximate) in [
        (full, filter.clone(), true),
        (edges, filter.clone(), true),
        (full, exact_filter.clone(), false),
    ] {
        let summary = repo.usage_summary(range, filter.clone()).await.unwrap();
        assert_eq!(
            (
                summary.requests.request_count,
                summary.requests.success_count,
                summary.requests.cancelled_count
            ),
            (5, 4, 1)
        );
        assert_eq!(
            (summary.requests.latency_sum, summary.requests.latency_count),
            (404, 3)
        );
        assert_eq!(summary.requests.total_tokens, 480);
        assert_eq!(summary.attempts.attempt_count, 5);
        assert_eq!(summary.attempts.cost_coverage.provider_reported_count, 3);
        assert_eq!(summary.attempts.cost_coverage.unavailable_count, 1);
        assert_eq!(
            summary
                .attempts
                .costs
                .iter()
                .map(|cost| (cost.currency.as_str(), cost.amount.as_str()))
                .collect::<Vec<_>>(),
            vec![("CNY", "2.5"), ("USD", "1.25")]
        );
        assert_eq!(summary.analytics.percentiles_approximate, approximate);
    }
    let empty = repo
        .usage_summary(
            full,
            UsageRecordFilter {
                client_api_key_ref: Some("key_missing".into()),
                ..filter.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(empty.requests, Default::default());
    assert!(empty.attempts.costs.is_empty());
    let other = repo
        .usage_summary(
            full,
            UsageRecordFilter {
                client_api_key_ref: Some("key_other".into()),
                ..filter
            },
        )
        .await
        .unwrap();
    assert_eq!(other.requests.request_count, 1);
    assert_eq!(other.attempts.costs[0].amount.as_str(), "99");

    // 事实摘要按币种合并时，零毫秒是一个样本，缺少延迟和空结果均不能补造样本
    for (minute, request_count, latency_count) in [(5, 1, 1), (35, 1, 0), (90, 0, 0)] {
        let range = ObservabilityRange::new(
            start + TimeDelta::minutes(minute),
            start + TimeDelta::minutes(minute + 1),
        )
        .unwrap();
        let summary = repo
            .usage_summary(range, exact_filter.clone())
            .await
            .unwrap();
        assert_eq!(summary.requests.request_count, request_count);
        assert_eq!(summary.requests.latency_count, latency_count);
        assert_eq!(summary.requests.latency_sum, 0);
    }

    database.close().await;
}
