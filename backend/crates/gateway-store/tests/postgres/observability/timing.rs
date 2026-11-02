use super::*;
use serde_json::{Value, json};

#[tokio::test]
async fn upstream_tps_projects_existing_observations_and_rejects_invalid_clocks() {
    let Some(database) = TestDatabase::create("upstream_tps_projection").await else {
        return;
    };
    let now = Utc::now();
    seed_observability_facts(&database.pool, now).await.unwrap();
    let invalid_document = sqlx::query("update model_requests set provider_observation_json = 'null'::jsonb where id = 'req_observe_success'")
        .execute(&database.pool).await.expect_err("top-level observation must remain an object or SQL NULL");
    assert_eq!(
        invalid_document
            .as_database_error()
            .and_then(|error| error.constraint()),
        Some("model_requests_provider_observation_ck")
    );
    sqlx::query("update model_requests set output_tokens = 100, latency_ms = 2000, first_token_ms = 1000 where id = 'req_observe_success'")
        .execute(&database.pool).await.unwrap();
    let repository = observability_repository(&database.pool);
    let store = admin_observability_store(&database.pool);
    let range =
        ObservabilityRange::new(now - TimeDelta::hours(1), now + TimeDelta::hours(1)).unwrap();
    let filter = UsageRecordFilter {
        request_id: Some("req_observe_success".to_owned()),
        ..Default::default()
    };
    let mut cases: Vec<(Option<Value>, Option<u64>)> = vec![
        (None, None),
        (Some(json!({})), None),
        (Some(json!({"upstreamTimingMetrics": null})), None),
        (Some(json!({"upstreamTimingMetrics": []})), None),
        (
            Some(json!({"upstreamTimingMetrics": {"upstreamEngineMs": 400}})),
            None,
        ),
    ];
    for (value, expected) in [
        (json!(null), None),
        (json!(0), None),
        (json!(-1), None),
        (json!(0.5), None),
        (json!("400"), None),
        (json!("invalid"), None),
        (json!(true), None),
        (json!([400]), None),
        (json!({}), None),
        (json!(u64::MAX), None),
        (json!(1e100), None),
        (json!(400), Some(400)),
        (json!(2000), Some(2000)),
        (json!(4000), Some(4000)),
    ] {
        cases.push((
            Some(json!({"upstreamTimingMetrics": {"upstreamResponseMs": value}})),
            expected,
        ));
    }
    for (observation, expected) in cases {
        sqlx::query("update model_requests set provider_observation_json = $1 where id = 'req_observe_success'")
            .bind(&observation).execute(&database.pool).await.unwrap();
        let page = repository
            .list_usage_records(UsageRecordQuery {
                record_status: Default::default(),
                range,
                filter: filter.clone(),
                current_page: 1,
                page_size: ObservabilityPageSize::new(10).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(
            page.items[0].upstream_response_ms, expected,
            "{observation:?}"
        );
        assert_eq!(page.items[0].latency_ms, Some(2000));
        assert_eq!(page.items[0].output_tokens, Some(100));
        let detail = repository
            .usage_record_detail("req_observe_success")
            .await
            .unwrap();
        assert_eq!(detail.request.upstream_response_ms, expected);
        let admin_detail = store
            .usage_record_detail("req_observe_success")
            .await
            .unwrap();
        assert_eq!(admin_detail.request.upstream_response_ms, expected);
        let summary = repository
            .usage_summary(range, filter.clone())
            .await
            .unwrap();
        let tps = expected.map(|ms| 100_000 / ms);
        assert_eq!(summary.requests.output_throughput_p10, tps);
        assert_eq!(summary.requests.output_throughput_p50, tps);
        assert_eq!(summary.requests.output_throughput_p90, tps);
        assert_eq!(summary.requests.output_tokens, 100);
    }
    database.close().await;
}

#[tokio::test]
async fn upstream_tps_percentiles_share_only_measured_completed_responses() {
    let Some(database) = TestDatabase::create("upstream_tps_percentiles").await else {
        return;
    };
    let now = Utc::now();
    seed_observability_facts(&database.pool, now).await.unwrap();
    sqlx::query("update model_requests set output_tokens = 100, provider_observation_json = '{\"upstreamTimingMetrics\":{\"upstreamResponseMs\":1000}}'::jsonb where id = 'req_observe_success'")
        .execute(&database.pool).await.unwrap();
    for (id, duration) in [
        ("tps_250", Some(400_i64)),
        ("tps_500", Some(200)),
        ("tps_unknown", None),
    ] {
        sqlx::query("insert into model_requests select (jsonb_populate_record(null::model_requests, to_jsonb(mr) || jsonb_build_object('id', $1::text, 'provider_observation_json', jsonb_build_object('upstreamTimingMetrics', jsonb_build_object('upstreamResponseMs', $2::bigint))))).* from model_requests mr where id = 'req_observe_success'")
            .bind(id).bind(duration).execute(&database.pool).await.unwrap();
    }
    let range =
        ObservabilityRange::new(now - TimeDelta::hours(1), now + TimeDelta::hours(1)).unwrap();
    let repository = observability_repository(&database.pool);
    let summary = repository
        .usage_summary(range, UsageRecordFilter::default())
        .await
        .unwrap();
    assert_eq!(summary.requests.output_throughput_p10, Some(130));
    assert_eq!(summary.requests.output_throughput_p50, Some(250));
    assert_eq!(summary.requests.output_throughput_p90, Some(450));
    assert_eq!(summary.requests.output_tokens, 400);
    let dashboard = repository.dashboard_summary(range, now).await.unwrap();
    let measured: Vec<_> = dashboard
        .trend
        .iter()
        .filter(|point| point.metrics.output_throughput_p50.is_some())
        .collect();
    assert_eq!(measured.len(), 1);
    assert_eq!(measured[0].metrics.output_throughput_p10, Some(130));
    assert_eq!(measured[0].metrics.output_throughput_p50, Some(250));
    assert_eq!(measured[0].metrics.output_throughput_p90, Some(450));
    database.close().await;
}
