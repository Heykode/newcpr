use super::*;
use gateway_core::account::{AccountModelAccess, AccountModelAccessMode};

#[tokio::test]
async fn model_policy_patch_preserves_identity_state_groups_and_egress() {
    let Some(database) = TestDatabase::create("model_policy_patch").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    repository
        .insert_provider_account(account("acct_models", "model-owner"))
        .await
        .unwrap();
    super::super::turn_states::enable(&database).await;
    sqlx::query(
        "insert into provider_turn_states (
            provider_account_id, upstream_model, credential_revision, normal_length,
            active_state, active_issued_at, active_expires_at, refresh_status
        ) values ('acct_models', 'model-a', 1, 292, 'synthetic-state',
                  now(), now()+interval '1 hour', 'ready')",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into account_groups (id, name, color, created_at, updated_at)
         values ('grp_00000000000000000000000000000091', 'Model group', '#2563EBFF', now(), now())",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into account_group_accounts (account_group_id, provider_account_id, created_at)
         values ('grp_00000000000000000000000000000091', 'acct_models', now())",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let before: serde_json::Value = sqlx::query_scalar(
        "select to_jsonb(a)-array['model_access_json','updated_at'] from provider_accounts a where id='acct_models'",
    ).fetch_one(&database.pool).await.unwrap();
    let state_before: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(s) from provider_turn_states s")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    let policy =
        AccountModelAccess::new(AccountModelAccessMode::Denylist, vec!["model-a".into()]).unwrap();
    let store = admin_account_store(&database.pool);
    let command = BatchUpdateAccounts {
        account_ids: vec!["acct_models".into()],
        model_access: Some(policy.clone()),
        custom_name: None,
        enabled: None,
        turn_state_injection_enabled: None,
        responses_upstream: Default::default(),
        excel_models_follow_global: Default::default(),
        excel_models: Default::default(),
        concurrency_limit: None,
        weight: None,
        group_ids: None,
        outbound_proxy: None,
    };
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "model-only".into(),
    };
    store
        .batch_update_accounts(command.clone(), &context)
        .await
        .unwrap();
    let after: serde_json::Value = sqlx::query_scalar(
        "select to_jsonb(a)-array['model_access_json','updated_at'] from provider_accounts a where id='acct_models'",
    ).fetch_one(&database.pool).await.unwrap();
    let state_after: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(s) from provider_turn_states s")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    assert_eq!(state_before, state_after);
    assert_eq!(
        account_group_ids(&database.pool, "acct_models").await,
        ["grp_00000000000000000000000000000091"]
    );
    assert_eq!(
        repository
            .load_provider_account("acct_models")
            .await
            .unwrap()
            .unwrap()
            .summary
            .model_access,
        policy
    );
    assert_eq!(
        repository
            .get_account(&ProviderAccountId::new("acct_models").unwrap())
            .await
            .unwrap()
            .unwrap()
            .model_access(),
        &policy
    );
    assert_eq!(
        repository
            .list_provider_accounts(Some("openai"), true)
            .await
            .unwrap()[0]
            .model_access,
        policy
    );
    use gateway_store::postgres::{PgRuntimeSnapshotRepository, RuntimeSnapshotRepository};
    let snapshot = PgRuntimeSnapshotRepository::new(database.pool.clone())
        .load_runtime_snapshot()
        .await
        .unwrap();
    assert_eq!(snapshot.provider_accounts[0].model_access, policy);
    assert!(
        store
            .batch_update_accounts(
                BatchUpdateAccounts {
                    model_access: Some(AccountModelAccess::all()),
                    group_ids: Some(vec![
                        AccountGroupId::new("grp_00000000000000000000000000000099").unwrap()
                    ]),
                    ..command.clone()
                },
                &context
            )
            .await
            .is_err()
    );
    assert_eq!(
        repository
            .load_provider_account("acct_models")
            .await
            .unwrap()
            .unwrap()
            .summary
            .model_access,
        policy
    );
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                model_access: Some(AccountModelAccess::all()),
                ..command
            },
            &context,
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .load_provider_account("acct_models")
            .await
            .unwrap()
            .unwrap()
            .summary
            .model_access,
        AccountModelAccess::all()
    );
    database.close().await;
}
