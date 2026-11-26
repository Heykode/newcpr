use super::*;

#[tokio::test]
async fn scoped_account_reads_follow_current_enabled_groups_without_losing_admin_details() {
    let Some(database) = TestDatabase::create("account_group_scope").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    for id in ["acct_first", "acct_second", "acct_outside"] {
        repository
            .insert_provider_account(account(id, id))
            .await
            .unwrap();
    }
    let active = AccountGroupId::new(format!("grp_{}", "1".repeat(32))).unwrap();
    let disabled = AccountGroupId::new(format!("grp_{}", "2".repeat(32))).unwrap();
    sqlx::query(
        "insert into account_groups (id, name, color, enabled, created_at, updated_at)
         values ($1, 'Visible group', '#2563EBFF', true, now(), now()),
                ($2, 'Disabled group', '#2563EBFF', false, now(), now())",
    )
    .bind(active.as_str())
    .bind(disabled.as_str())
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into account_group_accounts (account_group_id, provider_account_id, created_at)
         values ($1, 'acct_first', now()), ($1, 'acct_outside', now()),
                ($2, 'acct_first', now()), ($2, 'acct_second', now())",
    )
    .bind(active.as_str())
    .bind(disabled.as_str())
    .execute(&database.pool)
    .await
    .unwrap();
    let store = admin_account_store(&database.pool);
    let ids = ["acct_first", "acct_second"].map(|id| ProviderAccountId::new(id).unwrap());
    assert!(
        store
            .load_accounts_in_enabled_groups(&ids, &[])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .load_accounts_in_enabled_groups(&ids, std::slice::from_ref(&disabled))
            .await
            .unwrap()
            .is_empty()
    );
    let accounts = store
        .load_accounts_in_enabled_groups(&ids, &[active.clone(), disabled])
        .await
        .unwrap();
    assert_eq!(
        accounts
            .iter()
            .map(|account| account.id.as_str())
            .collect::<Vec<_>>(),
        ["acct_first"]
    );
    assert!(accounts[0].groups.is_empty());
    let detail = store
        .load_account("acct_first", AccountRuntimeSnapshot::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.account.groups.len(), 2);
    assert!(
        detail
            .account
            .groups
            .iter()
            .any(|group| group.id == active && group.name == "Visible group")
    );

    sqlx::query("update account_groups set enabled = false where id = $1")
        .bind(active.as_str())
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(
        store
            .load_accounts_in_enabled_groups(&ids, std::slice::from_ref(&active))
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("update account_groups set enabled = true where id = $1")
        .bind(active.as_str())
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("update account_group_accounts set provider_account_id = 'acct_second' where account_group_id = $1 and provider_account_id = 'acct_first'")
        .bind(active.as_str()).execute(&database.pool).await.unwrap();
    let accounts = store
        .load_accounts_in_enabled_groups(&ids, &[active])
        .await
        .unwrap();
    assert_eq!(
        accounts
            .iter()
            .map(|account| account.id.as_str())
            .collect::<Vec<_>>(),
        ["acct_second"]
    );
    database.close().await;
}
