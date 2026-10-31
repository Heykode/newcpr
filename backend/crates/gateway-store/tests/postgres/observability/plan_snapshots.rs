use super::*;

#[tokio::test]
async fn diagnostics_and_logs_keep_historical_plan_after_account_deletion() {
    let Some(database) = TestDatabase::create("diagnostic_plan_history").await else {
        return;
    };
    let now = Utc::now();
    seed_observability_facts(&database.pool, now).await.unwrap();
    let repository = observability_repository(&database.pool);
    let range = ObservabilityRange::new(now - TimeDelta::hours(1), now).unwrap();
    for mutation in [
        "update provider_accounts set plan_type='free', name='Changed', email='changed@example.invalid' where id='acct_observe'",
        "delete from provider_accounts where id='acct_observe'",
    ] {
        sqlx::query(mutation).execute(&database.pool).await.unwrap();
        let items = repository
            .usage_diagnostics(
                range,
                UsageRecordFilter::default(),
                DiagnosticDimension::Account,
                None,
            )
            .await
            .unwrap()
            .items;
        assert_eq!(items[0].name, "account@example.invalid");
        assert_eq!(items[0].account_provider_kind.as_deref(), Some("openai"));
        assert_eq!(
            items[0].account_authentication_kind.as_deref(),
            Some("oauth")
        );
        assert_eq!(items[0].account_plan_type.as_deref(), Some("pro"));
        let errors = repository
            .list_ops_errors(OpsErrorQuery {
                range,
                filter: OpsErrorFilter::default(),
                current_page: 1,
                page_size: ObservabilityPageSize::new(10).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(errors.items.len(), 2);
        assert!(
            errors
                .items
                .iter()
                .all(|item| item.provider_account_plan_type.as_deref() == Some("pro"))
        );
    }
    // Missing fields on the newest request cannot borrow older facts.
    sqlx::query("update model_requests set provider_account_plan_type_snapshot=null,
                provider_account_authentication_kind_snapshot=null,
                provider_account_email_snapshot=null, provider_account_name_snapshot='Latest'
                where id=(select id from model_requests where provider_account_ref='acct_observe' order by started_at desc,id desc limit 1)")
        .execute(&database.pool).await.unwrap();
    let items = repository
        .usage_diagnostics(
            range,
            UsageRecordFilter::default(),
            DiagnosticDimension::Account,
            None,
        )
        .await
        .unwrap()
        .items;
    assert_eq!(items[0].name, "Latest");
    assert!(items[0].account_plan_type.is_none());
    assert!(items[0].account_authentication_kind.is_none());
    let items = repository
        .usage_diagnostics(
            range,
            UsageRecordFilter::default(),
            DiagnosticDimension::ApiKey,
            None,
        )
        .await
        .unwrap()
        .items;
    assert!(
        items
            .iter()
            .all(|item| item.account_plan_type.is_none()
                && item.account_authentication_kind.is_none())
    );
    database.close().await;
}
