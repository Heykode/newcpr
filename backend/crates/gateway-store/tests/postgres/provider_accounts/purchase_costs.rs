use super::*;
use chrono::{Datelike, NaiveDate};
use gateway_admin::model::account_purchase::{AccountPurchaseUpdate, AccountPurchaseView};

fn identified(id: &str, user: &str, workspace: &str) -> NewProviderAccount {
    let mut value = account(id, user);
    value.upstream_account_id = Some(workspace.into());
    value
}

fn month_start() -> NaiveDate {
    (Utc::now() + TimeDelta::hours(8))
        .date_naive()
        .with_day(1)
        .unwrap()
}

fn patch(ids: &[&str], amount: Option<&str>, date: Option<NaiveDate>) -> BatchUpdateAccounts {
    BatchUpdateAccounts {
        purchase_cost: Some(AccountPurchaseUpdate {
            amount_cny: amount.map(str::to_owned),
            cycle_start: date,
        }),
        account_ids: ids.iter().map(|id| (*id).into()).collect(),
        egress_mode: None,
        custom_name: None,
        enabled: None,
        turn_state_injection_enabled: None,
        responses_upstream: None,
        excel_models: None,
        excel_models_follow_global: None,
        excel_cache_creation_as_input: None,
        excel_ignore_encrypted_content: None,
        request_proxy_source: None,
        excel_auto_disable_on_403: None,
        excel_403_action: None,
        concurrency_limit: None,
        weight: None,
        model_access: None,
        group_ids: None,
        outbound_proxy: None,
    }
}

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "purchase-cost-test".into(),
    }
}

#[tokio::test]
async fn purchase_import_applies_per_account_preserves_omission_and_rolls_back_invalid_identity() {
    use gateway_admin::model::accounts::AccountImportSettings;
    let Some(db) = TestDatabase::create("purchase_import_costs").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    let mut settings = AccountImportSettings {
        purchase_cost: Some(AccountPurchaseUpdate {
            amount_cny: Some("50.125".into()),
            cycle_start: Some(month_start()),
        }),
        clear_outbound_proxy: false,
        egress_mode: None,
        custom_name: None,
        enabled: true,
        turn_state_injection_enabled: None,
        responses_upstream: None,
        excel_models: None,
        excel_models_follow_global: None,
        excel_cache_creation_as_input: None,
        excel_ignore_encrypted_content: None,
        request_proxy_source: None,
        excel_auto_disable_on_403: None,
        excel_403_action: None,
        concurrency_limit: None,
        weight: gateway_core::account::AccountWeight::DEFAULT,
        model_access: None,
        group_ids: vec![],
    };
    let import = |settings, accounts| ImportProviderAccounts {
        settings: Some(settings),
        accounts,
        outbound_proxy: None,
        scope: ProviderAccountAdminScope {
            provider_kind: "openai".into(),
        },
        audit: audit(
            &format!("purchase_import_{}", uuid::Uuid::new_v4().simple()),
            "import",
            "provider_accounts",
        ),
    };
    let created = repo
        .import_provider_accounts(import(
            settings.clone(),
            vec![
                identified("acct_purchase_import_a", "import_user_a", "workspace_a"),
                identified("acct_purchase_import_b", "import_user_b", "workspace_b"),
            ],
        ))
        .await
        .unwrap();
    for id in &created.account_ids {
        assert_eq!(
            view(&db.pool, id).await.amount_cny.as_deref(),
            Some("50.1250000000")
        );
    }
    usage(
        &db.pool,
        "purchase_import_usage",
        "acct_purchase_import_a",
        "10",
        Utc::now(),
    )
    .await;
    settings.purchase_cost = None;
    repo.import_provider_accounts(import(
        settings.clone(),
        vec![identified(
            "acct_purchase_duplicate",
            "import_user_a",
            "workspace_a",
        )],
    ))
    .await
    .unwrap();
    let preserved = view(&db.pool, "acct_purchase_import_a").await;
    assert_eq!(preserved.amount_cny.as_deref(), Some("50.1250000000"));
    assert_eq!(preserved.usage_usd, "10.0000000000");
    settings.purchase_cost = Some(AccountPurchaseUpdate {
        amount_cny: Some("60".into()),
        cycle_start: None,
    });
    assert!(
        repo.import_provider_accounts(import(
            settings.clone(),
            vec![
                identified("acct_purchase_duplicate", "import_user_a", "workspace_a"),
                account("acct_purchase_unverified_import", "unverified_import"),
            ]
        ))
        .await
        .is_err()
    );
    assert_eq!(view(&db.pool, "acct_purchase_import_a").await, preserved);
    assert!(
        repo.load_provider_account("acct_purchase_unverified_import")
            .await
            .unwrap()
            .is_none()
    );
    repo.import_provider_accounts(import(
        settings,
        vec![
            identified("acct_purchase_duplicate", "import_user_a", "workspace_a"),
            identified(
                "acct_purchase_duplicate_again",
                "import_user_a",
                "workspace_a",
            ),
        ],
    ))
    .await
    .unwrap();
    assert_eq!(
        view(&db.pool, "acct_purchase_import_a")
            .await
            .amount_cny
            .as_deref(),
        Some("60.0000000000")
    );
    assert_eq!(
        view(&db.pool, "acct_purchase_import_a").await.usage_usd,
        "10.0000000000"
    );
    db.close().await;
}

async fn view(pool: &sqlx::PgPool, id: &str) -> AccountPurchaseView {
    admin_account_store(pool)
        .load_account_purchase_costs(&[id.into()])
        .await
        .unwrap()
        .remove(id)
        .unwrap()
}

async fn usage(
    pool: &sqlx::PgPool,
    request: &str,
    id: &str,
    amount: &str,
    at: chrono::DateTime<Utc>,
) {
    seed_model_request(
        pool,
        ModelRequestSeed {
            request_id: request,
            account_id: id,
            provider_kind: "openai",
            model: "model-purchase",
            total_tokens: 10,
            cost_amount: amount,
            started_at: at,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn purchase_cost_survives_reimport_delete_quota_reset_and_log_cleanup() {
    let Some(db) = TestDatabase::create("purchase_continuity").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    repo.insert_provider_account(identified(
        "acct_purchase_old",
        "purchase_user",
        "workspace_one",
    ))
    .await
    .unwrap();
    let store = admin_account_store(&db.pool);
    store
        .batch_update_accounts(
            patch(&["acct_purchase_old"], Some("50"), Some(month_start())),
            &context(),
        )
        .await
        .unwrap();
    usage(
        &db.pool,
        "purchase_request",
        "acct_purchase_old",
        "200",
        Utc::now(),
    )
    .await;
    let before = view(&db.pool, "acct_purchase_old").await;
    assert_eq!(before.usage_usd, "200.0000000000");
    assert!(
        before
            .breakeven_cny_per_usd
            .as_deref()
            .unwrap()
            .starts_with("0.25")
    );
    let imported = repo
        .import_provider_accounts(ImportProviderAccounts {
            settings: None,
            outbound_proxy: None,
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".into(),
            },
            accounts: vec![identified(
                "acct_purchase_candidate",
                "purchase_user",
                "workspace_one",
            )],
            audit: audit("purchase_import", "import", "acct_purchase_old"),
        })
        .await
        .unwrap();
    assert_eq!(imported.account_ids, ["acct_purchase_old"]);
    assert_eq!(view(&db.pool, "acct_purchase_old").await, before);
    sqlx::query("update provider_accounts set provider_quota_json=null,quota_reset_at=null where id='acct_purchase_old'").execute(&db.pool).await.unwrap();
    repo.delete_provider_accounts_admin(DeleteProviderAccounts {
        scope: ProviderAccountAdminScope {
            provider_kind: "openai".into(),
        },
        account_ids: vec!["acct_purchase_old".into()],
        audit: audit("purchase_delete", "delete", "acct_purchase_old"),
    })
    .await
    .unwrap();
    repo.insert_provider_account(identified(
        "acct_purchase_readded",
        "purchase_user",
        "workspace_one",
    ))
    .await
    .unwrap();
    assert_eq!(view(&db.pool, "acct_purchase_readded").await, before);
    sqlx::query("delete from model_requests")
        .execute(&db.pool)
        .await
        .unwrap();
    usage(
        &db.pool,
        "purchase_request",
        "acct_purchase_readded",
        "200",
        Utc::now(),
    )
    .await;
    assert_eq!(view(&db.pool, "acct_purchase_readded").await, before);
    usage(
        &db.pool,
        "purchase_new",
        "acct_purchase_readded",
        "5",
        Utc::now(),
    )
    .await;
    assert_eq!(
        view(&db.pool, "acct_purchase_readded").await.usage_usd,
        "205.0000000000"
    );
    store
        .batch_update_accounts(patch(&["acct_purchase_readded"], None, None), &context())
        .await
        .unwrap();
    let cleared = view(&db.pool, "acct_purchase_readded").await;
    assert!(cleared.amount_cny.is_none());
    assert!(cleared.breakeven_cny_per_usd.is_none());
    assert_eq!(cleared.usage_usd, "205.0000000000");
    store
        .batch_update_accounts(
            patch(&["acct_purchase_readded"], Some("0"), None),
            &context(),
        )
        .await
        .unwrap();
    let free = view(&db.pool, "acct_purchase_readded").await;
    assert_eq!(free.cycle_anchor, Some(month_start()));
    assert!(free.breakeven_cny_per_usd.unwrap().parse::<f64>().unwrap() == 0.0);
    db.close().await;
}

#[tokio::test]
async fn purchase_months_clamp_without_drifting_and_exclude_prior_cycles() {
    let Some(db) = TestDatabase::create("purchase_months").await else {
        return;
    };
    for (anchor, today, start, end) in [
        ("2026-01-31", "2026-02-28", "2026-02-28", "2026-03-31"),
        ("2026-01-31", "2026-03-30", "2026-02-28", "2026-03-31"),
        ("2026-01-31", "2026-03-31", "2026-03-31", "2026-04-30"),
        ("2024-01-31", "2024-02-29", "2024-02-29", "2024-03-31"),
        ("2025-12-30", "2026-01-30", "2026-01-30", "2026-02-28"),
    ] {
        let result: (String,String)=sqlx::query_as("select period_start::text,period_end::text from account_purchase_period($1::text::date,$2::text::date)")
            .bind(anchor).bind(today).fetch_one(&db.pool).await.unwrap();
        assert_eq!(result, (start.into(), end.into()));
    }
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    repo.insert_provider_account(identified(
        "acct_purchase_month",
        "month_user",
        "workspace_one",
    ))
    .await
    .unwrap();
    admin_account_store(&db.pool)
        .batch_update_accounts(
            patch(&["acct_purchase_month"], Some("10"), Some(month_start())),
            &context(),
        )
        .await
        .unwrap();
    let boundary = month_start().and_hms_opt(0, 0, 0).unwrap().and_utc() - TimeDelta::hours(8);
    usage(
        &db.pool,
        "purchase_prior",
        "acct_purchase_month",
        "100",
        boundary - TimeDelta::seconds(1),
    )
    .await;
    usage(
        &db.pool,
        "purchase_current",
        "acct_purchase_month",
        "20",
        boundary,
    )
    .await;
    assert_eq!(
        view(&db.pool, "acct_purchase_month").await.usage_usd,
        "20.0000000000"
    );
    db.close().await;
}

#[tokio::test]
async fn purchase_batch_is_atomic_and_does_not_mutate_account_configuration() {
    let Some(db) = TestDatabase::create("purchase_atomic").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    repo.insert_provider_account(identified("acct_purchase_a", "same_user", "workspace_a"))
        .await
        .unwrap();
    repo.insert_provider_account(identified("acct_purchase_b", "same_user", "workspace_b"))
        .await
        .unwrap();
    repo.insert_provider_account(account("acct_purchase_unverified", "unknown_user"))
        .await
        .unwrap();
    let before: Vec<serde_json::Value> =
        sqlx::query_scalar("select to_jsonb(a)-'updated_at' from provider_accounts a order by id")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    let store = admin_account_store(&db.pool);
    store
        .batch_update_accounts(
            patch(
                &["acct_purchase_a", "acct_purchase_b"],
                Some("50"),
                Some(month_start()),
            ),
            &context(),
        )
        .await
        .unwrap();
    assert!(
        store
            .batch_update_accounts(
                patch(
                    &["acct_purchase_a", "acct_purchase_unverified"],
                    Some("999"),
                    None
                ),
                &context()
            )
            .await
            .is_err()
    );
    assert_eq!(
        view(&db.pool, "acct_purchase_a")
            .await
            .amount_cny
            .as_deref(),
        Some("50.0000000000")
    );
    let after: Vec<serde_json::Value> =
        sqlx::query_scalar("select to_jsonb(a)-'updated_at' from provider_accounts a order by id")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    usage(
        &db.pool,
        "purchase_a_usage",
        "acct_purchase_a",
        "200",
        Utc::now(),
    )
    .await;
    assert_eq!(view(&db.pool, "acct_purchase_b").await.usage_usd, "0");
    assert!(
        view(&db.pool, "acct_purchase_b")
            .await
            .breakeven_cny_per_usd
            .is_none()
    );
    db.close().await;
}

#[tokio::test]
async fn purchase_usage_is_exact_under_concurrent_requests_and_rollback() {
    let Some(db) = TestDatabase::create("purchase_concurrent").await else {
        return;
    };
    PgProviderAccountRepository::new(db.pool.clone())
        .insert_provider_account(identified(
            "acct_purchase_parallel",
            "parallel_user",
            "workspace_one",
        ))
        .await
        .unwrap();
    admin_account_store(&db.pool)
        .batch_update_accounts(
            patch(&["acct_purchase_parallel"], Some("1"), Some(month_start())),
            &context(),
        )
        .await
        .unwrap();
    futures::future::join_all((0..20).map(|i| {
        let pool = db.pool.clone();
        async move {
            usage(
                &pool,
                &format!("purchase_parallel_{i}"),
                "acct_purchase_parallel",
                "0.1",
                Utc::now(),
            )
            .await;
        }
    }))
    .await;
    assert_eq!(
        view(&db.pool, "acct_purchase_parallel").await.usage_usd,
        "2.0000000000"
    );
    usage(
        &db.pool,
        "purchase_rollback_source",
        "acct_purchase_parallel",
        "1",
        Utc::now(),
    )
    .await;
    sqlx::query("update model_requests set outcome='failed',downstream_committed_at=null where id='purchase_rollback_source'")
        .execute(&db.pool).await.unwrap();
    sqlx::query(
        "delete from account_cumulative_cost_entries where request_id='purchase_rollback_source'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let mut tx = db.pool.begin().await.unwrap();
    sqlx::query("update model_requests set outcome='succeeded',downstream_committed_at=now(),cost_amount=10 where id='purchase_rollback_source'")
        .execute(&mut *tx).await.unwrap();
    let during: String =
        sqlx::query_scalar("select sum(amount_usd)::text from account_purchase_daily_usage")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        during, "13.0000000000",
        "the transaction must actually write a daily cost before rollback"
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        view(&db.pool, "acct_purchase_parallel").await.usage_usd,
        "3.0000000000"
    );
    db.close().await;
}

#[tokio::test]
async fn purchase_inflight_snapshot_survives_workspace_change_and_deleted_account() {
    let Some(db) = TestDatabase::create("purchase_snapshot").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    repo.insert_provider_account(identified(
        "acct_purchase_inflight",
        "snapshot_user",
        "workspace_a",
    ))
    .await
    .unwrap();
    admin_account_store(&db.pool)
        .batch_update_accounts(
            patch(&["acct_purchase_inflight"], Some("50"), Some(month_start())),
            &context(),
        )
        .await
        .unwrap();
    usage(
        &db.pool,
        "purchase_inflight",
        "acct_purchase_inflight",
        "0",
        Utc::now(),
    )
    .await;
    // Reuse the complete synthetic row as a selected, not-yet-billed request.
    sqlx::query("update model_requests set outcome='failed',downstream_committed_at=null where id='purchase_inflight'")
        .execute(&db.pool).await.unwrap();
    sqlx::query("delete from account_cumulative_cost_entries where request_id='purchase_inflight'")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("update provider_accounts set upstream_account_id='workspace_b' where id='acct_purchase_inflight'")
        .execute(&db.pool).await.unwrap();
    repo.insert_provider_account(identified(
        "acct_purchase_original",
        "snapshot_user",
        "workspace_a",
    ))
    .await
    .unwrap();
    sqlx::query("delete from provider_accounts where id='acct_purchase_inflight'")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("update model_requests set provider_account_ref=provider_account_ref,outcome='succeeded',downstream_committed_at=now(),cost_amount=25 where id='purchase_inflight'")
        .execute(&db.pool).await.unwrap();
    assert_eq!(
        view(&db.pool, "acct_purchase_original").await.usage_usd,
        "25.0000000000"
    );
    assert_eq!(
        view(&db.pool, "acct_purchase_inflight").await.amount_cny,
        None
    );
    assert_eq!(
        view(&db.pool, "acct_purchase_inflight").await.usage_usd,
        "0"
    );
    db.close().await;
}

#[tokio::test]
async fn purchase_upgrade_does_not_assign_an_old_workspace_history_to_a_new_one() {
    let old = sqlx::migrate::Migrator {
        migrations: super::super::TEST_MIGRATOR
            .iter()
            .filter(|m| m.version < 57)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let Some(db) = TestDatabase::create_with_migrator("purchase_upgrade_identity", &old).await
    else {
        return;
    };
    PgProviderAccountRepository::new(db.pool.clone())
        .insert_provider_account(identified(
            "acct_purchase_rotated",
            "rotated_user",
            "workspace_old",
        ))
        .await
        .unwrap();
    usage(
        &db.pool,
        "purchase_old_workspace",
        "acct_purchase_rotated",
        "100",
        Utc::now() - TimeDelta::hours(2),
    )
    .await;
    sqlx::query("update provider_accounts set upstream_account_id='workspace_new',credential_revision=2,credential_observed_at=now()-interval '1 hour',updated_at=now() where id='acct_purchase_rotated'")
        .execute(&db.pool).await.unwrap();
    usage(
        &db.pool,
        "purchase_between_observation_and_commit",
        "acct_purchase_rotated",
        "100",
        Utc::now() - TimeDelta::minutes(30),
    )
    .await;
    usage(
        &db.pool,
        "purchase_new_workspace",
        "acct_purchase_rotated",
        "5",
        Utc::now(),
    )
    .await;
    super::super::TEST_MIGRATOR.run(&db.pool).await.unwrap();
    admin_account_store(&db.pool)
        .batch_update_accounts(
            patch(&["acct_purchase_rotated"], Some("50"), Some(month_start())),
            &context(),
        )
        .await
        .unwrap();
    let result = view(&db.pool, "acct_purchase_rotated").await;
    assert_eq!(result.usage_usd, "5.0000000000");
    assert!(!result.history_complete);
    db.close().await;
}

#[tokio::test]
async fn purchase_new_attempt_snapshots_new_workspace_even_when_local_id_is_unchanged() {
    let Some(db) = TestDatabase::create("purchase_retry_identity").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    repo.insert_provider_account(identified(
        "acct_purchase_retry",
        "retry_user",
        "workspace_before",
    ))
    .await
    .unwrap();
    let store = admin_account_store(&db.pool);
    store
        .batch_update_accounts(
            patch(&["acct_purchase_retry"], Some("50"), Some(month_start())),
            &context(),
        )
        .await
        .unwrap();
    usage(
        &db.pool,
        "purchase_retry",
        "acct_purchase_retry",
        "0",
        Utc::now(),
    )
    .await;
    sqlx::query("update model_requests set outcome='failed',downstream_committed_at=null where id='purchase_retry'").execute(&db.pool).await.unwrap();
    sqlx::query("delete from account_cumulative_cost_entries where request_id='purchase_retry'")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("update provider_accounts set upstream_account_id='workspace_after' where id='acct_purchase_retry'").execute(&db.pool).await.unwrap();
    repo.insert_provider_account(identified(
        "acct_purchase_before",
        "retry_user",
        "workspace_before",
    ))
    .await
    .unwrap();
    store
        .batch_update_accounts(
            patch(&["acct_purchase_retry"], Some("50"), Some(month_start())),
            &context(),
        )
        .await
        .unwrap();
    sqlx::query("update model_requests set provider_account_ref=provider_account_ref,attempt_count=2 where id='purchase_retry'").execute(&db.pool).await.unwrap();
    sqlx::query("update model_requests set outcome='succeeded',downstream_committed_at=now(),cost_amount=25 where id='purchase_retry'").execute(&db.pool).await.unwrap();
    assert_eq!(
        view(&db.pool, "acct_purchase_retry").await.usage_usd,
        "25.0000000000"
    );
    assert_eq!(
        view(&db.pool, "acct_purchase_before").await.usage_usd,
        "0.0000000000"
    );
    db.close().await;
}

#[tokio::test]
async fn purchase_late_identity_does_not_claim_unknown_history_is_complete() {
    let Some(db) = TestDatabase::create("purchase_late_identity").await else {
        return;
    };
    PgProviderAccountRepository::new(db.pool.clone())
        .insert_provider_account(account("acct_purchase_unknown", "later_user"))
        .await
        .unwrap();
    usage(
        &db.pool,
        "purchase_unidentified_usage",
        "acct_purchase_unknown",
        "10",
        Utc::now(),
    )
    .await;
    sqlx::query("update provider_accounts set upstream_account_id='workspace_verified' where id='acct_purchase_unknown'")
        .execute(&db.pool).await.unwrap();
    admin_account_store(&db.pool)
        .batch_update_accounts(
            patch(&["acct_purchase_unknown"], Some("50"), Some(month_start())),
            &context(),
        )
        .await
        .unwrap();
    let result = view(&db.pool, "acct_purchase_unknown").await;
    assert_eq!(result.usage_usd, "0");
    assert!(!result.history_complete);
    usage(
        &db.pool,
        "purchase_identified_usage",
        "acct_purchase_unknown",
        "2",
        Utc::now(),
    )
    .await;
    assert_eq!(
        view(&db.pool, "acct_purchase_unknown").await.usage_usd,
        "2.0000000000"
    );
    db.close().await;
}

#[tokio::test]
async fn purchase_upgrade_backfills_only_proven_history_and_marks_missing_records() {
    let old = sqlx::migrate::Migrator {
        migrations: super::super::TEST_MIGRATOR
            .iter()
            .filter(|m| m.version < 57)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let Some(db) = TestDatabase::create_with_migrator("purchase_upgrade", &old).await else {
        return;
    };
    PgProviderAccountRepository::new(db.pool.clone())
        .insert_provider_account(identified(
            "acct_purchase_legacy",
            "legacy_user",
            "workspace_one",
        ))
        .await
        .unwrap();
    usage(
        &db.pool,
        "purchase_kept",
        "acct_purchase_legacy",
        "20",
        Utc::now(),
    )
    .await;
    usage(
        &db.pool,
        "purchase_pruned",
        "acct_purchase_legacy",
        "100",
        Utc::now(),
    )
    .await;
    sqlx::query("delete from model_requests where id='purchase_pruned'")
        .execute(&db.pool)
        .await
        .unwrap();
    super::super::TEST_MIGRATOR.run(&db.pool).await.unwrap();
    admin_account_store(&db.pool)
        .batch_update_accounts(
            patch(&["acct_purchase_legacy"], Some("50"), Some(month_start())),
            &context(),
        )
        .await
        .unwrap();
    let result = view(&db.pool, "acct_purchase_legacy").await;
    assert_eq!(result.usage_usd, "20.0000000000");
    assert!(!result.history_complete);
    assert!(result.history_complete_from.is_some());
    sqlx::query("delete from model_requests where id='purchase_kept'")
        .execute(&db.pool)
        .await
        .unwrap();
    usage(
        &db.pool,
        "purchase_kept",
        "acct_purchase_legacy",
        "20",
        Utc::now(),
    )
    .await;
    assert_eq!(
        view(&db.pool, "acct_purchase_legacy").await.usage_usd,
        "20.0000000000"
    );
    db.close().await;
}
