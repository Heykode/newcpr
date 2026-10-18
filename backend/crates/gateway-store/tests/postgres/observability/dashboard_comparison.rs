use super::*;

#[tokio::test]
async fn dashboard_comparison_counters_observe_boundaries_and_current_business_facts() {
    let Some(db) = TestDatabase::create("dashboard_comparison").await else {
        return;
    };
    let end = chrono::DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap();
    let start = end - TimeDelta::days(1);
    seed_observability_facts(&db.pool, end).await.unwrap();
    sqlx::query("update model_requests set started_at=$1, completed_at=$1")
        .bind(end)
        .execute(&db.pool)
        .await
        .unwrap();
    let range = admin_observability::TimeRange::new(start, end).unwrap();
    let store = admin_observability_store(&db.pool);
    assert_eq!(
        store
            .dashboard_card_metrics(range)
            .await
            .unwrap()
            .request_count,
        0
    );
    sqlx::query("update model_requests set started_at=$1 where id='req_observe_success'")
        .bind(start)
        .execute(&db.pool)
        .await
        .unwrap();
    let first = store.dashboard_card_metrics(range).await.unwrap();
    assert_eq!(first.request_count, 1);
    assert_eq!(first.input_tokens, 100);
    assert_eq!(first.cached_tokens, 40);
    assert_eq!(first.total_tokens, 120);
    sqlx::query("update model_requests set total_tokens=121 where id='req_observe_success'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .dashboard_card_metrics(range)
            .await
            .unwrap()
            .total_tokens,
        121
    );
    sqlx::query("update model_requests set request_kind='account_quality_check' where id='req_observe_success'")
        .execute(&db.pool).await.unwrap();
    // A caller-supplied kind alone must not hide real business traffic.
    assert_eq!(
        store
            .dashboard_card_metrics(range)
            .await
            .unwrap()
            .request_count,
        1
    );
    sqlx::query("update model_requests set protocol='admin_quality_check',client_transport='internal' where id='req_observe_success'")
        .execute(&db.pool).await.unwrap();
    assert_eq!(
        store
            .dashboard_card_metrics(range)
            .await
            .unwrap()
            .request_count,
        0
    );
    sqlx::query("update model_requests set request_kind=null,protocol='openai',client_transport='http_sse',recovered_at=$1,
        recovery_request_id='req_observe_recovery',recovery_attempt_count=1,
        recovery_retry_delay_ms=0,recovery_total_latency_ms=1 where id='req_observe_success'")
        .bind(end).execute(&db.pool).await.unwrap();
    assert_eq!(
        store
            .dashboard_card_metrics(range)
            .await
            .unwrap()
            .request_count,
        0
    );
    sqlx::query(
        "update model_requests set recovered_at=null,recovery_request_id=null,
         recovery_attempt_count=0,recovery_retry_delay_ms=null,recovery_total_latency_ms=null,
         started_at=$1 where id='req_observe_success'",
    )
    .bind(start - TimeDelta::seconds(1))
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        store
            .dashboard_card_metrics(range)
            .await
            .unwrap()
            .request_count,
        0
    );
    db.close().await;
}
