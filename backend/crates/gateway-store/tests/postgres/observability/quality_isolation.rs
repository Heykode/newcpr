use super::*;

#[tokio::test]
async fn quality_history_is_isolated_from_all_business_projections_without_deleting_audit() {
    let Some(database) = TestDatabase::create("quality_log_isolation").await else {
        return;
    };
    let pool = &database.pool;
    let now = Utc::now();
    seed_observability_facts(pool, now).await.unwrap();
    seed_calculated_billing_facts(pool, now).await.unwrap();
    // Clients can supply request-kind metadata, never internal protocol/transport.
    sqlx::query("update model_requests set request_kind='account_quality_check' where id in ('req_observe_success','req_observe_failed')")
        .execute(pool).await.unwrap();
    // Unknown kinds and NULL legacy kinds remain business traffic. An unlinked
    // system failure must survive the NULL side of the error-query left join.
    sqlx::query("update model_requests set request_kind='future_request' where id='req_observe_uncommitted'")
        .execute(pool).await.unwrap();
    sqlx::query(
        "insert into ops_events(id,level,component,operation,failure_kind,message,created_at)
         values('ops_system_fixture','error','worker','poll','internal','fixture dependency failure',$1)",
    )
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
    let range =
        ObservabilityRange::new(now - TimeDelta::hours(1), now + TimeDelta::hours(1)).unwrap();
    let repository = observability_repository(pool);
    let dashboard = repository.dashboard_summary(range, now).await.unwrap();
    assert!(
        dashboard
            .recent_requests
            .iter()
            .any(|row| row.id == "req_observe_success")
    );
    let summary = repository
        .usage_summary(range, UsageRecordFilter::default())
        .await
        .unwrap();
    let trend = repository
        .usage_trend(range, UsageRecordFilter::default())
        .await
        .unwrap();
    let records_query = UsageRecordQuery {
        range,
        filter: UsageRecordFilter::default(),
        current_page: 1,
        page_size: ObservabilityPageSize::new(100).unwrap(),
    };
    let records = repository
        .list_usage_records(records_query.clone())
        .await
        .unwrap();
    let account_query = ProviderAccountUsageQuery::for_accounts(range, vec!["acct_observe".into()])
        .unwrap()
        .with_five_minute_request_buckets()
        .unwrap();
    let account_usage = repository
        .provider_account_usage(account_query.clone())
        .await
        .unwrap();
    let dimensions = [
        DiagnosticDimension::Provider,
        DiagnosticDimension::Model,
        DiagnosticDimension::KeyModel,
        DiagnosticDimension::Account,
        DiagnosticDimension::ApiKey,
        DiagnosticDimension::Transport,
        DiagnosticDimension::Failure,
        DiagnosticDimension::Status,
    ];
    let mut diagnostics = Vec::new();
    for dimension in dimensions {
        diagnostics.push(
            repository
                .usage_diagnostics(range, UsageRecordFilter::default(), dimension, None)
                .await
                .unwrap(),
        );
    }
    let error_query = OpsErrorQuery {
        range,
        filter: OpsErrorFilter::default(),
        current_page: 1,
        page_size: ObservabilityPageSize::new(100).unwrap(),
    };
    let errors = repository
        .list_ops_errors(error_query.clone())
        .await
        .unwrap();
    assert!(
        errors
            .items
            .iter()
            .any(|error| error.event_id == "ops_system_fixture")
    );
    assert!(
        errors
            .items
            .iter()
            .any(|error| error.event_id == "req_observe_failed")
    );
    let audit_count: i64 = sqlx::query_scalar("select count(*) from model_requests")
        .fetch_one(pool)
        .await
        .unwrap();
    let charged_count: i64 =
        sqlx::query_scalar("select count(*) from account_cumulative_cost_entries")
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(charged_count > 0);

    // Simulate existing quality successes and failures, including intermediate
    // errors written by older builds. All raw facts remain in the same tables.
    sqlx::query(
        "insert into model_requests
         select (jsonb_populate_record(null::model_requests,
             to_jsonb(mr) || jsonb_build_object(
                 'id','quality_' || mr.id,
                 'request_kind','account_quality_check',
                 'client_api_key_ref','admin_quality_check',
                 'client_api_key_id',null,
                 'protocol','admin_quality_check',
                 'endpoint','/api/admin/quality-ops',
                 'client_transport','internal'
             ))).* from model_requests mr",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into ops_events
         select (jsonb_populate_record(null::ops_events,
             to_jsonb(oe) || jsonb_build_object(
                 'id','quality_' || oe.id,
                 'model_request_id','quality_' || oe.model_request_id
             ))).* from ops_events oe where oe.model_request_id is not null",
    )
    .execute(pool)
    .await
    .unwrap();

    assert_eq!(
        repository.dashboard_summary(range, now).await.unwrap(),
        dashboard
    );
    assert_eq!(
        repository.dashboard_trend(range).await.unwrap(),
        dashboard.trend
    );
    assert_eq!(
        repository
            .usage_summary(range, UsageRecordFilter::default())
            .await
            .unwrap(),
        summary
    );
    assert_eq!(
        repository
            .usage_trend(range, UsageRecordFilter::default())
            .await
            .unwrap(),
        trend
    );
    assert_eq!(
        repository.list_usage_records(records_query).await.unwrap(),
        records
    );
    assert_eq!(
        repository
            .provider_account_usage(account_query)
            .await
            .unwrap(),
        account_usage
    );
    assert_eq!(
        repository
            .list_ops_errors(error_query.clone())
            .await
            .unwrap(),
        errors
    );
    for (dimension, before) in dimensions.into_iter().zip(diagnostics) {
        assert_eq!(
            repository
                .usage_diagnostics(range, UsageRecordFilter::default(), dimension, None,)
                .await
                .unwrap(),
            before,
            "diagnostic dimension {dimension:?}"
        );
    }
    // Pagination, explicit searches and source scopes must not reintroduce checks.
    for (index, expected) in errors.items.iter().enumerate() {
        let mut query = error_query.clone();
        query.current_page = u32::try_from(index).unwrap() + 1;
        query.page_size = ObservabilityPageSize::new(1).unwrap();
        let page = repository.list_ops_errors(query).await.unwrap();
        assert_eq!(page.total, errors.total);
        assert_eq!(page.items.as_slice(), std::slice::from_ref(expected));
    }
    for scope in [None, Some("requests"), Some("events")] {
        let mut query = error_query.clone();
        query.filter.details.error_scope = scope.map(str::to_owned);
        query.filter.request_id = Some("quality_req_observe_failed".into());
        let page = repository.list_ops_errors(query).await.unwrap();
        assert_eq!(page.total, 0);
        assert!(page.items.is_empty());
    }
    let quality_summary = repository
        .usage_summary(
            range,
            UsageRecordFilter {
                client_api_key_ref: Some("admin_quality_check".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(quality_summary.requests.request_count, 0);
    assert_eq!(quality_summary.attempts.attempt_count, 0);
    assert!(quality_summary.providers.is_empty());

    let retained: i64 = sqlx::query_scalar("select count(*) from model_requests")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(retained, audit_count * 2);
    let matched_charges: i64 = sqlx::query_scalar(
        "select count(*) from account_cumulative_cost_entries quality
         join account_cumulative_cost_entries business
           on quality.request_id = 'quality_' || business.request_id
          and quality.amount = business.amount
          and quality.currency = business.currency
          and quality.provider_account_ref = business.provider_account_ref",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        matched_charges, charged_count,
        "quality costs still reach the real ledger"
    );
    let detail = repository
        .usage_record_detail("quality_req_observe_success")
        .await
        .unwrap();
    assert_eq!(
        detail.request.request_kind.as_deref(),
        Some("account_quality_check")
    );
    assert_eq!(
        detail.request.cost_amount.as_ref().unwrap().as_str(),
        "1.25"
    );
    let failure = repository
        .usage_record_detail("quality_req_observe_failed")
        .await
        .unwrap();
    assert_eq!(failure.request.error_kind.as_deref(), Some("rate_limited"));
    database.close().await;
}
