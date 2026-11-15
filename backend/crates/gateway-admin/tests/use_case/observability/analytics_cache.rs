//! Official cache invalidation, coalescing and bounded-stale regressions.
use super::*;
use gateway_admin::model::observability::AnalyticsState;

#[tokio::test]
async fn usage_summary_cache_should_coalesce_identical_queries_and_invalidate_deleted_generation() {
    let range = observation_range(Utc::now());
    let store = Arc::new(FixtureObservabilityStore::new(range));
    store.set_dashboard_delay(StdDuration::from_millis(20));
    let services = observability_services(store.clone()).await;
    let service = services.observability();
    let filter = UsageFilter {
        client_api_key_ref: Some("deleted-key".to_owned()),
        ..Default::default()
    };
    let (first, second) = tokio::join!(
        service.usage_summary(range, filter.clone()),
        service.usage_summary(range, filter.clone()),
    );
    first.expect("first summary");
    second.expect("coalesced summary");
    assert_eq!(store.usage_summary_filters.lock().unwrap().len(), 1);
    {
        let mut overview = store.overview.lock().unwrap();
        overview.analytics.generation = 1;
        overview.requests.request_count = 7;
    }
    let refreshed = service
        .usage_summary(range, filter)
        .await
        .expect("generation refresh");
    assert_eq!(refreshed.overview.analytics.generation, 1);
    assert_eq!(refreshed.overview.requests.request_count, 7);
    assert_eq!(store.usage_summary_filters.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn usage_summary_cache_should_coalesce_when_generation_advances_during_the_first_load() {
    let range = observation_range(Utc::now());
    let store = Arc::new(FixtureObservabilityStore::new(range));
    store.set_dashboard_delay(StdDuration::from_millis(20));
    let services = observability_services(store.clone()).await;
    let service = services.observability();
    let (first, second, ()) = tokio::join!(
        service.usage_summary(range, UsageFilter::default()),
        service.usage_summary(range, UsageFilter::default()),
        async {
            tokio::time::sleep(StdDuration::from_millis(5)).await;
            store.overview.lock().unwrap().analytics.generation = 1;
        },
    );
    assert_eq!(
        first.expect("first summary").overview.analytics.generation,
        1
    );
    assert_eq!(
        second
            .expect("coalesced summary")
            .overview
            .analytics
            .generation,
        1
    );
    assert_eq!(store.usage_summary_filters.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn usage_summary_cache_should_isolate_key_identity_and_filters() {
    let range = observation_range(Utc::now());
    let store = Arc::new(FixtureObservabilityStore::new(range));
    let services = observability_services(store.clone()).await;
    let service = services.observability();
    for (key, model) in [
        ("key-a", None),
        ("key-a", None),
        ("key-b", None),
        ("key-a", Some("model-a")),
    ] {
        service
            .usage_summary(
                range,
                UsageFilter {
                    client_api_key_ref: Some(key.to_owned()),
                    model: model.map(str::to_owned),
                    ..Default::default()
                },
            )
            .await
            .expect("isolated summary");
    }
    let calls = store.usage_summary_filters.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].client_api_key_ref.as_deref(), Some("key-a"));
    assert_eq!(calls[1].client_api_key_ref.as_deref(), Some("key-b"));
    assert_eq!(calls[2].model.as_deref(), Some("model-a"));
}

#[tokio::test(start_paused = true)]
async fn usage_summary_cache_should_bound_waiting_after_failed_refresh_and_allow_recovery() {
    let range = observation_range(Utc::now());
    let store = Arc::new(FixtureObservabilityStore::new(range));
    store.set_dashboard_delay(store.query_timeout().mul_f64(0.7));
    store.summary_fail.store(true, Ordering::SeqCst);
    let services = observability_services(store.clone()).await;
    let service = services.observability();
    let started = tokio::time::Instant::now();
    let (first, second, third, fourth) = tokio::join!(
        service.usage_summary(range, UsageFilter::default()),
        service.usage_summary(range, UsageFilter::default()),
        service.usage_summary(range, UsageFilter::default()),
        service.usage_summary(range, UsageFilter::default()),
    );
    assert!(first.is_err() && second.is_err() && third.is_err() && fourth.is_err());
    assert!(
        started.elapsed() <= store.query_timeout(),
        "同键失败不能使后续请求逐个等待完整数据库超时: {:?}",
        started.elapsed()
    );

    store.summary_fail.store(false, Ordering::SeqCst);
    store.set_dashboard_delay(StdDuration::ZERO);
    service
        .usage_summary(range, UsageFilter::default())
        .await
        .expect("cancelled cache refresh releases its slot for recovery");
}

#[tokio::test(start_paused = true)]
async fn insights_cache_should_bound_stale_lifetime_without_renewing_failed_refreshes() {
    let range = observation_range(Utc::now());
    let store = Arc::new(FixtureObservabilityStore::new(range));
    {
        let mut overview = store.overview.lock().unwrap();
        overview.analytics.state = AnalyticsState::Ready;
        overview.analytics.generated_at = Utc::now();
        overview.analytics.lag_seconds = 7;
        overview.requests.request_count = 19;
    }
    let services = observability_services(store.clone()).await;
    let service = services.observability();
    service
        .usage_insights(range, UsageFilter::default())
        .await
        .unwrap();
    store.insights_fail.store(true, Ordering::SeqCst);
    tokio::time::advance(StdDuration::from_secs(16)).await;
    let stale = service
        .usage_insights(range, UsageFilter::default())
        .await
        .unwrap();
    assert_eq!(stale.analytics.state, AnalyticsState::Stale);
    assert!(stale.analytics.lag_seconds >= 7);
    assert_eq!(stale.health.total_requests, 19);
    tokio::time::advance(StdDuration::from_secs(45)).await;
    assert!(
        service
            .usage_insights(range, UsageFilter::default())
            .await
            .is_err()
    );
}

#[tokio::test(start_paused = true)]
async fn insights_cache_should_reject_deleted_building_and_old_generation_times() {
    for reason in ["deleted", "building", "generated-too-long-ago"] {
        let range = observation_range(Utc::now());
        let store = Arc::new(FixtureObservabilityStore::new(range));
        {
            let mut overview = store.overview.lock().unwrap();
            overview.analytics.state = if reason == "building" {
                AnalyticsState::Building
            } else {
                AnalyticsState::Ready
            };
            overview.analytics.generated_at = Utc::now()
                - Duration::seconds(if reason == "generated-too-long-ago" {
                    61
                } else {
                    0
                });
        }
        let services = observability_services(store.clone()).await;
        let service = services.observability();
        service
            .usage_insights(range, UsageFilter::default())
            .await
            .unwrap();
        store.insights_fail.store(true, Ordering::SeqCst);
        if reason == "deleted" {
            store.overview.lock().unwrap().analytics.generation += 1;
        }
        tokio::time::advance(StdDuration::from_secs(16)).await;
        assert!(
            service
                .usage_insights(range, UsageFilter::default())
                .await
                .is_err(),
            "{reason}"
        );
    }
}
