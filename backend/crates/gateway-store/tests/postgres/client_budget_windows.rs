//! 显式窗口的事务、重试和结算边界

use super::{
    TestDatabase,
    client_budgets::{charge, context, key_id, seed, status},
};
use chrono::{Duration, Utc};
use gateway_admin::{
    model::client_keys::{
        ClientKeyBudgetWindowPeriod, ExplicitClientKeyBudgetWindow, SetClientKeyBudgetWindow,
    },
    ports::store::{AdminStoreErrorKind, ClientKeyStore as _},
};
use gateway_core::engine::budget::ClientBudgetPort;
use gateway_store::postgres::{PgAdminClientKeyStore, PgClientBudgetStore};

fn window(
    revision: u64,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
) -> SetClientKeyBudgetWindow {
    SetClientKeyBudgetWindow {
        id: key_id("key"),
        period: ClientKeyBudgetWindowPeriod::Weekly,
        expected_revision: revision,
        window: Some(ExplicitClientKeyBudgetWindow {
            resume_calendar: true,
            starts_at: start,
            ends_at: end,
        }),
    }
}

#[tokio::test]
async fn explicit_window_retries_preserve_spending_and_reject_obsolete_operations() {
    let Some(database) = TestDatabase::create("explicit_retries").await else {
        return;
    };
    seed(&database, "key", "0", "10").await;
    let admin = PgAdminClientKeyStore::new(database.pool.clone());
    let budgets = PgClientBudgetStore::new(database.pool.clone());
    budgets.settle(charge("key", "before", "2")).await.unwrap();
    let now = chrono::DateTime::from_timestamp_millis(Utc::now().timestamp_millis()).unwrap();
    let command = window(0, now - Duration::hours(1), now + Duration::hours(1));
    assert_eq!(
        admin
            .set_client_key_budget_window(command.clone(), &context())
            .await
            .unwrap(),
        1
    );
    budgets.settle(charge("key", "after", "3")).await.unwrap();
    assert_eq!(
        admin
            .set_client_key_budget_window(command.clone(), &context())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        status(&database, "key").await.weekly_used_usd.canonical(),
        "5"
    );
    let mut changed = command.clone();
    changed.window.as_mut().unwrap().ends_at += Duration::hours(1);
    assert_eq!(
        admin
            .set_client_key_budget_window(changed, &context())
            .await
            .unwrap_err()
            .kind(),
        AdminStoreErrorKind::Conflict
    );
    let a = window(1, now - Duration::minutes(30), now + Duration::hours(2));
    let b = window(1, now - Duration::minutes(30), now + Duration::hours(3));
    let mutation_context = context();
    let (a, b) = tokio::join!(
        admin.set_client_key_budget_window(a, &mutation_context),
        admin.set_client_key_budget_window(b, &mutation_context)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        a.err().or(b.err()).unwrap().kind(),
        AdminStoreErrorKind::Conflict
    );
    assert_eq!(
        admin
            .set_client_key_budget_window(command, &context())
            .await
            .unwrap_err()
            .kind(),
        AdminStoreErrorKind::StaleRevision
    );
    let audits: i64 = sqlx::query_scalar(
        "select count(*) from admin_audit_events where action='set_budget_window'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audits, 2);
    database.close().await;
}

#[tokio::test]
async fn manual_alignment_counts_only_current_charges_and_returns_to_calendar_at_expiry() {
    let Some(db) = TestDatabase::create("manual_alignment").await else {
        return;
    };
    seed(&db, "key", "10", "20").await;
    let admin = PgAdminClientKeyStore::new(db.pool.clone());
    let budgets = PgClientBudgetStore::new(db.pool.clone());
    let now = chrono::DateTime::from_timestamp_millis(Utc::now().timestamp_millis()).unwrap();
    let mut outside = charge("key", "outside", "4");
    outside.completed_at = (now - Duration::days(8)).into();
    budgets.settle(outside).await.unwrap();
    budgets.settle(charge("key", "inside", "3")).await.unwrap();
    let before = status(&db, "key").await;
    let end = now + Duration::days(1);
    let mut command = window(0, end - Duration::days(7), end);
    command.window.as_mut().unwrap().resume_calendar = true;
    let mut expired = command.clone();
    expired.window.as_mut().unwrap().ends_at = now;
    assert_eq!(
        admin
            .set_client_key_budget_window(expired, &context())
            .await
            .unwrap_err()
            .kind(),
        AdminStoreErrorKind::Conflict
    );
    admin
        .set_client_key_budget_window(command.clone(), &context())
        .await
        .unwrap();
    let after = status(&db, "key").await;
    assert_eq!(after.daily_used_usd, before.daily_used_usd);
    assert_eq!(after.limits, before.limits);
    assert_eq!(after.weekly_used_usd.canonical(), "3");
    let aligned = admin
        .client_key_budget_window(&key_id("key"), ClientKeyBudgetWindowPeriod::Weekly)
        .await
        .unwrap();
    assert_eq!(aligned.ends_at, Some(end));
    assert!(!aligned.explicit);
    budgets.settle(charge("key", "after", "2")).await.unwrap();
    assert_eq!(
        admin
            .set_client_key_budget_window(command, &context())
            .await
            .unwrap(),
        1
    );
    assert_eq!(status(&db, "key").await.weekly_used_usd.canonical(), "5");
    // 将同一账本推进至到期状态，验证下一次准入会续开原生窗口
    sqlx::query("update client_key_budget_windows set weekly_start = weekly_start - interval '2 days', weekly_end = weekly_end - interval '2 days' where client_api_key_id = 'key'").execute(&db.pool).await.unwrap();
    budgets.admit(key_id("key")).await.unwrap();
    let next = admin
        .client_key_budget_window(&key_id("key"), ClientKeyBudgetWindowPeriod::Weekly)
        .await
        .unwrap();
    assert!(next.ends_at.unwrap() > end);
    assert_eq!(next.used_usd.canonical(), "0");
    db.close().await;
}

#[tokio::test]
async fn reference_accounts_page_through_large_pools_and_search_only_key_groups() {
    let Some(database) = TestDatabase::create("budget_reference_accounts").await else {
        return;
    };
    seed(&database, "key", "0", "0").await;
    let key = key_id("key");
    let store = PgAdminClientKeyStore::new(database.pool.clone());
    sqlx::query(
        "insert into provider_accounts (id, name, email, provider_kind, authentication_kind,
            provider_credentials_json, has_refresh_token, credential_observed_at, created_at, updated_at)
         select 'ref-' || n, 'Account ' || n, 'person' || n || '@example.test', 'openai', 'oauth',
                '{}'::jsonb, false, now(), now(), now()
         from generate_series(1000, 2199) n",
    ).execute(&database.pool).await.unwrap();
    let first = store.budget_reference_accounts(&key, "", "").await.unwrap();
    assert_eq!(first.items.len(), 20);
    let next = store
        .budget_reference_accounts(&key, "", first.next_cursor.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(next.items[0].id, "ref-1020");
    let id_only = store
        .budget_reference_accounts(&key, "ref-2199", "")
        .await
        .unwrap();
    assert!(id_only.items.is_empty());
    for term in ["PERSON2199@", "account 2199"] {
        let page = store
            .budget_reference_accounts(&key, term, "")
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, "ref-2199");
        assert!(page.next_cursor.is_none());
    }
    let group = "grp_00000000000000000000000000000001";
    sqlx::query("insert into account_groups(id, name, color, created_at, updated_at) values ($1, 'Test', '#2563EBFF', now(), now())")
        .bind(group).execute(&database.pool).await.unwrap();
    sqlx::query("insert into client_api_key_groups(client_api_key_id, account_group_id, created_at) values ('key', $1, now())").bind(group).execute(&database.pool).await.unwrap();
    sqlx::query("insert into account_group_accounts(account_group_id, provider_account_id, created_at) values ($1, 'ref-2198', now())").bind(group).execute(&database.pool).await.unwrap();
    let scoped = store
        .budget_reference_accounts(&key, "219", "")
        .await
        .unwrap();
    assert_eq!(scoped.items.len(), 1);
    assert_eq!(scoped.items[0].id, "ref-2198");
    sqlx::query("update account_groups set enabled = false where id = $1")
        .bind(group)
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(
        store
            .budget_reference_accounts(&key, "", "")
            .await
            .unwrap()
            .items
            .is_empty()
    );
    database.close().await;
}
