use super::*;

#[tokio::test]
async fn concurrency_sweeps_overlaps_boundaries_running_and_quality_filters() {
    let Some(database) = TestDatabase::create("concurrency_sweep").await else {
        return;
    };
    let start = chrono::DateTime::from_timestamp(1_700_000_100, 0).unwrap();
    seed_observability_facts(&database.pool, start - TimeDelta::days(1))
        .await
        .unwrap();
    let range = ObservabilityRange::new(start, start + TimeDelta::minutes(30)).unwrap();
    // First bucket: one request for all 900s + overlapping 450s.
    // Second: the boundary request ends as the running request starts, peak remains one.
    for (id, begin, end, quality) in [
        ("crossing", -60, Some(900), false),
        ("overlap", 0, Some(450), false),
        ("boundary", 900, Some(1350), false),
        ("running", 1350, None, false),
        ("zero", 450, Some(450), false),
        ("outside", 1800, Some(1801), false),
        ("quality", 0, Some(1800), true),
    ] {
        sqlx::query(
            "insert into model_requests
            select (jsonb_populate_record(null::model_requests, to_jsonb(mr) ||
                jsonb_build_object('id',$1::text,'started_at',$2::timestamptz,
                    'completed_at',$3::timestamptz,'deadline_at',$4::timestamptz,
                    'outcome',$5::text,'protocol',$6::text,'client_transport',$7::text,
                    'request_kind',$8::text))).*
            from model_requests mr where id='req_observe_uncommitted'",
        )
        .bind(id)
        .bind(start + TimeDelta::seconds(begin))
        .bind(end.map(|end| start + TimeDelta::seconds(end)))
        .bind(range.end + TimeDelta::hours(1))
        .bind(if end.is_some() {
            "succeeded"
        } else {
            "running"
        })
        .bind(if quality {
            "admin_quality_check"
        } else {
            "openai"
        })
        .bind(if quality { "internal" } else { "http_sse" })
        .bind(if quality {
            "account_quality_check"
        } else {
            "chat"
        })
        .execute(&database.pool)
        .await
        .unwrap();
    }
    let repository = observability_repository(&database.pool);
    let points = repository
        .usage_concurrency(range, UsageRecordFilter::default())
        .await
        .unwrap();
    assert_eq!(points.len(), 2);
    assert!((points[0].average_concurrency - 1.5).abs() < 1e-9);
    assert_eq!(points[0].peak_concurrency, 2);
    assert!((points[1].average_concurrency - 1.0).abs() < 1e-9);
    assert_eq!(points[1].peak_concurrency, 1);
    let partial = repository
        .usage_concurrency(
            ObservabilityRange::new(
                start + TimeDelta::seconds(450),
                start + TimeDelta::seconds(1350),
            )
            .unwrap(),
            UsageRecordFilter::default(),
        )
        .await
        .unwrap();
    assert_eq!(partial.len(), 2);
    assert!(
        partial
            .iter()
            .all(|point| (point.average_concurrency - 1.0).abs() < 1e-9)
    );
    let filtered = repository
        .usage_concurrency(
            range,
            UsageRecordFilter {
                client_api_key_ref: Some("missing".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        filtered
            .iter()
            .all(|point| point.average_concurrency == 0.0 && point.peak_concurrency == 0)
    );
    let future = repository
        .usage_concurrency(
            ObservabilityRange::new(
                Utc::now() + TimeDelta::hours(1),
                Utc::now() + TimeDelta::hours(2),
            )
            .unwrap(),
            UsageRecordFilter::default(),
        )
        .await
        .unwrap();
    assert!(
        future
            .iter()
            .all(|point| point.average_concurrency == 0.0 && point.peak_concurrency == 0)
    );
    database.close().await;
}
