use super::*;
use std::time::Duration;

fn sample(used: &str, at: SystemTime) -> CodexRateLimitObservation {
    CodexRateLimitObservation::from_headers(
        &[("x-codex-primary-used-percent".to_owned(), used.to_owned())],
        at,
    )
    .unwrap()
}

#[tokio::test]
async fn passive_quota_retains_capture_time_and_rejects_stale_buffered_samples() {
    let store = Arc::new(MemoryAccountStore::default());
    let id = "acct_quota_clock";
    create_account(&store, id).await;
    let account = store.account(id).unwrap();
    let service = quota_service(&store);
    let now = SystemTime::now();
    let old = sample("8", now - Duration::from_secs(20));
    let fresh = sample("60", now - Duration::from_secs(10));
    service
        .synchronize_passive_rate_limits(
            &account,
            std::slice::from_ref(&fresh),
            QuotaRefreshAuthority::PreserveAccess,
        )
        .await
        .unwrap();
    let before = store.quota_json(id).unwrap();
    assert!(
        !service
            .synchronize_passive_rate_limits(
                &account,
                &[old],
                QuotaRefreshAuthority::PreserveAccess,
            )
            .await
            .unwrap()
    );
    assert_eq!(store.quota_json(id).unwrap(), before);
    let persisted = store
        .get_quotas(std::slice::from_ref(account.id()))
        .await
        .unwrap();
    assert_eq!(persisted[0].observed_at, fresh.observed_at);
}

#[tokio::test]
async fn passive_quota_orders_buffered_samples_by_capture_time() {
    let store = Arc::new(MemoryAccountStore::default());
    let id = "acct_quota_sample_order";
    create_account(&store, id).await;
    let account = store.account(id).unwrap();
    let service = quota_service(&store);
    let now = SystemTime::now();
    service
        .synchronize_passive_rate_limits(
            &account,
            &[
                sample("60", now),
                sample("8", now - Duration::from_secs(10)),
            ],
            QuotaRefreshAuthority::PreserveAccess,
        )
        .await
        .unwrap();
    let snapshot = service.read_account(account.id()).await.unwrap().unwrap();
    assert_eq!(snapshot.fact().remaining_percent(), Some(40));
    assert_eq!(snapshot.observed_at(), now);
}

#[tokio::test]
async fn passive_failure_preserves_plan_access_and_identity_revisions() {
    let store = Arc::new(MemoryAccountStore::default());
    let id = "acct_quota_failure_plan";
    create_account(&store, id).await;
    let account = store.account(id).unwrap();
    persist_quota_state(&store, &account, exhausted_quota(None)).await;
    let before = store.account(id).unwrap();
    let service = quota_service(&store);
    service
        .synchronize_passive_headers(
            &account,
            &[
                ("x-codex-primary-used-percent".to_owned(), "8".to_owned()),
                ("x-codex-plan-type".to_owned(), "team".to_owned()),
            ],
            SystemTime::now(),
            QuotaRefreshAuthority::PreserveAccess,
        )
        .await
        .unwrap();
    let after = store.account(id).unwrap();
    assert_eq!(after.quota().access(), QuotaAccessState::Exhausted);
    assert_eq!(after.plan_type(), before.plan_type());
    assert_eq!(after.revision(), before.revision());
    assert_eq!(
        after.turn_state_binding_revision(),
        before.turn_state_binding_revision()
    );
}

#[tokio::test]
async fn successful_inference_recovers_access_without_refreshing_stale_quota() {
    let store = Arc::new(MemoryAccountStore::default());
    let id = "acct_quota_stale_success";
    create_account(&store, id).await;
    let account = store.account(id).unwrap();
    persist_quota_state(&store, &account, exhausted_quota(None)).await;
    let account = store.account(id).unwrap();
    let service = quota_service(&store);
    let now = SystemTime::now();
    let fresh = sample("60", now);
    service
        .synchronize_passive_rate_limits(&account, &[fresh], QuotaRefreshAuthority::PreserveAccess)
        .await
        .unwrap();
    let before = store.quota_json(id).unwrap();
    service
        .synchronize_passive_rate_limits(
            &account,
            &[sample("8", now - Duration::from_secs(10))],
            QuotaRefreshAuthority::ObserveAccess,
        )
        .await
        .unwrap();
    assert_eq!(
        store.account(id).unwrap().quota().access(),
        QuotaAccessState::Allowed
    );
    assert_eq!(store.quota_json(id).unwrap(), before);
    assert_eq!(
        service
            .read_account(account.id())
            .await
            .unwrap()
            .unwrap()
            .observed_at(),
        now
    );
}

#[tokio::test]
async fn metadata_only_observation_does_not_refresh_quota_window_age() {
    let store = Arc::new(MemoryAccountStore::default());
    let id = "acct_quota_metadata_clock";
    create_account(&store, id).await;
    let account = store.account(id).unwrap();
    let service = quota_service(&store);
    let at = SystemTime::now() - Duration::from_secs(20);
    service
        .synchronize_passive_rate_limits(
            &account,
            &[sample("60", at)],
            QuotaRefreshAuthority::ObserveAccess,
        )
        .await
        .unwrap();
    service
        .synchronize_passive_headers(
            &account,
            &[("x-codex-credits-has-credits".to_owned(), "false".to_owned())],
            SystemTime::now(),
            QuotaRefreshAuthority::PreserveAccess,
        )
        .await
        .unwrap();
    let snapshot = service.read_account(account.id()).await.unwrap().unwrap();
    assert_eq!(snapshot.observed_at(), at);
    assert_eq!(snapshot.fact().remaining_percent(), Some(40));
}
