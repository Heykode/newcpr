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
            account_search: Some("account@example.invalid".into()),
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
            record_status: Default::default(),
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
            record_status: Default::default(),
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
async fn usage_account_filter_matches_only_historical_names_and_emails() {
    use admin_observability::UsageRecordStatus::{All, Error, Success};
    let Some(database) = TestDatabase::create("usage_account_snapshot").await else {
        return;
    };
    let now = Utc::now();
    seed_observability_facts(&database.pool, now).await.unwrap();
    sqlx::query(
        "update model_requests set provider_account_email_snapshot = case when id = 'req_observe_success' then $1 end,
           provider_account_name_snapshot = case when id = 'req_observe_failed' then $2 end
         where id in ('req_observe_success', 'req_observe_failed')",
    )
    .bind(r"alice_%\ops@example.invalid")
    .bind("alice team")
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "update provider_accounts set custom_name = 'Current Alias' where id = 'acct_observe'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let range =
        ObservabilityRange::new(now - TimeDelta::hours(1), now + TimeDelta::hours(1)).unwrap();
    let repository = observability_repository(&database.pool);
    for deleted in [false, true] {
        if deleted {
            sqlx::query("delete from provider_accounts where id = 'acct_observe'")
                .execute(&database.pool)
                .await
                .unwrap();
            super::super::drain_analytics(&database.pool).await;
        }
        for (record_status, search, page, total, expected) in [
            (Success, r"_%\", 1, 1, Some("req_observe_success")),
            (Error(None), "team", 1, 1, Some("req_observe_failed")),
            (All, "alice", 1, 2, Some("req_observe_failed")),
            (All, "alice", 2, 2, Some("req_observe_success")),
            (Error(None), "@example", 1, 0, None),
            (All, "ALICE", 1, 0, None),
            (All, "alice%ops", 1, 0, None),
            (All, "Current Alias", 1, 0, None),
            (All, "acct_observe", 1, 0, None),
        ] {
            let filter = UsageRecordFilter {
                details: admin_observability::RequestSearchFilter {
                    account_search: Some(search.into()),
                    ..Default::default()
                },
                ..Default::default()
            };
            let records = repository
                .list_usage_records(UsageRecordQuery {
                    record_status,
                    range,
                    filter: filter.clone(),
                    current_page: page,
                    page_size: ObservabilityPageSize::new(1).unwrap(),
                })
                .await
                .unwrap();
            assert_eq!(
                records.total, total,
                "{record_status:?} {search} page {page}"
            );
            assert_eq!(records.items.first().map(|item| item.id.as_str()), expected);
            if record_status == All {
                let summary = repository.usage_summary(range, filter).await.unwrap();
                assert_eq!(summary.requests.request_count, total);
            }
        }
    }
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
                provider_account_name_snapshot = 'Final account',
                error_message = 'test blocked_%_body',
                diagnostic_trace_json = $1 where id = 'req_observe_failed'")
        .bind(serde_json::json!({"events": [
            {"stage":"attempt.failed", "attemptIndex":1, "data":{"diagnostic":{"stage":"connect"}}},
            {"stage":"attempt.failed", "attemptIndex":2, "data":{"diagnostic":{"stage":"read_body"}}}
        ]})).execute(&database.pool).await.unwrap();
    use admin_observability::RequestSearchFilter as F;
    for (scope, account, total) in [
        ("requests", "Final account", 1),
        ("events", "Final account", 0),
        ("events", "primary", 1),
        ("requests", "primary", 0),
    ] {
        let filter = F {
            error_scope: Some(scope.into()),
            account_search: Some(account.into()),
            ..Default::default()
        };
        assert_eq!(
            error_page(&database.pool, range, filter, None).await.total,
            total
        );
    }
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

#[tokio::test]
async fn error_origin_filters_matching_requests_before_counting_and_paging() {
    use gateway_admin::model::observability::{UsageErrorOrigin::*, UsageRecordStatus};

    let Some(database) = TestDatabase::create("usage_error_origin_pages").await else {
        return;
    };
    let start = Utc::now();
    let repository = admin_observability_store(&database.pool);
    let range = admin_observability::TimeRange::new(start, start + TimeDelta::hours(1)).unwrap();
    let saved =
        |body: serde_json::Value| serde_json::json!({"upstream": body.to_string()}).to_string();
    for (minute, (id, outcome, kind, status, details, model)) in [
        ("edge_0", "failed", "internal_error", None, None, "main"),
        (
            "edge_1",
            "failed",
            "internal_error",
            Some(503_i32),
            None,
            "main",
        ),
        ("edge_2", "incomplete", "transport", Some(200), None, "main"),
        (
            "edge_3",
            "incomplete",
            "invalid_request",
            None,
            Some("old plain text".into()),
            "main",
        ),
        (
            "edge_4",
            "failed",
            "internal_error",
            None,
            Some(saved(
                serde_json::json!({"response":{"error":{"message":"sample"}}}),
            )),
            "main",
        ),
        (
            "edge_5",
            "failed",
            "internal_error",
            None,
            Some(saved(
                serde_json::json!({"type":"websocket.close","code":1012}),
            )),
            "other",
        ),
        (
            "edge_6",
            "failed",
            "internal_error",
            None,
            Some(saved(
                serde_json::json!({"type":"websocket.close","code":65536}),
            )),
            "main",
        ),
        ("edge_7", "cancelled", "internal_error", None, None, "main"),
        ("edge_8", "succeeded", "internal_error", None, None, "main"),
    ]
    .into_iter()
    .enumerate()
    {
        super::super::analytics::seed(
            &database.pool,
            id,
            start + TimeDelta::minutes(minute as i64),
            &[],
        )
        .await;
        sqlx::query("update model_requests set outcome=$2, error_kind=$3, upstream_status_code=$4, raw_upstream_error=$5, requested_model_id=$6 where id=$1")
            .bind(id).bind(outcome).bind(kind).bind(status).bind(details).bind(model)
            .execute(&database.pool).await.unwrap();
    }
    for (origin, model, ids) in [
        (Proxy, None, vec!["edge_6", "edge_0"]),
        (Upstream, None, vec!["edge_5", "edge_4", "edge_1"]),
        (Connection, None, vec!["edge_2"]),
        (Unknown, None, vec!["edge_3"]),
        (Upstream, Some("main"), vec!["edge_4", "edge_1"]),
        (Proxy, Some("other"), vec![]),
    ] {
        for current_page in 1..=ids.len() + 1 {
            let page = repository
                .list_usage_records(admin_observability::UsageQuery {
                    range,
                    record_status: UsageRecordStatus::Error(Some(origin)),
                    filter: admin_observability::UsageFilter {
                        model: model.map(str::to_owned),
                        ..Default::default()
                    },
                    current_page: current_page as u32,
                    page_size: PageSize::new(1).unwrap(),
                })
                .await
                .unwrap();
            assert_eq!(page.total, ids.len() as u64, "{origin:?} / {model:?}");
            let items: Vec<_> = page
                .items
                .iter()
                .map(|item| (item.id.as_str(), item.error_origin()))
                .collect();
            let expected: Vec<_> = ids
                .get(current_page - 1)
                .map(|id| (*id, origin))
                .into_iter()
                .collect();
            assert_eq!(
                items, expected,
                "{origin:?} / {model:?} / page {current_page}"
            );
        }
    }
    database.close().await;
}
