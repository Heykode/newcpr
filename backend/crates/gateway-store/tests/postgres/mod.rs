use std::{str::FromStr, time::Duration};

use gateway_store::postgres::{
    ObservabilityQueryBudget, PgAdminAccountStore, PgAdminObservabilityStore,
    PgObservabilityRepository, connect_and_migrate,
};
use sqlx::{
    ConnectOptions as _, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use uuid::Uuid;

mod account_groups;
mod admin_security_audit;
mod admission_recovery;
mod backup;
mod client_budget_buffer;
mod client_budgets;
mod client_keys;
mod egress;
mod execution;
mod execution_buffer;
mod group_monitor_snapshots;
mod health;
mod import_email_snapshots;
mod log_cleanup;
mod notifications;
mod observability;
mod ops_events;
mod outbound_user_agent;
mod provider_accounts;
mod proxies;
mod proxy_test_locks;
mod quality_ops;
mod query_budget;
mod quota_learning;
mod quota_overdraft;
mod relogin;
mod request_capture;
mod reset_credits;
mod retention;
mod runtime_settings;
mod schema_integrity;
mod shared_account_wait;
mod snapshot;
mod snapshots;
mod turn_state_upgrade;
mod turn_states;

static TEST_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub(super) struct TestDatabase {
    admin: PgPool,
    pub(super) pool: PgPool,
    schema: String,
}

pub(super) fn observability_query_budget() -> ObservabilityQueryBudget {
    ObservabilityQueryBudget::try_new(4, Duration::from_secs(1))
        .expect("valid test observability query budget")
}

pub(super) fn observability_repository(pool: &PgPool) -> PgObservabilityRepository {
    PgObservabilityRepository::new(pool.clone(), None, observability_query_budget())
}

pub(super) fn admin_observability_store(pool: &PgPool) -> PgAdminObservabilityStore {
    PgAdminObservabilityStore::new(pool.clone(), None, None, observability_query_budget())
}

pub(super) fn admin_account_store(pool: &PgPool) -> PgAdminAccountStore {
    PgAdminAccountStore::new(pool.clone(), None, observability_query_budget())
}

impl TestDatabase {
    pub(super) async fn create(label: &str) -> Option<Self> {
        Self::create_with_migrator(label, &TEST_MIGRATOR).await
    }

    async fn create_with_migrator(label: &str, migrator: &sqlx::migrate::Migrator) -> Option<Self> {
        let database_url = crate::support::test_env("CPR_TEST_DATABASE_URL")?;
        let schema = format!("cpr_store_{label}_{}", Uuid::new_v4().simple());
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("connect test PostgreSQL");
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("create schema \"{schema}\"")))
            .execute(&admin)
            .await
            .expect("create test schema");
        let search_path = schema.clone();
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(move |connection, _metadata| {
                let search_path = search_path.clone();
                Box::pin(async move {
                    sqlx::query("select set_config('search_path', $1, false)")
                        .bind(search_path)
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .expect("connect isolated test schema");
        {
            let mut connection = pool.acquire().await.expect("migration connection");
            gateway_store::postgres::migrate_schema(&mut connection, migrator)
                .await
                .expect("apply test migrations");
        }
        Some(Self {
            admin,
            pool,
            schema,
        })
    }

    pub(super) async fn close(self) {
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "drop schema \"{}\" cascade",
            self.schema
        )))
        .execute(&self.admin)
        .await
        .expect("drop test schema");
        self.admin.close().await;
    }
}

#[tokio::test]
async fn connect_and_migrate_should_apply_all_migrations_once_and_reopen_cleanly() {
    let Some(database_url) = crate::support::test_env("CPR_TEST_DATABASE_URL") else {
        return;
    };
    let database = format!("cpr_store_migrator_{}", Uuid::new_v4().simple());
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("connect migration test PostgreSQL");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "create database \"{database}\""
    )))
    .execute(&admin)
    .await
    .expect("create migration test database");

    let isolated_url = PgConnectOptions::from_str(&database_url)
        .expect("parse migration test PostgreSQL URL")
        .database(&database)
        .to_url_lossy()
        .to_string();
    let pool_config = gateway_store::StorePoolConfig::default();
    let first = connect_and_migrate(&isolated_url, pool_config)
        .await
        .expect("apply migrations through production migrator");
    let initial: i64 =
        sqlx::query_scalar("select max_concurrent_per_account from runtime_settings where id=1")
            .fetch_one(&first)
            .await
            .unwrap();
    assert_eq!(initial, 3);
    sqlx::query("update runtime_settings set max_concurrent_per_account=7 where id=1")
        .execute(&first)
        .await
        .unwrap();
    let session_settings = sqlx::query_as::<_, (String, i64, i64, i64)>(
        "select current_setting('application_name'),
                extract(epoch from current_setting('statement_timeout')::interval)::bigint,
                extract(epoch from current_setting('lock_timeout')::interval)::bigint,
                extract(epoch from current_setting(
                    'idle_in_transaction_session_timeout'
                )::interval)::bigint",
    )
    .fetch_one(&first)
    .await
    .expect("load runtime PostgreSQL session settings");
    let first_tables = sqlx::query_scalar::<_, String>(
        "select table_name
         from information_schema.tables
         where table_schema = 'public'
         order by table_name",
    )
    .fetch_all(&first)
    .await
    .expect("load migrated tables");
    first.close().await;

    let second = connect_and_migrate(&isolated_url, pool_config)
        .await
        .expect("reopen database through production migrator");
    let saved: i64 =
        sqlx::query_scalar("select max_concurrent_per_account from runtime_settings where id=1")
            .fetch_one(&second)
            .await
            .unwrap();
    assert_eq!(saved, 7);
    let migration_count =
        sqlx::query_scalar::<_, i64>("select count(*) from _sqlx_migrations where success")
            .fetch_one(&second)
            .await
            .expect("count successful migrations");
    let response_id_types = sqlx::query_scalar::<_, String>(
        "select data_type
         from information_schema.columns
         where table_schema = 'public'
           and table_name = 'model_requests'
           and column_name in ('client_response_id', 'upstream_response_id')
         order by column_name",
    )
    .fetch_all(&second)
    .await
    .expect("load opaque response ID column types");
    let raw_response_id_index_exists = sqlx::query_scalar::<_, bool>(
        "select exists (
           select 1
           from pg_indexes
           where schemaname = 'public'
             and indexname = 'model_requests_client_response_uq'
         )",
    )
    .fetch_one(&second)
    .await
    .expect("check removed raw response ID index");
    let legacy_key_provider_column_exists = sqlx::query_scalar::<_, bool>(
        "select exists (
           select 1 from information_schema.columns
           where table_schema = 'public'
             and table_name = 'client_api_keys'
             and column_name = 'provider_kind'
         )",
    )
    .fetch_one(&second)
    .await
    .expect("check removed client key provider column");
    let routing_history_columns = sqlx::query_scalar::<_, String>(
        "select column_name from information_schema.columns
         where table_schema = 'public'
           and table_name = 'model_requests'
           and column_name in (
             'routing_scope', 'routing_group_refs', 'routing_group_names_snapshot'
           )
         order by column_name",
    )
    .fetch_all(&second)
    .await
    .expect("load routing history columns");
    second.close().await;

    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "drop database \"{database}\" with (force)"
    )))
    .execute(&admin)
    .await
    .expect("drop migration test database");
    admin.close().await;

    assert_eq!(
        first_tables,
        [
            "_sqlx_migrations",
            "account_auto_reset_jobs",
            "account_auto_reset_policies",
            "account_cumulative_cost_entries",
            "account_cumulative_costs",
            "account_excel_recovery",
            "account_group_accounts",
            "account_group_alert_incidents",
            "account_group_alert_policies",
            "account_group_monitor_snapshots",
            "account_groups",
            "account_purchase_bindings",
            "account_purchase_daily_usage",
            "account_purchase_identities",
            "account_relogin_entries",
            "account_relogin_history",
            "account_relogin_settings",
            "account_relogin_successes",
            "account_relogin_templates",
            "account_reset_batches",
            "account_reset_consumptions",
            "account_reset_inventories",
            "account_token_guard_config",
            "account_token_guard_events",
            "admin_audit_events",
            "admin_users",
            "backup_records",
            "backup_settings",
            "backup_usage_hours",
            "client_api_key_groups",
            "client_api_keys",
            "client_key_budget_windows",
            "client_key_charge_events",
            "codex_quota_overdraft_states",
            "excel_recovery_slots",
            "log_cleanup_control",
            "model_requests",
            "monitor_account_lifecycles",
            "notification_channels",
            "notification_outbox",
            "openai_excel_credentials",
            "ops_events",
            "outbound_proxies",
            "provider_account_model_catalogs",
            "provider_accounts",
            "provider_device_identities",
            "provider_egress_account_overrides",
            "provider_egress_addresses",
            "provider_egress_fixed_affinity",
            "provider_egress_settings",
            "provider_outbound_user_agents",
            "provider_turn_states",
            "quality_group_members",
            "quality_group_rules",
            "quality_rule_templates",
            "quality_rules",
            "quality_runs",
            "quota_learning_accounts",
            "quota_learning_plan_samples",
            "request_capture_config",
            "request_capture_records",
            "request_capture_tasks",
            "runtime_settings",
        ]
    );
    assert_eq!(session_settings, ("codex-proxy-rs".to_owned(), 30, 5, 30));
    assert_eq!(
        migration_count,
        i64::try_from(TEST_MIGRATOR.iter().count())
            .expect("migration count fits PostgreSQL bigint")
    );
    assert_eq!(response_id_types, ["bytea", "bytea"]);
    assert!(!raw_response_id_index_exists);
    assert!(!legacy_key_provider_column_exists);
    assert_eq!(
        routing_history_columns,
        [
            "routing_group_names_snapshot",
            "routing_group_refs",
            "routing_scope",
        ]
    );
}

#[tokio::test]
async fn migrate_schema_should_upgrade_mainline_to_excel_once_and_preserve_account_settings() {
    let mainline = sqlx::migrate::Migrator {
        migrations: TEST_MIGRATOR
            .iter()
            .filter(|migration| migration.version <= 91)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let excel = sqlx::migrate::Migrator {
        migrations: TEST_MIGRATOR
            .iter()
            .filter(|migration| migration.version <= 92)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let Some(db) = TestDatabase::create_with_migrator("excel_upgrade", &mainline).await else {
        return;
    };
    sqlx::query(
        "insert into provider_accounts (
           id, provider_kind, name, authentication_kind, upstream_user_id, upstream_account_id,
           provider_credentials_json, credential_revision, has_refresh_token, weight, enabled,
           credential_observed_at, created_at, updated_at
         ) values (
           'acct_excel_upgrade', 'openai', 'migration fixture', 'oauth', 'user-fixture',
           'workspace-fixture', '{\"codex\":\"fixture\"}', 3, true, 7, false, now(), now(), now()
         )",
    )
    .execute(&db.pool)
    .await
    .expect("seed existing Codex credential and account settings");
    sqlx::query(
        "update runtime_settings
         set max_concurrent_per_account = 7, usage_retention_days = 4 where id = 1",
    )
    .execute(&db.pool)
    .await
    .expect("save custom mainline runtime settings");
    let saved: serde_json::Value = sqlx::query_scalar(
        "select jsonb_build_object(
           'account', (select to_jsonb(a) from provider_accounts a where id = 'acct_excel_upgrade'),
           'settings', (select to_jsonb(s) from runtime_settings s where id = 1),
           'mainline_migration', (select to_jsonb(m) from _sqlx_migrations m where version = 91)
         )",
    )
    .fetch_one(&db.pool)
    .await
    .expect("snapshot existing account, settings and mainline migration");
    let mut first_ledger = None;
    for _ in 0..2 {
        let mut connection = db.pool.acquire().await.expect("migration connection");
        gateway_store::postgres::migrate_schema(&mut connection, &excel)
            .await
            .expect("upgrade or reopen mainline database through production migrator");
        drop(connection);
        let retained: serde_json::Value = sqlx::query_scalar(
            "select jsonb_build_object(
               'account', (select to_jsonb(a) from provider_accounts a where id = 'acct_excel_upgrade'),
               'settings', (select to_jsonb(s) from runtime_settings s where id = 1),
               'mainline_migration', (select to_jsonb(m) from _sqlx_migrations m where version = 91)
             )",
        )
        .fetch_one(&db.pool)
        .await
        .expect("read preserved mainline data");
        assert_eq!(retained, saved);
        let excel_count: i64 = sqlx::query_scalar("select count(*) from openai_excel_credentials")
            .fetch_one(&db.pool)
            .await
            .expect("new Excel credential table exists");
        assert_eq!(
            excel_count, 0,
            "Codex credentials must not be copied to Excel"
        );
        let excel_migration: (bool, Vec<u8>) =
            sqlx::query_as("select success, checksum from _sqlx_migrations where version = 92")
                .fetch_one(&db.pool)
                .await
                .expect("Excel migration recorded at version 92");
        let expected_checksum = excel
            .iter()
            .find(|migration| migration.version == 92)
            .expect("frozen Excel migration")
            .checksum
            .as_ref();
        assert!(excel_migration.0);
        assert_eq!(excel_migration.1, expected_checksum);
        let ledger: serde_json::Value = sqlx::query_scalar(
            "select jsonb_agg(to_jsonb(m) order by version) from _sqlx_migrations m",
        )
        .fetch_one(&db.pool)
        .await
        .expect("snapshot complete migration ledger");
        if let Some(first) = first_ledger.as_ref() {
            assert_eq!(&ledger, first, "reopening must not rerun migrations");
        } else {
            first_ledger = Some(ledger);
        }
    }
    db.close().await;
}

#[test]
fn migrations_should_leave_transaction_ownership_to_sqlx() {
    let transaction_statements = TEST_MIGRATOR
        .iter()
        .flat_map(|migration| migration.sql.as_str().lines())
        .map(str::trim)
        .filter(|line| matches!(*line, "begin;" | "commit;"))
        .count();

    assert_eq!(transaction_statements, 0);
}
