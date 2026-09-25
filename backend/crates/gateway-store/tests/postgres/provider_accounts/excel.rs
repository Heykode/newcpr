use super::*;
use gateway_core::account::ResponsesUpstream;

#[tokio::test]
async fn excel_patch_is_account_local_preserves_credentials_and_omission() {
    let Some(database) = TestDatabase::create("excel_patch").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    repository
        .insert_provider_account(account("acct_excel", "owner"))
        .await
        .unwrap();
    let before: serde_json::Value = sqlx::query_scalar(
        "select to_jsonb(a)-array['responses_upstream','excel_models','updated_at'] from provider_accounts a where id='acct_excel'",
    ).fetch_one(&database.pool).await.unwrap();
    let store = admin_account_store(&database.pool);
    let command = BatchUpdateAccounts {
        account_ids: vec!["acct_excel".into()],
        responses_upstream: Some(ResponsesUpstream::Excel),
        excel_models: Some(
            gateway_core::account::ExcelModels::try_from(vec![
                "gpt-5.6-sol".into(),
                "gpt-6-astra".into(),
            ])
            .unwrap(),
        ),
        model_access: None,
        custom_name: None,
        enabled: None,
        turn_state_injection_enabled: None,
        concurrency_limit: None,
        weight: None,
        group_ids: None,
        outbound_proxy: None,
    };
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "excel-test".into(),
    };
    store
        .batch_update_accounts(command.clone(), &context)
        .await
        .unwrap();
    let after: serde_json::Value = sqlx::query_scalar(
        "select to_jsonb(a)-array['responses_upstream','excel_models','updated_at'] from provider_accounts a where id='acct_excel'",
    ).fetch_one(&database.pool).await.unwrap();
    assert_eq!(before, after);
    let loaded = repository
        .load_provider_account("acct_excel")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.summary.responses_upstream, ResponsesUpstream::Excel);
    assert_eq!(
        loaded.summary.excel_models.as_slice(),
        ["gpt-5.6-sol", "gpt-6-astra"]
    );
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                responses_upstream: None,
                excel_models: Default::default(),
                weight: Some(gateway_core::account::AccountWeight::new(2).unwrap()),
                ..command.clone()
            },
            &context,
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .get_account(&ProviderAccountId::new("acct_excel").unwrap())
            .await
            .unwrap()
            .unwrap()
            .responses_upstream(),
        ResponsesUpstream::Excel
    );
    let configured = repository
        .get_account(&ProviderAccountId::new("acct_excel").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        configured.responses_upstream_for_model("gpt-6-astra"),
        ResponsesUpstream::Excel
    );
    assert_eq!(
        configured.responses_upstream_for_model("gpt-5.4"),
        ResponsesUpstream::Codex
    );
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                responses_upstream: Some(ResponsesUpstream::Codex),
                excel_models: Default::default(),
                ..command
            },
            &context,
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .load_provider_account("acct_excel")
            .await
            .unwrap()
            .unwrap()
            .summary
            .responses_upstream,
        ResponsesUpstream::Codex
    );
    database.close().await;
}
