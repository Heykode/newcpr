//! 额度刷新与凭证事实相互独立的回归测试。

use super::*;

#[tokio::test]
async fn quota_refresh_updates_access_fact_without_recovering_credential_error() {
    let store = Arc::new(MemoryAccountStore::default());
    let account_id = "acct_independent_quota_and_credential";
    create_account(&store, account_id).await;
    let account = store.account(account_id).expect("created account");
    persist_credential_state(&store, &account, CredentialState::Expired).await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .and(header(
            "authorization",
            format!("Bearer token-{account_id}"),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "rate_limit": {
                "allowed": false,
                "limit_reached": true,
                "primary_window": {"used_percent": 100, "reset_at": 1_900_000_000}
            }
        })))
        .mount(&server)
        .await;
    let service = quota_service_with_base_url(
        &store,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        server.uri(),
    );

    let snapshot = service
        .refresh_account(account.id())
        .await
        .expect("refresh exhausted quota");
    let current = store.account(account_id).expect("updated account");

    assert_eq!(snapshot.quota().access(), QuotaAccessState::Exhausted);
    assert_eq!(current.quota().access(), QuotaAccessState::Exhausted);
    assert_eq!(current.credential_state(), CredentialState::Expired);

    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .and(header(
            "authorization",
            format!("Bearer token-{account_id}"),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {"used_percent": 9, "reset_at": 1_900_003_600}
            }
        })))
        .mount(&server)
        .await;

    let snapshot = service
        .refresh_account(account.id())
        .await
        .expect("refresh allowed quota");
    let current = store.account(account_id).expect("recovered quota account");

    assert_eq!(snapshot.fact().remaining_percent(), Some(91));
    assert_eq!(snapshot.quota().access(), QuotaAccessState::Allowed);
    assert_eq!(current.quota().access(), QuotaAccessState::Allowed);
    assert_eq!(current.credential_state(), CredentialState::Expired);
}

#[tokio::test]
async fn exhausted_quota_worker_updates_usage_without_unlocking_at_high_usage() {
    let store = Arc::new(MemoryAccountStore::default());
    let account_id = "acct_worker_quota_recovery_gate";
    create_account(&store, account_id).await;
    let account = store.account(account_id).expect("created account");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "rate_limit": {
                "allowed": false,
                "limit_reached": true,
                "primary_window": {"used_percent": 100, "reset_at": 1_700_000_000}
            }
        })))
        .mount(&server)
        .await;
    let service = quota_service_with_base_url(
        &store,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        server.uri(),
    );

    let old_snapshot = service
        .refresh_account(account.id())
        .await
        .expect("seed exhausted quota");
    let old_quota = store.quota_json(account_id).expect("old quota document");

    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {"used_percent": 98, "reset_at": 1_900_000_000}
            }
        })))
        .mount(&server)
        .await;

    let summary = service.synchronize().await.expect("worker quota refresh");

    assert_eq!(summary.exhausted, 1);
    assert_eq!(summary.updated, 0);
    assert_ne!(store.quota_json(account_id), Some(old_quota));
    let refreshed = service
        .read_account(account.id())
        .await
        .expect("read refreshed quota")
        .expect("refreshed quota snapshot");
    assert!(refreshed.observed_at() > old_snapshot.observed_at());
    assert_eq!(refreshed.fact().remaining_percent(), Some(2));
    assert_eq!(refreshed.quota().access(), QuotaAccessState::Exhausted);
    assert_eq!(
        store
            .account(account_id)
            .expect("preserved account")
            .quota()
            .access(),
        QuotaAccessState::Exhausted
    );
}

#[tokio::test]
async fn single_manual_quota_refresh_only_recovers_after_reset_advances_below_ten_percent() {
    for (suffix, allowed, limit_reached, reset_at, used_percent, recovered) in [
        ("stale_denial", false, true, 1_900_003_600_i64, 0_u8, true),
        ("same_reset", true, false, 1_900_000_000_i64, 0_u8, false),
        ("exactly_ten", true, false, 1_900_003_600_i64, 10_u8, false),
        ("below_ten", true, false, 1_900_003_600_i64, 9_u8, true),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        let account_id = format!("acct_manual_quota_recovery_{suffix}");
        create_account(&store, &account_id).await;
        let account = store.account(&account_id).expect("created account");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/codex/usage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "rate_limit": {
                    "allowed": false,
                    "limit_reached": true,
                    "primary_window": {"used_percent": 100, "reset_at": 1_900_000_000}
                }
            })))
            .mount(&server)
            .await;
        let service = quota_service_with_base_url(
            &store,
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("client"),
            server.uri(),
        );
        let old_snapshot = service
            .refresh_account(account.id())
            .await
            .expect("seed exhausted quota");
        let old_quota = store.quota_json(&account_id).expect("old quota document");

        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/api/codex/usage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "rate_limit": {
                    "allowed": allowed,
                    "limit_reached": limit_reached,
                    "primary_window": {"used_percent": used_percent, "reset_at": reset_at}
                }
            })))
            .mount(&server)
            .await;

        let snapshot = service
            .refresh_account(account.id())
            .await
            .expect("refresh exhausted quota");
        let current = store.account(&account_id).expect("refreshed account");

        assert_eq!(
            snapshot.fact().remaining_percent(),
            Some(100 - used_percent)
        );
        assert_ne!(store.quota_json(&account_id), Some(old_quota));
        assert_eq!(snapshot.quota().is_exhausted(), !recovered, "{suffix}");
        assert_eq!(current.quota().is_exhausted(), !recovered, "{suffix}");
        assert!(snapshot.observed_at() > old_snapshot.observed_at());
    }
}

#[tokio::test]
async fn deactivated_workspace_quota_response_persists_credential_error_reason() {
    let store = Arc::new(MemoryAccountStore::default());
    let account_id = "acct_deactivated_workspace";
    create_account(&store, account_id).await;
    let account = store.account(account_id).expect("created account");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "detail": {
                "code": "deactivated_workspace",
                "message": "workspace disabled"
            }
        })))
        .mount(&server)
        .await;
    let service = quota_service_with_base_url(
        &store,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        server.uri(),
    );

    assert!(service.refresh_account(account.id()).await.is_err());
    let current = store
        .account(account_id)
        .expect("account after deactivation");
    assert_eq!(current.credential_state(), CredentialState::Banned);
    assert_eq!(
        current.last_error_reason(),
        Some(gateway_core::account::AccountErrorReason::AccountBanned)
    );
}

const SAME_WINDOW_SHORT_RESET: i64 = 1_900_000_000;
const SAME_WINDOW_WEEK_RESET: i64 = 1_900_600_000;

fn same_window_usage(short_used: u8, weekly_used: u8) -> serde_json::Value {
    json!({
        "rate_limit": {
            "allowed": short_used < 100 && weekly_used < 100,
            "primary_window": {
                "used_percent": short_used,
                "reset_at": SAME_WINDOW_SHORT_RESET,
                "limit_window_seconds": 18_000
            },
            "secondary_window": {
                "used_percent": weekly_used,
                "reset_at": SAME_WINDOW_WEEK_RESET,
                "limit_window_seconds": 604_800
            }
        }
    })
}

async fn mount_same_window_usage(server: &MockServer, usage: serde_json::Value) {
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(usage))
        .mount(server)
        .await;
}

async fn seed_same_window_exhaustion(
    short_used: u8,
) -> (
    Arc<MemoryAccountStore>,
    MockServer,
    CodexCredentialQuotaService,
    ProviderAccount,
) {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_same_window").await;
    let account = store.account("acct_same_window").expect("account");
    let server = MockServer::start().await;
    let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());
    mount_same_window_usage(&server, same_window_usage(short_used, 100)).await;
    service
        .refresh_account(account.id())
        .await
        .expect("seed exhausted quota");
    (store, server, service, account)
}

async fn replace_same_window_document(
    store: &MemoryAccountStore,
    account: &ProviderAccount,
    document: serde_json::Value,
    observed_at: SystemTime,
    state: QuotaState,
) {
    let outcome = store
        .compare_and_swap_quota(gateway_core::account::QuotaObservation {
            plan_type: None,
            account_id: account.id().clone(),
            expected_revision: account.revision(),
            quota: gateway_core::account::OpaqueProviderData::new(
                document.as_object().expect("quota document").clone(),
            ),
            observed_at,
            state,
        })
        .await
        .expect("replace synthetic recovery document");
    assert_eq!(outcome, gateway_core::account::QuotaWriteOutcome::Updated);
}

#[tokio::test]
async fn same_window_recovery_survives_restart_and_preserves_credential_and_enabled_state() {
    let (store, server, service, account) = seed_same_window_exhaustion(100).await;
    persist_credential_state(&store, &account, CredentialState::Expired).await;
    store
        .set_enabled(account.id(), false)
        .await
        .expect("disable");
    let mut usage = same_window_usage(1, 18);
    usage["rate_limit"]["primary_window"]["reset_at"] = json!(SAME_WINDOW_SHORT_RESET + 18_000);
    mount_same_window_usage(&server, usage).await;
    let first = service.refresh_account(account.id()).await.expect("first");
    assert!(first.quota().is_exhausted());
    let document = store.quota_json(account.id().as_str()).expect("quota");
    assert_eq!(
        document["_quota_recovery"]["pending"]
            .as_object()
            .expect("pending")
            .len(),
        1
    );
    assert_eq!(
        document["_quota_recovery"]["candidates"]
            .as_object()
            .expect("candidates")
            .len(),
        1
    );
    drop(service);

    let restarted = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());
    let second = restarted
        .refresh_account(account.id())
        .await
        .expect("second");
    assert_eq!(second.quota().access(), QuotaAccessState::Allowed);
    let current = store.account(account.id().as_str()).expect("account");
    assert!(!current.enabled());
    assert_eq!(current.credential_state(), CredentialState::Expired);
    assert!(
        store
            .quota_json(account.id().as_str())
            .expect("quota")
            .get("_quota_recovery")
            .is_none()
    );
}

#[tokio::test]
async fn same_window_recovery_requires_consecutive_known_account_wide_observations() {
    let mut missing_window = same_window_usage(50, 18);
    missing_window["rate_limit"]
        .as_object_mut()
        .unwrap()
        .remove("secondary_window");
    let mut model_only = missing_window.clone();
    model_only["additional_rate_limits"] = json!([{
        "limit_name": "separate_model",
        "rate_limit": {"primary_window": same_window_usage(50, 18)["rate_limit"]["secondary_window"]}
    }]);
    let mut missing_usage = same_window_usage(50, 18);
    missing_usage["rate_limit"]["secondary_window"]
        .as_object_mut()
        .unwrap()
        .remove("used_percent");
    let mut missing_reset = same_window_usage(50, 18);
    missing_reset["rate_limit"]["secondary_window"]
        .as_object_mut()
        .unwrap()
        .remove("reset_at");
    let mut older_reset = same_window_usage(50, 18);
    older_reset["rate_limit"]["secondary_window"]["reset_at"] = json!(SAME_WINDOW_WEEK_RESET - 1);
    let mut advanced_high_usage = same_window_usage(50, 18);
    advanced_high_usage["rate_limit"]["secondary_window"]["reset_at"] =
        json!(SAME_WINDOW_WEEK_RESET + 604_800);
    let mut explicit_limit = same_window_usage(50, 18);
    explicit_limit["rate_limit"]["secondary_window"]["limit_reached"] = json!(true);
    for interruption in [
        same_window_usage(50, 100),
        explicit_limit,
        missing_window,
        model_only,
        missing_usage,
        missing_reset,
        older_reset,
        advanced_high_usage,
    ] {
        let (_, server, service, account) = seed_same_window_exhaustion(50).await;
        for (usage, exhausted) in [
            (same_window_usage(50, 18), true),
            (interruption, true),
            (same_window_usage(50, 18), true),
            (same_window_usage(50, 18), false),
        ] {
            mount_same_window_usage(&server, usage.clone()).await;
            let snapshot = service
                .refresh_account(account.id())
                .await
                .expect("refresh");
            assert_eq!(snapshot.quota().is_exhausted(), exhausted, "{usage}");
        }
    }
}

#[tokio::test]
async fn same_window_recovery_new_exhaustion_discards_previous_candidates() {
    let (store, server, service, account) = seed_same_window_exhaustion(50).await;
    mount_same_window_usage(&server, same_window_usage(50, 18)).await;
    assert!(
        service
            .refresh_account(account.id())
            .await
            .expect("first")
            .quota()
            .is_exhausted()
    );
    persist_quota_state(&store, &account, QuotaState::allowed(SystemTime::now())).await;
    persist_quota_state(
        &store,
        &account,
        exhausted_quota(Some(
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(SAME_WINDOW_WEEK_RESET as u64),
        )),
    )
    .await;
    for exhausted in [true, false] {
        let snapshot = service
            .refresh_account(account.id())
            .await
            .expect("new exhaustion");
        assert_eq!(snapshot.quota().is_exhausted(), exhausted);
    }
}

#[tokio::test]
async fn same_window_recovery_legacy_documents_need_two_fresh_observations() {
    for retain_unstamped_candidate in [false, true] {
        let (store, server, service, account) = seed_same_window_exhaustion(50).await;
        mount_same_window_usage(&server, same_window_usage(50, 18)).await;
        let first = service.refresh_account(account.id()).await.expect("first");
        let mut document = store.quota_json(account.id().as_str()).expect("quota");
        let recovery = document["_quota_recovery"]
            .as_object_mut()
            .expect("recovery");
        recovery.remove("last_observed_at_micros");
        if !retain_unstamped_candidate {
            recovery.remove("candidates");
        }
        replace_same_window_document(
            &store,
            &account,
            document,
            first.observed_at(),
            first.quota(),
        )
        .await;
        for exhausted in [true, false] {
            let snapshot = service
                .refresh_account(account.id())
                .await
                .expect("legacy refresh");
            assert_eq!(snapshot.quota().is_exhausted(), exhausted);
        }
    }
}

#[tokio::test]
async fn same_window_recovery_repeated_observations_before_watermark_do_not_advance() {
    let (store, server, service, account) = seed_same_window_exhaustion(50).await;
    mount_same_window_usage(&server, same_window_usage(50, 18)).await;
    let first = service.refresh_account(account.id()).await.expect("first");
    let mut document = store.quota_json(account.id().as_str()).expect("quota");
    let future = SystemTime::now() + std::time::Duration::from_secs(86_400);
    document["_quota_recovery"]["last_observed_at_micros"] =
        json!(chrono::DateTime::<Utc>::from(future).timestamp_micros());
    let recovery = document["_quota_recovery"].clone();
    replace_same_window_document(
        &store,
        &account,
        document,
        first.observed_at(),
        first.quota(),
    )
    .await;
    for _ in 0..2 {
        let snapshot = service
            .refresh_account(account.id())
            .await
            .expect("old observation");
        assert!(snapshot.quota().is_exhausted());
        assert_eq!(
            store.quota_json(account.id().as_str()).expect("quota")["_quota_recovery"],
            recovery
        );
    }
}

#[tokio::test]
async fn same_window_recovery_observations_before_exhaustion_do_not_count() {
    let (store, server, service, account) = seed_same_window_exhaustion(50).await;
    let future = SystemTime::now() + std::time::Duration::from_secs(86_400);
    persist_quota_state(
        &store,
        &account,
        QuotaState::exhausted(QuotaEvidence::UsageLimitReached, future, None),
    )
    .await;
    mount_same_window_usage(&server, same_window_usage(50, 18)).await;
    for _ in 0..2 {
        let snapshot = service
            .refresh_account(account.id())
            .await
            .expect("pre-exhaustion");
        assert!(snapshot.quota().is_exhausted());
        let document = store.quota_json(account.id().as_str()).expect("quota");
        assert!(document["_quota_recovery"].get("candidates").is_none());
        assert!(
            document["_quota_recovery"]
                .get("last_observed_at_micros")
                .is_none()
        );
    }
}

#[tokio::test]
async fn same_window_recovery_unknown_reset_needs_a_baseline_then_two_observations() {
    let (store, server, service, account) = seed_same_window_exhaustion(50).await;
    let previous = service
        .read_account(account.id())
        .await
        .expect("read quota")
        .expect("quota snapshot");
    let mut document = same_window_usage(50, 100);
    document["rate_limit"]["secondary_window"]
        .as_object_mut()
        .expect("weekly window")
        .remove("reset_at");
    replace_same_window_document(
        &store,
        &account,
        document,
        previous.observed_at(),
        previous.quota(),
    )
    .await;
    mount_same_window_usage(&server, same_window_usage(50, 18)).await;
    for exhausted in [true, true, false] {
        let snapshot = service
            .refresh_account(account.id())
            .await
            .expect("establish baseline or observe recovery");
        assert_eq!(snapshot.quota().is_exhausted(), exhausted);
    }
}

#[tokio::test]
async fn same_window_recovery_worker_uses_the_same_two_observation_rule() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_same_window_worker").await;
    let account = store.account("acct_same_window_worker").expect("account");
    let server = MockServer::start().await;
    let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());
    let mut usage = same_window_usage(50, 100);
    usage["rate_limit"]["secondary_window"]["reset_at"] = json!(1_700_000_000);
    mount_same_window_usage(&server, usage.clone()).await;
    service
        .refresh_account(account.id())
        .await
        .expect("seed overdue exhaustion");
    usage["rate_limit"]["secondary_window"]["used_percent"] = json!(18);
    usage["rate_limit"]["allowed"] = json!(true);
    mount_same_window_usage(&server, usage).await;
    drop(service);
    for exhausted in [true, false] {
        // Each worker instance starts without the existing 30-minute polling cooldown.
        let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());
        let summary = service.synchronize().await.expect("worker refresh");
        assert_eq!(summary.exhausted, u64::from(exhausted));
        assert_eq!(summary.updated, u64::from(!exhausted));
        assert_eq!(
            store
                .account(account.id().as_str())
                .expect("account")
                .quota()
                .is_exhausted(),
            exhausted
        );
    }
}
