use super::*;
use gateway_core::account::ResponsesUpstream;

#[tokio::test]
async fn request_proxy_source_roundtrip_preserves_native_egress_and_identity() {
    use gateway_core::provider_ports::session_proxy::RequestProxySource;
    let Some(database) = TestDatabase::create("excel_exit").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    repository
        .insert_provider_account(account("acct_excel_exit", "fixture"))
        .await
        .unwrap();
    let id = ProviderAccountId::new("acct_excel_exit").unwrap();
    assert_eq!(
        repository
            .get_account(&id)
            .await
            .unwrap()
            .unwrap()
            .request_proxy_source(),
        RequestProxySource::Account
    );
    let before: serde_json::Value = sqlx::query_scalar("select to_jsonb(a)-array['request_proxy_source','responses_upstream','updated_at'] from provider_accounts a where id=$1")
        .bind(id.as_str()).fetch_one(&database.pool).await.unwrap();
    let store = admin_account_store(&database.pool);
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "excel-exit-regression".into(),
    };
    let patch = BatchUpdateAccounts {
        credential_proxy_id: None,
        quota_settings: Default::default(),
        auto_use_quota_credits: None,
        purchase_cost: None,
        account_ids: vec![id.as_str().into()],
        request_proxy_source: Some(RequestProxySource::Mihomo),
        egress_mode: None,
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
        .batch_update_accounts(patch.clone(), Default::default(), &context)
        .await
        .unwrap();
    assert_eq!(
        repository
            .get_account(&id)
            .await
            .unwrap()
            .unwrap()
            .responses_upstream(),
        ResponsesUpstream::Codex
    );
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                auto_use_quota_credits: None,
                purchase_cost: None,
                request_proxy_source: None,
                ..patch.clone()
            },
            Default::default(),
            &context,
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .get_account(&id)
            .await
            .unwrap()
            .unwrap()
            .request_proxy_source(),
        RequestProxySource::Mihomo
    );
    assert_eq!(
        repository
            .load_provider_account(id.as_str())
            .await
            .unwrap()
            .unwrap()
            .summary
            .request_proxy_source,
        RequestProxySource::Mihomo
    );
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                auto_use_quota_credits: None,
                purchase_cost: None,
                request_proxy_source: None,
                ..patch.clone()
            },
            Default::default(),
            &context,
        )
        .await
        .unwrap();
    let restored = repository.get_account(&id).await.unwrap().unwrap();
    assert_eq!(restored.responses_upstream(), ResponsesUpstream::Codex);
    assert_eq!(restored.request_proxy_source(), RequestProxySource::Mihomo);
    let after: serde_json::Value = sqlx::query_scalar("select to_jsonb(a)-array['request_proxy_source','responses_upstream','updated_at'] from provider_accounts a where id=$1")
        .bind(id.as_str()).fetch_one(&database.pool).await.unwrap();
    assert_eq!(before, after);
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                auto_use_quota_credits: None,
                purchase_cost: None,
                request_proxy_source: Some(RequestProxySource::ProxyPool),
                ..patch
            },
            Default::default(),
            &context,
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .get_account(&id)
            .await
            .unwrap()
            .unwrap()
            .request_proxy_source(),
        RequestProxySource::ProxyPool
    );
    assert!(
        sqlx::query("update provider_accounts set request_proxy_source='invalid' where id=$1")
            .bind(id.as_str())
            .execute(&database.pool)
            .await
            .is_err()
    );
    // Manual enable still acknowledges a historical independent pause. Ordinary
    // edits must not clear it, and clearing it must not change any native setting.
    sqlx::query(
        "update provider_accounts set enabled=false,excel_auto_disabled_at=now() where id=$1",
    )
    .bind(id.as_str())
    .execute(&database.pool)
    .await
    .unwrap();
    let legacy_before: serde_json::Value = sqlx::query_scalar("select to_jsonb(a)-array['enabled','excel_auto_disabled_at','updated_at'] from provider_accounts a where id=$1")
        .bind(id.as_str()).fetch_one(&database.pool).await.unwrap();
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                account_ids: vec![id.as_str().into()],
                enabled: Some(true),
                credential_proxy_id: None,
                quota_settings: Default::default(),
                auto_use_quota_credits: None,
                purchase_cost: None,
                request_proxy_source: None,
                egress_mode: None,
                model_access: None,
                custom_name: None,
                turn_state_injection_enabled: None,
                concurrency_limit: None,
                weight: None,
                group_ids: None,
                outbound_proxy: None,
            },
            Default::default(),
            &context,
        )
        .await
        .unwrap();
    let cleared: bool = sqlx::query_scalar(
        "select enabled and excel_auto_disabled_at is null from provider_accounts where id=$1",
    )
    .bind(id.as_str())
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(cleared);
    let legacy_after: serde_json::Value = sqlx::query_scalar("select to_jsonb(a)-array['enabled','excel_auto_disabled_at','updated_at'] from provider_accounts a where id=$1")
        .bind(id.as_str()).fetch_one(&database.pool).await.unwrap();
    assert_eq!(legacy_before, legacy_after);
    database.close().await;
}
