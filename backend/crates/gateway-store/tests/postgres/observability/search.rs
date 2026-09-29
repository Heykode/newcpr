use super::*;

async fn error_page(
    pool: &PgPool,
    range: ObservabilityRange,
    details: admin_observability::RequestSearchFilter,
    search: Option<&str>,
) -> gateway_store::postgres::OpsErrorPage {
    observability_repository(pool)
        .list_ops_errors(OpsErrorQuery {
            range,
            filter: OpsErrorFilter {
                details,
                search: search.map(str::to_owned),
                ..Default::default()
            },
            current_page: 1,
            page_size: ObservabilityPageSize::new(100).unwrap(),
        })
        .await
        .expect("filtered errors")
}

#[tokio::test]
async fn usage_search_shared_filters_match_real_facts_and_analytics() {
    let Some(database) = TestDatabase::create("usage_shared_filters").await else {
        return;
    };
    let now = Utc::now();
    seed_observability_facts(&database.pool, now).await.unwrap();
    sqlx::query(
        "update provider_accounts set custom_name = 'literal_%_name' where id = 'acct_observe'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query("update model_requests set upstream_transport = 'excel_http_sse' where id = 'req_observe_success'")
        .execute(&database.pool).await.unwrap();
    let range =
        ObservabilityRange::new(now - TimeDelta::hours(1), now + TimeDelta::hours(1)).unwrap();
    let filter = UsageRecordFilter {
        operation: Some("/v1/responses".into()),
        details: admin_observability::RequestSearchFilter {
            account_ids: vec!["acct_observe".into(), "missing".into()],
            account_search: Some("literal_%".into()),
            group_id: Some("grp_history".into()),
            requested_model: Some("public-model".into()),
            upstream_model: Some("upstream-model".into()),
            upstream_mode: Some("excel".into()),
            upstream_transport: Some("http_sse".into()),
            client_status_code: Some(200),
            upstream_status_code: Some(200),
            cache_match: Some("hit".into()),
            min_first_token_ms: Some(100),
            max_latency_ms: Some(1_000),
            ..Default::default()
        },
        search: Some("LITERAL_%".into()),
        ..Default::default()
    };
    let repository = observability_repository(&database.pool);
    let page = repository
        .list_usage_records(UsageRecordQuery {
            range,
            filter: filter.clone(),
            current_page: 1,
            page_size: ObservabilityPageSize::new(1).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].id, "req_observe_success");
    let second = repository
        .list_usage_records(UsageRecordQuery {
            range,
            filter: filter.clone(),
            current_page: 2,
            page_size: ObservabilityPageSize::new(1).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(second.total, 1);
    assert!(second.items.is_empty());
    let summary = repository
        .usage_summary(range, filter.clone())
        .await
        .unwrap();
    assert_eq!(summary.requests.request_count, 1);
    let no_match = UsageRecordFilter {
        search: Some("literalXXname".into()),
        ..filter.clone()
    };
    assert_eq!(
        repository
            .usage_summary(range, no_match)
            .await
            .unwrap()
            .requests
            .request_count,
        0
    );

    let time = admin_observability::TimeRange::new(range.start, range.end).unwrap();
    let options = admin_observability_store(&database.pool)
        .account_filter_options(time, "literal_%")
        .await
        .unwrap();
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].id, "acct_observe");
    assert!(!options[0].deleted);
    sqlx::query("delete from provider_accounts where id = 'acct_observe'")
        .execute(&database.pool)
        .await
        .unwrap();
    let options = admin_observability_store(&database.pool)
        .account_filter_options(time, "account@example.invalid")
        .await
        .unwrap();
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].id, "acct_observe");
    assert!(options[0].deleted);
    database.close().await;
}

#[tokio::test]
async fn usage_search_error_filters_separate_attempt_and_final_request_evidence() {
    let Some(database) = TestDatabase::create("usage_error_filters").await else {
        return;
    };
    let now = Utc::now();
    seed_observability_facts(&database.pool, now).await.unwrap();
    let range =
        ObservabilityRange::new(now - TimeDelta::hours(1), now + TimeDelta::hours(1)).unwrap();
    sqlx::query("update model_requests set upstream_status_code = 403, upstream_transport = 'excel_http_sse',
                error_message = 'test blocked_%_body',
                diagnostic_trace_json = $1 where id = 'req_observe_failed'")
        .bind(serde_json::json!({"events": [
            {"stage":"attempt.failed", "attemptIndex":1, "data":{"diagnostic":{"stage":"connect"}}},
            {"stage":"attempt.failed", "attemptIndex":2, "data":{"diagnostic":{"stage":"read_body"}}}
        ]})).execute(&database.pool).await.unwrap();
    use admin_observability::RequestSearchFilter as F;
    let p = error_page(
        &database.pool,
        range,
        F {
            client_status_code: Some(502),
            upstream_status_code: Some(403),
            ..Default::default()
        },
        None,
    )
    .await;
    assert_eq!(p.total, 1);
    assert_eq!(p.items.len(), 1);
    for (route, expected) in [("/v1/responses", 2), ("/v1/chat/completions", 0)] {
        let page = observability_repository(&database.pool)
            .list_ops_errors(OpsErrorQuery {
                range,
                filter: OpsErrorFilter {
                    operation: Some(route.into()),
                    ..Default::default()
                },
                current_page: 1,
                page_size: ObservabilityPageSize::new(10).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(page.total, expected);
    }
    let p = error_page(
        &database.pool,
        range,
        F {
            client_status_code: Some(403),
            upstream_status_code: Some(502),
            ..Default::default()
        },
        None,
    )
    .await;
    assert_eq!(p.total, 0);
    for text in ["blocked_%", "ACCOUNT@EXAMPLE.INVALID"] {
        let p = error_page(
            &database.pool,
            range,
            F {
                error_scope: Some("requests".into()),
                ..Default::default()
            },
            Some(text),
        )
        .await;
        assert_eq!(p.total, 1);
    }
    for (filter, count) in [
        (
            F {
                upstream_mode: Some("excel".into()),
                ..Default::default()
            },
            1,
        ),
        (
            F {
                upstream_mode: Some("codex".into()),
                ..Default::default()
            },
            0,
        ),
        (
            F {
                error_scope: Some("events".into()),
                upstream_mode: Some("excel".into()),
                ..Default::default()
            },
            0,
        ),
        (
            F {
                error_scope: Some("events".into()),
                upstream_transport: Some("unknown".into()),
                ..Default::default()
            },
            1,
        ),
        (
            F {
                error_scope: Some("requests".into()),
                error_phase: Some("connect".into()),
                ..Default::default()
            },
            0,
        ),
        (
            F {
                error_scope: Some("requests".into()),
                error_phase: Some("read_body".into()),
                ..Default::default()
            },
            1,
        ),
        (
            F {
                error_scope: Some("events".into()),
                error_phase: Some("connect".into()),
                ..Default::default()
            },
            1,
        ),
        (
            F {
                recovery: Some("recovered".into()),
                ..Default::default()
            },
            0,
        ),
        (
            F {
                recovery: Some("unrecovered".into()),
                ..Default::default()
            },
            2,
        ),
        (
            F {
                client_ip: Some("203.0.113.9".into()),
                ..Default::default()
            },
            2,
        ),
    ] {
        let page = error_page(&database.pool, range, filter.clone(), None).await;
        assert_eq!(page.total, count, "{filter:?}");
        assert_eq!(page.items.len() as u64, count);
    }
    database.close().await;
}
