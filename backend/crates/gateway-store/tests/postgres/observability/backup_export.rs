use super::*;
use gateway_admin::ports::backup::{DatabaseDumpPort, DumpRequest};
use gateway_core::lifecycle::CancellationToken;
use gateway_store::backup::{pg_dump::PgDumpAdapter, staging::StagingArea};
use std::process::Stdio;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[tokio::test]
async fn business_backup_restore_preserves_ledgers_and_global_usage_without_details() {
    let Some(db) = TestDatabase::create("business_backup_restore").await else {
        return;
    };
    let hour = chrono::DateTime::from_timestamp(Utc::now().timestamp().div_euclid(3600) * 3600, 0)
        .unwrap();
    seed_observability_facts(&db.pool, hour + TimeDelta::minutes(30))
        .await
        .unwrap();
    sqlx::query("insert into client_api_keys(id,name,key,created_at,updated_at) values('backup_key','Backup key',$1,now(),now())")
        .bind(format!("sk_{}", "b".repeat(43))).execute(&db.pool).await.unwrap();
    sqlx::query("insert into client_key_budget_windows(client_api_key_id,daily_start,daily_end,weekly_start,weekly_end,daily_used_usd,weekly_used_usd) values('backup_key',$1,$1+interval '1 day',$1,$1+interval '7 days',2.5,9.5)")
        .bind(hour).execute(&db.pool).await.unwrap();
    sqlx::query("insert into client_key_charge_events(request_id,client_api_key_id,amount_usd,completed_at) values('charge_before_backup','backup_key',2.5,now())")
        .execute(&db.pool).await.unwrap();
    let identity: String = sqlx::query_scalar(
        "select to_jsonb(a)::text from provider_accounts a where id='acct_observe'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let range =
        ObservabilityRange::new(hour - TimeDelta::hours(1), hour + TimeDelta::hours(1)).unwrap();
    let repo = observability_repository(&db.pool);
    let before = repo
        .usage_summary(range, UsageRecordFilter::default())
        .await
        .unwrap();
    let ledger: String = sqlx::query_scalar("select coalesce(jsonb_agg(to_jsonb(t) order by provider_account_ref,currency),'[]'::jsonb)::text from account_cumulative_costs t")
        .fetch_one(&db.pool).await.unwrap();
    let entries: i64 = sqlx::query_scalar("select count(*) from account_cumulative_cost_entries")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let mut url = url::Url::parse(&std::env::var("CPR_TEST_DATABASE_URL").unwrap()).unwrap();
    let password = url.password().unwrap().to_owned();
    url.set_password(None).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let staging = Arc::new(StagingArea::open(temp.path().to_path_buf(), 64 * 1024 * 1024).unwrap());
    let adapter = PgDumpAdapter::new(staging, url.to_string(), &password, db.pool.clone());
    let schema: String = sqlx::query_scalar("select current_schema()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    for (index, id) in ["first", "second"].into_iter().enumerate() {
        let artifact = adapter
            .dump(DumpRequest {
                backup_id: id.into(),
                cancellation: CancellationToken::new(),
            })
            .await
            .unwrap();
        let mut sql = Vec::new();
        async_compression::tokio::bufread::GzipDecoder::new(tokio::io::BufReader::new(
            tokio::fs::File::open(artifact.path).await.unwrap(),
        ))
        .read_to_end(&mut sql)
        .await
        .unwrap();
        let sql_text = String::from_utf8_lossy(&sql);
        assert!(sql_text.contains("backup_usage_hours"));
        assert!(!sql_text.contains("req_observe_uncommitted"));
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "drop schema \"{schema}\" cascade; create schema \"{schema}\""
        )))
        .execute(&db.pool)
        .await
        .unwrap();
        let mut process = tokio::process::Command::new("psql")
            .args([
                "-X",
                "--single-transaction",
                "--set=ON_ERROR_STOP=1",
                "--dbname",
                url.as_str(),
            ])
            .env("PGPASSWORD", &password)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        process.stdin.take().unwrap().write_all(&sql).await.unwrap();
        let output = process.wait_with_output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let restored = repo
            .usage_summary(range, UsageRecordFilter::default())
            .await
            .unwrap();
        assert_eq!(
            restored.requests.request_count,
            before.requests.request_count + index as u64
        );
        assert_eq!(
            restored.requests.success_count,
            before.requests.success_count
        );
        assert_eq!(
            restored.requests.failure_count,
            before.requests.failure_count + index as u64
        );
        assert_eq!(restored.requests.total_tokens, before.requests.total_tokens);
        assert_eq!(restored.attempts.costs, before.attempts.costs);
        let dashboard = admin_observability_store(&db.pool)
            .dashboard_summary(
                admin_observability::TimeRange::new(range.start, range.end).unwrap(),
                hour,
            )
            .await
            .unwrap();
        assert_eq!(
            dashboard.totals.request_count,
            restored.requests.request_count
        );
        assert_eq!(
            dashboard.totals.total_tokens,
            restored.requests.total_tokens
        );
        assert_eq!(
            dashboard
                .trend
                .iter()
                .map(|point| point.metrics.request_count)
                .sum::<u64>(),
            restored.requests.request_count
        );
        assert_eq!(
            dashboard
                .trend
                .iter()
                .map(|point| point.metrics.total_tokens)
                .sum::<u64>(),
            restored.requests.total_tokens
        );
        assert!(dashboard.recent_requests.is_empty());
        let details: i64 = sqlx::query_scalar("select count(*) from model_requests")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(details, 0);
        let actual: String = sqlx::query_scalar("select coalesce(jsonb_agg(to_jsonb(t) order by provider_account_ref,currency),'[]'::jsonb)::text from account_cumulative_costs t")
            .fetch_one(&db.pool).await.unwrap();
        assert_eq!(actual, ledger);
        let actual_entries: i64 =
            sqlx::query_scalar("select count(*) from account_cumulative_cost_entries")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(actual_entries, entries);
        let restored_identity: String = sqlx::query_scalar(
            "select to_jsonb(a)::text from provider_accounts a where id='acct_observe'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(restored_identity, identity);
        let budgets: (String,String,i64) = sqlx::query_as("select daily_used_usd::text,weekly_used_usd::text,(select count(*) from client_key_charge_events) from client_key_budget_windows where client_api_key_id='backup_key'")
            .fetch_one(&db.pool).await.unwrap();
        assert_eq!(budgets, ("2.5000000000".into(), "9.5000000000".into(), 1));
        let filtered = repo
            .usage_summary(
                range,
                UsageRecordFilter {
                    provider_account_ref: Some("acct_observe".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(filtered.requests.request_count, 0);
        assert!(filtered.attempts.costs.is_empty());
        if index == 0 {
            seed_observability_facts_after_restore(&db.pool, hour).await;
        }
    }
    let after = repo
        .usage_summary(range, UsageRecordFilter::default())
        .await
        .unwrap();
    assert_eq!(
        after.requests.request_count,
        before.requests.request_count + 1
    );
    db.close().await;
}

async fn seed_observability_facts_after_restore(pool: &PgPool, hour: chrono::DateTime<Utc>) {
    sqlx::query("insert into model_requests(id,client_api_key_ref,config_revision,protocol,operation,endpoint,client_transport,requested_model_id,attempt_count,upstream_send_state,outcome,client_status_code,started_at,deadline_at,completed_at,routing_scope) values('req_after_restore','key_observe',1,'openai','responses','/v1/responses','http_sse','public-model',0,'not_sent','failed',502,$1,$1+interval '1 minute',$1,'all')")
        .bind(hour+TimeDelta::minutes(45)).execute(pool).await.unwrap();
}
