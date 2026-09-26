use std::sync::Arc;

use super::*;
use gateway_core::account::ResponsesUpstream;

#[tokio::test]
async fn excel_auto_disable_403_is_atomic_fenced_and_preserves_other_account_fields() {
    let Some(database) = TestDatabase::create("excel_disable_403").await else {
        return;
    };
    let repository = Arc::new(PgProviderAccountRepository::new(database.pool.clone()));
    repository
        .insert_provider_account(account("acct_excel_403", "fixture"))
        .await
        .unwrap();
    let id = ProviderAccountId::new("acct_excel_403").unwrap();
    let initial = repository.get_account(&id).await.unwrap().unwrap();
    assert!(!initial.excel_auto_disable_on_403());
    assert!(!repository.disable_excel_on_403(&initial).await.unwrap());
    sqlx::query("update provider_accounts set responses_upstream='excel', excel_auto_disable_on_403=true, excel_cache_creation_as_input=true where id=$1")
        .bind(id.as_str()).execute(&database.pool).await.unwrap();
    let frozen = repository.get_account(&id).await.unwrap().unwrap();
    let before: serde_json::Value = sqlx::query_scalar("select to_jsonb(a)-array['responses_upstream','updated_at'] from provider_accounts a where id=$1")
        .bind(id.as_str()).fetch_one(&database.pool).await.unwrap();
    let revision: i64 =
        sqlx::query_scalar("select config_revision from runtime_settings where id=1")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let repository = repository.clone();
        let frozen = frozen.clone();
        tasks.spawn(async move { repository.disable_excel_on_403(&frozen).await.unwrap() });
    }
    let mut changes = 0;
    while let Some(result) = tasks.join_next().await {
        changes += usize::from(result.unwrap());
    }
    assert_eq!(changes, 1);
    let after: serde_json::Value = sqlx::query_scalar("select to_jsonb(a)-array['responses_upstream','updated_at'] from provider_accounts a where id=$1")
        .bind(id.as_str()).fetch_one(&database.pool).await.unwrap();
    assert_eq!(before, after);
    let next_revision: i64 =
        sqlx::query_scalar("select config_revision from runtime_settings where id=1")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(next_revision, revision + 1);
    let updated = repository.get_account(&id).await.unwrap().unwrap();
    assert_eq!(updated.responses_upstream(), ResponsesUpstream::Codex);
    assert!(updated.excel_auto_disable_on_403());
    assert!(updated.excel_cache_creation_as_input());
    sqlx::query("update provider_accounts set responses_upstream='excel', credential_revision=credential_revision+1 where id=$1")
        .bind(id.as_str()).execute(&database.pool).await.unwrap();
    assert!(!repository.disable_excel_on_403(&frozen).await.unwrap());
    let current = repository.get_account(&id).await.unwrap().unwrap();
    sqlx::query("update provider_accounts set excel_auto_disable_on_403=false where id=$1")
        .bind(id.as_str())
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(!repository.disable_excel_on_403(&current).await.unwrap());
    sqlx::query("update provider_accounts set excel_auto_disable_on_403=true where id=$1")
        .bind(id.as_str())
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(repository.disable_excel_on_403(&current).await.unwrap());
    sqlx::query("update provider_accounts set enabled=false where id=$1")
        .bind(id.as_str())
        .execute(&database.pool)
        .await
        .unwrap();
    repository.delete_account(&id).await.unwrap();
    assert!(!repository.disable_excel_on_403(&current).await.unwrap());
    database.close().await;
}

#[tokio::test]
async fn excel_global_models_resolve_without_rewriting_credentials_or_custom_lists() {
    let Some(database) = TestDatabase::create("excel_global_models").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    for id in ["acct_excel_global", "acct_excel_custom"] {
        repository
            .insert_provider_account(account(id, id))
            .await
            .unwrap();
    }
    let global = repository
        .load_provider_account("acct_excel_global")
        .await
        .unwrap()
        .unwrap();
    assert!(global.summary.excel_models_follow_global);
    assert!(!global.summary.excel_cache_creation_as_input);
    assert_eq!(
        global.summary.effective_excel_models.as_slice(),
        ["gpt-5.6-sol", "gpt-6-astra"]
    );
    let original_identity = (
        global.summary.credential_revision,
        global.summary.turn_state_binding_revision,
    );
    let store = admin_account_store(&database.pool);
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "excel-global".into(),
    };
    let patch = BatchUpdateAccounts {
        account_ids: vec!["acct_excel_global".into(), "acct_excel_custom".into()],
        responses_upstream: Some(ResponsesUpstream::Excel),
        excel_models_follow_global: None,
        excel_cache_creation_as_input: Some(true),
        excel_auto_disable_on_403: Some(true),
        excel_models: None,
        model_access: None,
        custom_name: None,
        enabled: None,
        turn_state_injection_enabled: None,
        concurrency_limit: None,
        weight: None,
        group_ids: None,
        outbound_proxy: None,
    };
    store
        .batch_update_accounts(patch.clone(), &context)
        .await
        .unwrap();
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                account_ids: vec!["acct_excel_custom".into()],
                excel_models: Some(
                    gateway_core::account::ExcelModels::try_from(vec!["custom-model".into()])
                        .unwrap(),
                ),
                ..patch.clone()
            },
            &context,
        )
        .await
        .unwrap();
    sqlx::query("update runtime_settings set excel_default_models = array['global-model']::text[] where id=1")
        .execute(&database.pool).await.unwrap();
    let global = repository
        .load_provider_account("acct_excel_global")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        global.summary.effective_excel_models.as_slice(),
        ["global-model"]
    );
    assert_eq!(
        (
            global.summary.credential_revision,
            global.summary.turn_state_binding_revision
        ),
        original_identity
    );
    let custom = repository
        .load_provider_account("acct_excel_custom")
        .await
        .unwrap()
        .unwrap();
    assert!(!custom.summary.excel_models_follow_global);
    assert_eq!(
        custom.summary.effective_excel_models.as_slice(),
        ["custom-model"]
    );
    let frozen = repository
        .get_account(&ProviderAccountId::new("acct_excel_custom").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        frozen.responses_upstream_for_model("custom-model"),
        ResponsesUpstream::Excel
    );
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                account_ids: vec!["acct_excel_custom".into()],
                excel_models_follow_global: Some(true),
                excel_cache_creation_as_input: Default::default(),
                excel_auto_disable_on_403: Default::default(),
                excel_models: Some(
                    gateway_core::account::ExcelModels::try_from(vec!["ignored-model".into()])
                        .unwrap(),
                ),
                ..patch.clone()
            },
            &context,
        )
        .await
        .unwrap();
    let current = repository
        .get_account(&ProviderAccountId::new("acct_excel_custom").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        current.responses_upstream_for_model("global-model"),
        ResponsesUpstream::Excel
    );
    assert_eq!(
        current.responses_upstream_for_model("custom-model"),
        ResponsesUpstream::Codex
    );
    assert_eq!(
        frozen.responses_upstream_for_model("custom-model"),
        ResponsesUpstream::Excel
    );
    let stored = repository
        .load_provider_account("acct_excel_custom")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.summary.excel_models.as_slice(), ["custom-model"]);
    repository
        .import_provider_accounts(ImportProviderAccounts {
            settings: None,
            outbound_proxy: None,
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".into(),
            },
            accounts: vec![account("acct_excel_custom", "acct_excel_custom")],
            audit: audit("audit_excel_reimport", "import", "acct_excel_custom"),
        })
        .await
        .unwrap();
    let reimported = repository
        .load_provider_account("acct_excel_custom")
        .await
        .unwrap()
        .unwrap();
    assert!(reimported.summary.excel_models_follow_global);
    assert!(reimported.summary.excel_cache_creation_as_input);
    assert!(reimported.summary.excel_auto_disable_on_403);
    assert_eq!(
        reimported.summary.responses_upstream,
        ResponsesUpstream::Excel
    );
    assert_eq!(reimported.summary.excel_models.as_slice(), ["custom-model"]);
    assert_eq!(
        reimported.summary.effective_excel_models.as_slice(),
        ["global-model"]
    );
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                account_ids: vec!["acct_excel_custom".into()],
                excel_models_follow_global: Some(false),
                excel_cache_creation_as_input: Default::default(),
                excel_auto_disable_on_403: Default::default(),
                excel_models: Some(
                    gateway_core::account::ExcelModels::try_from(Vec::new()).unwrap(),
                ),
                ..patch
            },
            &context,
        )
        .await
        .unwrap();
    assert!(
        repository
            .load_provider_account("acct_excel_custom")
            .await
            .unwrap()
            .unwrap()
            .summary
            .effective_excel_models
            .as_slice()
            .is_empty()
    );
    database.close().await;
}

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
        "select to_jsonb(a)-array['responses_upstream','excel_models','excel_models_follow_global','excel_cache_creation_as_input','excel_auto_disable_on_403','updated_at'] from provider_accounts a where id='acct_excel'",
    ).fetch_one(&database.pool).await.unwrap();
    let store = admin_account_store(&database.pool);
    let command = BatchUpdateAccounts {
        account_ids: vec!["acct_excel".into()],
        responses_upstream: Some(ResponsesUpstream::Excel),
        excel_models_follow_global: Default::default(),
        excel_cache_creation_as_input: Some(true),
        excel_auto_disable_on_403: Some(true),
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
        "select to_jsonb(a)-array['responses_upstream','excel_models','excel_models_follow_global','excel_cache_creation_as_input','excel_auto_disable_on_403','updated_at'] from provider_accounts a where id='acct_excel'",
    ).fetch_one(&database.pool).await.unwrap();
    assert_eq!(before, after);
    let loaded = repository
        .load_provider_account("acct_excel")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.summary.responses_upstream, ResponsesUpstream::Excel);
    assert!(!loaded.summary.excel_models_follow_global);
    assert!(loaded.summary.excel_cache_creation_as_input);
    assert_eq!(
        loaded.summary.excel_models.as_slice(),
        ["gpt-5.6-sol", "gpt-6-astra"]
    );
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                responses_upstream: None,
                excel_models_follow_global: Default::default(),
                excel_cache_creation_as_input: Default::default(),
                excel_auto_disable_on_403: Default::default(),
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
    assert!(configured.excel_cache_creation_as_input());
    assert!(configured.excel_auto_disable_on_403());
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
                excel_models_follow_global: Default::default(),
                excel_cache_creation_as_input: Default::default(),
                excel_auto_disable_on_403: Default::default(),
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
    assert!(
        !repository
            .load_provider_account("acct_excel")
            .await
            .unwrap()
            .unwrap()
            .summary
            .excel_cache_creation_as_input
    );
    assert!(
        !repository
            .load_provider_account("acct_excel")
            .await
            .unwrap()
            .unwrap()
            .summary
            .excel_auto_disable_on_403
    );
    database.close().await;
}
