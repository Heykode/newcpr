use super::*;
use gateway_store::postgres::{
    ModelRequestAttemptStart, ModelRequestRepository as _, NewModelRequest, PgExecutionStore,
};

fn admitted_request(id: &str, now: chrono::DateTime<Utc>) -> NewModelRequest {
    NewModelRequest {
        id: id.to_owned(),
        client_api_key_id: Some("key_history".to_owned()),
        client_api_key_ref: "key_history".to_owned(),
        config_revision: 1,
        routing_scope: "all".to_owned(),
        routing_group_refs: Vec::new(),
        routing_group_names_snapshot: serde_json::json!([]),
        protocol: "openai".to_owned(),
        operation: "responses".to_owned(),
        endpoint: "/v1/responses".to_owned(),
        client_transport: "http_sse".to_owned(),
        requested_model_id: Some("history-model".to_owned()),
        client_ip: None,
        user_agent: None,
        reasoning_effort: None,
        reasoning_preset: None,
        request_kind: None,
        subagent_kind: None,
        compact: false,
        continuation: Default::default(),
        image_generation_requested: false,
        admission_decision_ms: None,
        started_at: now,
        deadline_at: now + TimeDelta::minutes(1),
    }
}

#[tokio::test]
async fn admitted_key_names_survive_rename_delete_and_remain_searchable() {
    let Some(database) = TestDatabase::create("usage_history").await else {
        return;
    };
    let now = Utc::now();
    sqlx::query(
        "insert into client_api_keys (id, name, key, enabled, created_at, updated_at)
         values ('key_history', 'Original key', $1, true, $2, $2)",
    )
    .bind(format!("sk_{}", "K".repeat(43)))
    .bind(now)
    .execute(&database.pool)
    .await
    .unwrap();
    seed_observability_facts(&database.pool, now).await.unwrap();
    let execution = PgExecutionStore::new(database.pool.clone());
    execution
        .insert_model_request(admitted_request("req_history_plain", now))
        .await
        .unwrap();
    execution
        .insert_model_request(admitted_request("req_history_failure", now))
        .await
        .unwrap();
    execution
        .insert_model_request_with_first_attempt(
            admitted_request("req_history_merged", now),
            ModelRequestAttemptStart {
                model_request_id: "req_history_merged".to_owned(),
                attempt_count: 1,
                provider_kind: "openai".to_owned(),
                provider_account_id: Some("acct_observe".to_owned()),
                provider_account_ref: Some("acct_observe".to_owned()),
                upstream_model_id: Some("history-model".to_owned()),
                upstream_transport: "http_sse".to_owned(),
                http_version: None,
                account_selection_wait_ms: None,
                capacity_used_slots: None,
                capacity_total_slots: None,
            },
        )
        .await
        .unwrap();
    sqlx::query(
        "update model_requests set outcome = 'succeeded', client_status_code = 200,
           downstream_committed_at = $1, completed_at = $1
         where id in ('req_history_plain', 'req_history_merged')",
    )
    .bind(now)
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "update model_requests set outcome = 'failed', client_status_code = 502,
           upstream_status_code = 500, error_kind = 'upstream_unavailable',
           error_message = 'synthetic failure', completed_at = $1
         where id = 'req_history_failure'",
    )
    .bind(now)
    .execute(&database.pool)
    .await
    .unwrap();
    let range =
        ObservabilityRange::new(now - TimeDelta::hours(1), now + TimeDelta::hours(1)).unwrap();
    let repository = observability_repository(&database.pool);
    for deleted in [false, true] {
        if deleted {
            sqlx::query("delete from client_api_keys where id = 'key_history'")
                .execute(&database.pool)
                .await
                .unwrap();
        } else {
            sqlx::query("update client_api_keys set name = 'Renamed key' where id = 'key_history'")
                .execute(&database.pool)
                .await
                .unwrap();
        }
        for id in [
            "req_history_plain",
            "req_history_merged",
            "req_history_failure",
        ] {
            let detail = repository.usage_record_detail(id).await.unwrap();
            assert_eq!(
                detail.request.client_api_key_name.as_deref(),
                Some("Original key")
            );
        }
        let page = repository
            .list_usage_records(UsageRecordQuery {
                range,
                filter: UsageRecordFilter {
                    search: Some("Original key".to_owned()),
                    ..Default::default()
                },
                current_page: 1,
                page_size: ObservabilityPageSize::new(10).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(page.total, 2);
        assert!(
            page.items
                .iter()
                .all(|item| item.client_api_key_name.as_deref() == Some("Original key"))
        );
        let errors = repository
            .list_ops_errors(OpsErrorQuery {
                range,
                filter: OpsErrorFilter {
                    search: Some("Original key".to_owned()),
                    ..Default::default()
                },
                current_page: 1,
                page_size: ObservabilityPageSize::new(10).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(errors.total, 1);
        assert_eq!(
            errors.items[0].client_api_key_name.as_deref(),
            Some("Original key")
        );
        let diagnostics = repository
            .usage_diagnostics(
                range,
                UsageRecordFilter {
                    client_api_key_ref: Some("key_history".to_owned()),
                    ..Default::default()
                },
                DiagnosticDimension::ApiKey,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            diagnostics.items[0].name,
            if deleted {
                "Original key"
            } else {
                "Renamed key"
            }
        );
    }
    database.close().await;
}

#[tokio::test]
async fn usage_pagination_keeps_tie_order_filters_and_totals_from_both_ends() {
    let Some(database) = TestDatabase::create("usage_pages").await else {
        return;
    };
    let now = Utc::now();
    sqlx::query(
        "insert into model_requests (id, client_api_key_ref, config_revision, protocol,
           operation, endpoint, client_transport, requested_model_id, started_at,
           deadline_at, completed_at, downstream_committed_at, outcome, client_status_code, routing_scope)
         select 'req_page_' || lpad(n::text, 3, '0'), 'key_pages', 1, 'openai',
           'responses', '/v1/responses', 'http_sse', 'page-model', $1,
           $1 + interval '1 hour', $1, $1, 'succeeded', 200, 'all'
         from generate_series(1, 23) n",
    ).bind(now).execute(&database.pool).await.unwrap();
    sqlx::query(
        "update model_requests set request_kind = 'account_quality_check',
           protocol = 'admin_quality_check', client_transport = 'internal' where id = 'req_page_023'",
    ).execute(&database.pool).await.unwrap();
    sqlx::query(
        "update model_requests set downstream_committed_at = null where id = 'req_page_022'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let repository = observability_repository(&database.pool);
    let range =
        ObservabilityRange::new(now - TimeDelta::hours(1), now + TimeDelta::hours(1)).unwrap();
    let filter = UsageRecordFilter {
        client_api_key_ref: Some("key_pages".to_owned()),
        ..Default::default()
    };
    for (page_number, expected_numbers) in [
        (1, (15..=21).rev().collect::<Vec<_>>()),
        (2, (8..=14).rev().collect()),
        (3, (1..=7).rev().collect()),
        (4, vec![]),
    ] {
        let page = repository
            .list_usage_records(UsageRecordQuery {
                range,
                filter: filter.clone(),
                current_page: page_number,
                page_size: ObservabilityPageSize::new(7).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(page.total, 21);
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>(),
            expected_numbers
                .iter()
                .map(|number| format!("req_page_{number:03}"))
                .collect::<Vec<_>>()
        );
    }
    for search in ["req_page_001", "absent"] {
        let page = repository
            .list_usage_records(UsageRecordQuery {
                range,
                filter: UsageRecordFilter {
                    search: Some(search.to_owned()),
                    ..filter.clone()
                },
                current_page: 1,
                page_size: ObservabilityPageSize::new(7).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(page.total, u64::from(search == "req_page_001"));
        assert_eq!(page.items.len() as u64, page.total);
    }
    database.close().await;
}

#[tokio::test]
async fn usage_time_pages_match_exact_order_across_buckets_and_partial_ranges() {
    let Some(database) = TestDatabase::create("usage_time_pages").await else {
        return;
    };
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-14T10:00:15Z")
        .unwrap()
        .with_timezone(&Utc);
    sqlx::query(
        "insert into model_requests (id, client_api_key_ref, config_revision, protocol,
           operation, endpoint, client_transport, requested_model_id, started_at,
           deadline_at, completed_at, downstream_committed_at, outcome, client_status_code, routing_scope)
         select 'req_bucket_' || lpad(n::text, 3, '0'), 'key_buckets', 1, 'openai',
           'responses', '/v1/responses', 'http_sse',
           case when n % 5 = 0 then 'excluded-model' else 'bucket-model' end,
           $1 + (n / 3) * interval '25 seconds',
           $1 + interval '1 hour', $1 + interval '1 hour', $1, 'succeeded', 200, 'all'
         from generate_series(1, 83) n",
    )
    .bind(now)
    .execute(&database.pool)
    .await
    .unwrap();
    let repository = observability_repository(&database.pool);
    let range =
        ObservabilityRange::new(now + TimeDelta::seconds(37), now + TimeDelta::seconds(619))
            .unwrap();
    let expected: Vec<String> = sqlx::query_scalar(
        "select id from model_requests where started_at >= $1 and started_at < $2
           and requested_model_id = 'bucket-model' order by started_at desc, id desc",
    )
    .bind(range.start)
    .bind(range.end)
    .fetch_all(&database.pool)
    .await
    .unwrap();
    for page_size in [1_u16, 7, 19] {
        for page_number in 1..=(expected.len() / usize::from(page_size) + 3) {
            let page = repository
                .list_usage_records(UsageRecordQuery {
                    range,
                    filter: UsageRecordFilter {
                        model: Some("bucket-model".to_owned()),
                        search: Some("bucket_".to_owned()),
                        ..Default::default()
                    },
                    current_page: page_number as u32,
                    page_size: ObservabilityPageSize::new(page_size).unwrap(),
                })
                .await
                .unwrap();
            assert_eq!(page.total, expected.len() as u64);
            let offset = (page_number - 1) * usize::from(page_size);
            let expected_page: Vec<_> = expected
                .iter()
                .skip(offset)
                .take(usize::from(page_size))
                .cloned()
                .collect();
            assert_eq!(
                page.items
                    .into_iter()
                    .map(|item| item.id)
                    .collect::<Vec<_>>(),
                expected_page,
                "page {page_number}, size {page_size}"
            );
        }
    }
    database.close().await;
}

#[tokio::test]
async fn ops_time_pages_preserve_cross_source_ties_and_filters() {
    let Some(database) = TestDatabase::create("ops_time_pages").await else {
        return;
    };
    let now = Utc::now();
    seed_observability_facts(&database.pool, now).await.unwrap();
    sqlx::query(
        "insert into ops_events (id, model_request_id, attempt_index, level, component,
           operation, failure_kind, status_code, message, created_at)
         select 'ops_page_' || lpad(n::text, 3, '0'), 'req_observe_failed', 1,
           'warning', 'routing', 'responses', 'rate_limited', 429, 'page failure',
           $1 - interval '10 minutes' + (n / 3) * interval '30 seconds'
         from generate_series(1, 45) n",
    )
    .bind(now)
    .execute(&database.pool)
    .await
    .unwrap();
    let range = ObservabilityRange::new(now - TimeDelta::minutes(10) + TimeDelta::seconds(17), now)
        .unwrap();
    let expected: Vec<String> = sqlx::query_scalar(
        "select stable_id from (
           select 'model_request:' || id as stable_id, completed_at as occurred_at
           from model_requests where id = 'req_observe_failed'
           union all select 'ops_event:' || id, created_at from ops_events
           where model_request_id = 'req_observe_failed'
         ) events where occurred_at >= $1 and occurred_at < $2
         order by occurred_at desc, stable_id desc",
    )
    .bind(range.start)
    .bind(range.end)
    .fetch_all(&database.pool)
    .await
    .unwrap();
    let repository = observability_repository(&database.pool);
    for page_size in [1_u16, 7, 19] {
        for page_number in 1..=(expected.len() / usize::from(page_size) + 3) {
            let page = repository
                .list_ops_errors(OpsErrorQuery {
                    range,
                    filter: OpsErrorFilter {
                        request_id: Some("req_observe_failed".to_owned()),
                        status_code: Some(429),
                        ..Default::default()
                    },
                    current_page: page_number as u32,
                    page_size: ObservabilityPageSize::new(page_size).unwrap(),
                })
                .await
                .unwrap();
            assert_eq!(page.total, expected.len() as u64);
            let actual = page
                .items
                .iter()
                .map(|item| format!("{}:{}", item.source, item.event_id))
                .collect::<Vec<_>>();
            let expected_page = expected
                .iter()
                .skip((page_number - 1) * usize::from(page_size))
                .take(usize::from(page_size))
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(
                actual, expected_page,
                "page {page_number}, size {page_size}"
            );
        }
    }
    database.close().await;
}

#[tokio::test]
async fn history_upgrade_backfills_available_names_without_rewriting_billing_or_accounts() {
    let migrator = sqlx::migrate::Migrator {
        migrations: crate::postgres::TEST_MIGRATOR
            .iter()
            .filter(|migration| migration.version < 69)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let Some(database) =
        TestDatabase::create_with_migrator("usage_history_upgrade", &migrator).await
    else {
        return;
    };
    let now = Utc::now();
    sqlx::query(
        "insert into client_api_keys (id, name, key, enabled, created_at, updated_at)
        values ('key_observe', 'Legacy name', $1, true, $2, $2)",
    )
    .bind(format!("sk_{}", "K".repeat(43)))
    .bind(now)
    .execute(&database.pool)
    .await
    .unwrap();
    seed_observability_facts(&database.pool, now).await.unwrap();
    sqlx::query("update model_requests set client_api_key_ref = 'deleted_key' where id = 'req_observe_failed'")
        .execute(&database.pool).await.unwrap();
    let accounts: Vec<serde_json::Value> =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a order by id")
            .fetch_all(&database.pool)
            .await
            .unwrap();
    let costs: Vec<serde_json::Value> = sqlx::query_scalar("select to_jsonb(c) from account_cumulative_costs c order by provider_account_ref, currency")
        .fetch_all(&database.pool).await.unwrap();
    crate::postgres::TEST_MIGRATOR
        .run(&database.pool)
        .await
        .unwrap();
    assert_eq!(sqlx::query_scalar::<_, Option<String>>("select client_api_key_name_snapshot from model_requests where id = 'req_observe_success'")
        .fetch_one(&database.pool).await.unwrap().as_deref(), Some("Legacy name"));
    assert_eq!(sqlx::query_scalar::<_, Option<String>>("select client_api_key_name_snapshot from model_requests where id = 'req_observe_failed'")
        .fetch_one(&database.pool).await.unwrap(), None);
    assert_eq!(
        sqlx::query_scalar::<_, serde_json::Value>(
            "select to_jsonb(a) from provider_accounts a order by id"
        )
        .fetch_all(&database.pool)
        .await
        .unwrap(),
        accounts
    );
    assert_eq!(sqlx::query_scalar::<_, serde_json::Value>("select to_jsonb(c) from account_cumulative_costs c order by provider_account_ref, currency")
        .fetch_all(&database.pool).await.unwrap(), costs);
    let indexes: Vec<String> = sqlx::query_scalar(
        "select indexdef from pg_indexes where schemaname = current_schema()
           and indexname in ('model_requests_delivered_client_idx', 'model_requests_delivered_account_idx')",
    ).fetch_all(&database.pool).await.unwrap();
    assert_eq!(indexes.len(), 2);
    for index in indexes {
        assert!(index.contains("started_at, id"));
        assert!(index.contains("downstream_committed_at IS NOT NULL"));
        assert!(index.contains("account_quality_check"));
        assert!(index.contains("prewarm"));
    }
    database.close().await;
}
