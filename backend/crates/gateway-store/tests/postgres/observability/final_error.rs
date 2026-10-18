use super::*;

#[tokio::test]
async fn usage_list_projects_final_error_not_earlier_retry_failure() {
    let Some(database) = TestDatabase::create("usage_final_error").await else {
        return;
    };
    let now = Utc::now();
    seed_observability_facts(&database.pool, now).await.unwrap();
    let store = admin_observability_store(&database.pool);
    let query = || admin_observability::UsageQuery {
        record_status: admin_observability::UsageRecordStatus::All,
        range: admin_observability::TimeRange::new(
            now - TimeDelta::hours(1),
            now + TimeDelta::hours(1),
        )
        .unwrap(),
        filter: admin_observability::UsageFilter::default(),
        current_page: 1,
        page_size: PageSize::new(10).unwrap(),
    };
    let page = store.list_usage_records(query()).await.unwrap();
    let failed = page
        .items
        .iter()
        .find(|item| item.id == "req_observe_failed")
        .unwrap();
    assert_eq!(failed.error_code.as_deref(), Some("rate_limit"));
    assert_eq!(failed.error_message.as_deref(), Some("upstream limited"));
    // The earlier ops event remains, but the request succeeds after recovery.
    sqlx::query(
        "update model_requests set outcome = 'succeeded', client_status_code = 200,
        downstream_committed_at = completed_at, provider_error_code = null, error_message = null,
        error_kind = null where id = 'req_observe_failed'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let page = store.list_usage_records(query()).await.unwrap();
    let recovered = page
        .items
        .iter()
        .find(|item| item.id == "req_observe_failed")
        .unwrap();
    assert_eq!(
        recovered.outcome,
        admin_observability::RequestOutcome::Succeeded
    );
    assert!(recovered.error_code.is_none());
    assert!(recovered.error_message.is_none());
    database.close().await;
}
