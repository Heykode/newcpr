use super::*;
use gateway_store::postgres::{
    PgRetentionRepository, RetentionRepository, RuntimeRetentionSettings,
};

async fn seed_cost(pool: &sqlx::PgPool, id: &str, account_id: &str, amount: &str) {
    seed_model_request(
        pool,
        ModelRequestSeed {
            request_id: id,
            account_id,
            provider_kind: "openai",
            model: "gpt-test",
            total_tokens: 10,
            cost_amount: amount,
            started_at: Utc::now() - TimeDelta::days(100),
        },
    )
    .await
    .expect("seed completed cost");
}

async fn usd_total(pool: &sqlx::PgPool, account_id: &str) -> Option<String> {
    admin_account_store(pool)
        .load_account_cumulative_costs(&[account_id.to_owned()])
        .await
        .expect("load cumulative costs")
        .get(account_id)
        .and_then(|costs| costs.iter().find(|cost| cost.currency == "USD"))
        .map(|cost| cost.amount.to_string())
}

#[tokio::test]
async fn cumulative_cost_survives_reimport_quota_reset_and_log_retention() {
    let Some(db) = TestDatabase::create("cumulative_reimport").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(db.pool.clone());
    repository
        .insert_provider_account(account("acct_cumulative", "user-cumulative"))
        .await
        .expect("create original account");
    seed_cost(&db.pool, "request_old", "acct_cumulative", "100").await;
    let imported = repository
        .import_provider_accounts(ImportProviderAccounts {
            settings: None,
            outbound_proxy: None,
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".to_owned(),
            },
            accounts: vec![account("acct_candidate", "user-cumulative")],
            audit: audit("audit_cumulative", "import", "acct_cumulative"),
        })
        .await
        .expect("replace credentials for same identity");
    assert_eq!(imported.account_ids, ["acct_cumulative"]);
    assert_eq!(
        usd_total(&db.pool, "acct_cumulative").await.as_deref(),
        Some("100")
    );
    seed_cost(&db.pool, "request_new", "acct_cumulative", "5").await;
    assert_eq!(
        usd_total(&db.pool, "acct_cumulative").await.as_deref(),
        Some("105")
    );

    sqlx::query(
        "update provider_accounts set provider_quota_json = null,
         quota_reset_at = null, quota_observed_at = null where id = 'acct_cumulative'",
    )
    .execute(&db.pool)
    .await
    .expect("clear/reset quota independently");
    let report = PgRetentionRepository::new(db.pool.clone())
        .apply_retention(
            Utc::now(),
            RuntimeRetentionSettings {
                usage_retention_days: 31,
                ops_event_retention_days: 30,
                audit_retention_days: 90,
            },
        )
        .await
        .expect("prune expired request logs");
    assert_eq!(report.model_requests, 2);
    assert_eq!(
        usd_total(&db.pool, "acct_cumulative").await.as_deref(),
        Some("105")
    );
    assert_eq!(usd_total(&db.pool, "acct_candidate").await, None);

    // Even a replay of a pruned request must not create another charge.
    seed_cost(&db.pool, "request_old", "acct_cumulative", "100").await;
    assert_eq!(
        usd_total(&db.pool, "acct_cumulative").await.as_deref(),
        Some("105")
    );
    db.close().await;
}

#[tokio::test]
async fn cumulative_cost_is_exact_under_concurrency_retries_and_rollback() {
    let Some(db) = TestDatabase::create("cumulative_concurrent").await else {
        return;
    };
    PgProviderAccountRepository::new(db.pool.clone())
        .insert_provider_account(account("acct_concurrent", "user-concurrent"))
        .await
        .expect("create account");
    let requests = (0..32).map(|index| {
        let pool = db.pool.clone();
        async move {
            seed_cost(
                &pool,
                &format!("request_{index}"),
                "acct_concurrent",
                "0.0000000001",
            )
            .await;
        }
    });
    futures::future::join_all(requests).await;
    assert_eq!(
        usd_total(&db.pool, "acct_concurrent").await.as_deref(),
        Some("0.0000000032")
    );
    sqlx::query("update model_requests set outcome = 'succeeded', client_status_code = 200")
        .execute(&db.pool)
        .await
        .expect("duplicate completion observation");
    assert_eq!(
        usd_total(&db.pool, "acct_concurrent").await.as_deref(),
        Some("0.0000000032")
    );

    let mut transaction = db.pool.begin().await.expect("start rollback check");
    sqlx::query(
        "insert into model_requests
         select (jsonb_populate_record(null::model_requests,
             to_jsonb(mr) || '{\"id\":\"request_rolled_back\"}'::jsonb)).*
         from model_requests mr where id = 'request_0'",
    )
    .execute(&mut *transaction)
    .await
    .expect("write charge within transaction");
    transaction.rollback().await.expect("rollback charge");
    assert_eq!(
        usd_total(&db.pool, "acct_concurrent").await.as_deref(),
        Some("0.0000000032")
    );
    let entries: i64 = sqlx::query_scalar("select count(*) from account_cumulative_cost_entries")
        .fetch_one(&db.pool)
        .await
        .expect("count charge entries");
    assert_eq!(entries, 32);
    db.close().await;
}

#[tokio::test]
async fn cumulative_cost_can_exceed_the_single_request_amount_limit() {
    let Some(db) = TestDatabase::create("cumulative_large_amount").await else {
        return;
    };
    PgProviderAccountRepository::new(db.pool.clone())
        .insert_provider_account(account("acct_large", "user-large"))
        .await
        .expect("create account");
    seed_cost(&db.pool, "large", "acct_large", "9999999999.9999999999").await;
    seed_cost(&db.pool, "small", "acct_large", "0.0000000001").await;
    assert_eq!(
        usd_total(&db.pool, "acct_large").await.as_deref(),
        Some("10000000000")
    );
    let count: i64 =
        sqlx::query_scalar("select count(*) from model_requests where outcome = 'succeeded'")
            .fetch_one(&db.pool)
            .await
            .expect("normal terminal observations remain written");
    assert_eq!(count, 2);
    db.close().await;
}

#[tokio::test]
async fn cumulative_cost_counts_late_http_status_and_statusless_websocket_only_once() {
    let Some(db) = TestDatabase::create("cumulative_delivery").await else {
        return;
    };
    PgProviderAccountRepository::new(db.pool.clone())
        .insert_provider_account(account("acct_delivery", "user-delivery"))
        .await
        .expect("create account");
    // Insert the fixture with its final delivery facts in the same statement so
    // rejected cases never briefly become successful billable observations.
    for (id, transport, status, outcome, committed, cost) in [
        ("http_late", "http_sse", None, "succeeded", true, Some("2")),
        ("ws", "websocket", None, "succeeded", true, Some("3")),
        (
            "http_error",
            "http_sse",
            Some(502),
            "succeeded",
            true,
            Some("7"),
        ),
        ("failed", "http_sse", Some(200), "failed", true, Some("11")),
        (
            "uncommitted",
            "http_sse",
            Some(200),
            "succeeded",
            false,
            Some("13"),
        ),
        ("unknown", "http_sse", Some(200), "succeeded", true, None),
    ] {
        sqlx::query(
            "insert into model_requests (
               id, client_api_key_ref, config_revision, protocol, operation, endpoint,
               client_transport, requested_model_id, provider_kind, provider_account_id,
               provider_account_ref, upstream_transport, attempt_count, upstream_send_state, downstream_committed_at,
               outcome, client_status_code, cost_source, cost_amount, cost_currency,
               started_at, deadline_at, completed_at, routing_scope
             ) values (
               $1, 'key-test', 1, 'openai', 'responses', '/v1/responses',
               $2, 'gpt-test', 'openai', 'acct_delivery', 'acct_delivery', 'http_sse', 1, 'sent',
               case when $5 then now() else null end, $4, $3,
               case when $6::text is null then 'unavailable' else 'calculated' end,
               $6::numeric, case when $6::text is null then null else 'USD' end,
               now(), now() + interval '5 minutes', now(), 'all'
             )",
        )
        .bind(id)
        .bind(transport)
        .bind(status)
        .bind(outcome)
        .bind(committed)
        .bind(cost)
        .execute(&db.pool)
        .await
        .expect("seed delivery observation");
    }
    assert_eq!(
        usd_total(&db.pool, "acct_delivery").await.as_deref(),
        Some("3")
    );
    sqlx::query("update model_requests set client_status_code = 200 where id = 'http_late'")
        .execute(&db.pool)
        .await
        .expect("late HTTP status");
    assert_eq!(
        usd_total(&db.pool, "acct_delivery").await.as_deref(),
        Some("5")
    );
    sqlx::query("update model_requests set client_status_code = 200 where id = 'http_late'")
        .execute(&db.pool)
        .await
        .expect("duplicate HTTP status");
    assert_eq!(
        usd_total(&db.pool, "acct_delivery").await.as_deref(),
        Some("5")
    );
    let usage = admin_account_store(&db.pool)
        .load_account_usage(
            TimeRange {
                start: Utc::now() - TimeDelta::hours(1),
                end: Utc::now() + TimeDelta::hours(1),
            },
            &["acct_delivery".to_owned()],
        )
        .await
        .expect("existing usage predicate");
    assert_eq!(usage[0].costs[0].amount.as_str(), "5");
    db.close().await;
}

#[tokio::test]
async fn cumulative_cost_preserves_prewarm_fees_without_counting_them_as_inference() {
    let Some(db) = TestDatabase::create("cumulative_prewarm").await else {
        return;
    };
    PgProviderAccountRepository::new(db.pool.clone())
        .insert_provider_account(account("acct_prewarm_cost", "user-prewarm-cost"))
        .await
        .expect("create account");
    let now = Utc::now();
    let start = now - TimeDelta::days(100);
    for (id, kind, amount) in [
        ("inference", None, Some("2")),
        ("prewarm_known", Some("prewarm"), Some("3")),
        ("prewarm_unknown", Some("prewarm"), None),
    ] {
        sqlx::query(
            "insert into model_requests (
               id, client_api_key_ref, config_revision, protocol, operation, endpoint,
               client_transport, requested_model_id, provider_kind, provider_account_id,
               provider_account_ref, upstream_model_id, upstream_transport,
               request_kind, attempt_count, upstream_send_state, downstream_committed_at,
               outcome, client_status_code, input_tokens, output_tokens, total_tokens,
               cost_source, cost_amount, cost_currency, started_at, deadline_at,
               completed_at, routing_scope
             ) values (
               $1, 'key-test', 1, 'openai', 'responses', '/v1/responses',
               'websocket', 'gpt-test', 'openai', 'acct_prewarm_cost',
               'acct_prewarm_cost', 'gpt-test', 'websocket',
               $2, 1, 'sent', $4::timestamptz + interval '1 second', 'succeeded', null, 10, 0, 10,
               case when $3::text is null then 'unavailable' else 'provider_reported' end,
               $3::numeric, case when $3::text is null then null else 'USD' end,
               $4, $4 + interval '5 minutes', $4 + interval '1 second', 'all'
             )",
        )
        .bind(id)
        .bind(kind)
        .bind(amount)
        .bind(start)
        .execute(&db.pool)
        .await
        .expect("insert final classification and cost atomically");
    }

    let store = admin_account_store(&db.pool);
    let window = AccountUsageWindowQuery {
        account_id: "acct_prewarm_cost".to_owned(),
        key: "week".to_owned(),
        range: TimeRange {
            start,
            end: start + TimeDelta::hours(1),
        },
    };
    let usage = store
        .load_account_usage_by_windows(std::slice::from_ref(&window))
        .await
        .expect("load inference-only quota window");
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].usage.request_count, 1);
    assert_eq!(usage[0].usage.total_tokens, Some(10));
    assert_eq!(usage[0].usage.costs[0].amount.as_str(), "2");
    let forecast = store
        .load_quota_forecast_history(&window)
        .await
        .expect("load inference-only forecast");
    assert_eq!(forecast.usage.request_count, 1);
    assert_eq!(forecast.usage.known_cost_count, 1);
    assert_eq!(forecast.usage.unavailable_cost_count, 0);
    assert_eq!(forecast.usage.usd, 2.0);
    assert_eq!(forecast.usage.excluded_request_count, 2);
    assert_eq!(
        usd_total(&db.pool, "acct_prewarm_cost").await.as_deref(),
        Some("5"),
    );
    let entries: Vec<(String, String)> = sqlx::query_as(
        "select request_id, amount::text from account_cumulative_cost_entries
         order by request_id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("inspect retained cost evidence");
    assert_eq!(
        entries,
        vec![
            ("inference".to_owned(), "2.0000000000".to_owned()),
            ("prewarm_known".to_owned(), "3.0000000000".to_owned()),
        ],
    );
    let archived: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(mr) from model_requests mr where id = 'prewarm_known'")
            .fetch_one(&db.pool)
            .await
            .expect("retain synthetic historical request for replay");
    assert_eq!(archived["request_kind"], "prewarm");
    assert_eq!(archived["cost_amount"].as_f64(), Some(3.0));
    sqlx::query("update model_requests set outcome = 'succeeded', client_status_code = null")
        .execute(&db.pool)
        .await
        .expect("repeat terminal observations");
    assert_eq!(
        usd_total(&db.pool, "acct_prewarm_cost").await.as_deref(),
        Some("5"),
    );

    let report = PgRetentionRepository::new(db.pool.clone())
        .apply_retention(
            now,
            RuntimeRetentionSettings {
                usage_retention_days: 31,
                ops_event_retention_days: 30,
                audit_retention_days: 90,
            },
        )
        .await
        .expect("prune only old request logs");
    assert_eq!(report.model_requests, 3);
    assert_eq!(
        usd_total(&db.pool, "acct_prewarm_cost").await.as_deref(),
        Some("5"),
    );
    sqlx::query(
        "insert into model_requests
         select (jsonb_populate_record(null::model_requests, $1::jsonb)).*",
    )
    .bind(archived)
    .execute(&db.pool)
    .await
    .expect("replay the same known prewarm request after retention");
    assert_eq!(
        usd_total(&db.pool, "acct_prewarm_cost").await.as_deref(),
        Some("5"),
    );
    let entry_count: i64 =
        sqlx::query_scalar("select count(*) from account_cumulative_cost_entries")
            .fetch_one(&db.pool)
            .await
            .expect("count durable deduplication entries");
    assert_eq!(entry_count, 2);
    db.close().await;
}

#[tokio::test]
async fn cumulative_cost_distinguishes_accounts_currencies_zero_and_no_evidence() {
    let Some(db) = TestDatabase::create("cumulative_isolation").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(db.pool.clone());
    for name in ["one", "two", "empty"] {
        repository
            .insert_provider_account(account(&format!("acct_{name}"), &format!("user-{name}")))
            .await
            .expect("create distinct account");
    }
    seed_cost(&db.pool, "one_usd", "acct_one", "1.2345678901").await;
    seed_cost(&db.pool, "two_zero", "acct_two", "0").await;
    sqlx::query(
        "insert into model_requests
         select (jsonb_populate_record(null::model_requests,
             to_jsonb(mr) || '{\"id\":\"one_eur\",\"cost_currency\":\"EUR\",\"cost_amount\":9}'::jsonb)).*
         from model_requests mr where id = 'one_usd'",
    )
    .execute(&db.pool)
    .await
    .expect("seed distinct currency");
    let mut ids = vec![
        "acct_one".to_owned(),
        "acct_two".to_owned(),
        "acct_empty".to_owned(),
    ];
    ids.extend((0..200).map(|index| format!("acct_missing_{index}")));
    let costs = admin_account_store(&db.pool)
        .load_account_cumulative_costs(&ids)
        .await
        .expect("load across bounded chunks");
    assert_eq!(costs.len(), 2);
    assert_eq!(costs["acct_one"][0].currency, "EUR");
    assert_eq!(costs["acct_one"][0].amount.as_str(), "9");
    assert_eq!(costs["acct_one"][1].currency, "USD");
    assert_eq!(costs["acct_one"][1].amount.as_str(), "1.2345678901");
    assert_eq!(costs["acct_two"][0].amount.as_str(), "0");
    assert!(!costs.contains_key("acct_empty"));
    assert!(
        admin_account_store(&db.pool)
            .load_account_cumulative_costs(&[])
            .await
            .expect("empty selection")
            .is_empty()
    );
    db.close().await;
}

#[tokio::test]
async fn cumulative_cost_migration_backfills_existing_records_and_keeps_new_writes_idempotent() {
    let Some(db) = TestDatabase::create("cumulative_backfill").await else {
        return;
    };
    sqlx::raw_sql(
        "drop trigger model_requests_accumulate_account_cost on model_requests;
         drop function accumulate_account_cost();
         drop function account_cumulative_cost_is_countable(model_requests);
         drop table account_cumulative_cost_entries;
         drop table account_cumulative_costs;",
    )
    .execute(&db.pool)
    .await
    .expect("restore pre-feature schema");
    PgProviderAccountRepository::new(db.pool.clone())
        .insert_provider_account(account("acct_upgrade", "user-upgrade"))
        .await
        .expect("existing account");
    seed_cost(&db.pool, "before_upgrade", "acct_upgrade", "100").await;
    seed_cost(&db.pool, "failed_before_upgrade", "acct_upgrade", "50").await;
    sqlx::query("update model_requests set outcome = 'failed' where id = 'failed_before_upgrade'")
        .execute(&db.pool)
        .await
        .expect("old failed observation");
    let migration = super::super::TEST_MIGRATOR
        .iter()
        .find(|migration| migration.version == 6)
        .expect("cumulative migration");
    let mut transaction = db.pool.begin().await.expect("migration transaction");
    sqlx::raw_sql(sqlx::AssertSqlSafe(migration.sql.as_str()))
        .execute(&mut *transaction)
        .await
        .expect("apply cumulative migration to existing data");
    transaction.commit().await.expect("commit migration");
    assert_eq!(
        usd_total(&db.pool, "acct_upgrade").await.as_deref(),
        Some("100")
    );
    sqlx::query("update model_requests set outcome = 'succeeded' where id = 'before_upgrade'")
        .execute(&db.pool)
        .await
        .expect("reobserve historical completion");
    seed_cost(&db.pool, "after_upgrade", "acct_upgrade", "5").await;
    assert_eq!(
        usd_total(&db.pool, "acct_upgrade").await.as_deref(),
        Some("105")
    );
    db.close().await;
}
