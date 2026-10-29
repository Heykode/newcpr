use super::*;
use gateway_core::account::{
    AccountAffinity, AccountSelectionPolicy, AccountStateChange, RotationStrategy,
};
use gateway_core::error::ProviderError;
use gateway_core::provider_ports::{ProviderSessionAffinityKey, ProviderSessionAffinityPort};
use gateway_core::routing::RequestTuning;
use std::sync::atomic::Ordering;

struct Fixture {
    store: Arc<MemoryAccountStore>,
    affinity: Arc<MemorySessionAffinity>,
    leases: Arc<TestLeaseCoordinator>,
    upstream: MockServer,
    provider: CodexProvider,
    key: ProviderSessionAffinityKey,
}

fn request(child: bool) -> ProviderRequest {
    let generation = generate_with_session_context("root", child.then_some("child"), None);
    planned_request(
        "openai",
        Operation::Generate(GenerateRequest::from_protocol_payload(
            generation
                .protocol_payload()
                .clone()
                .with_context(Map::from_iter([("use_websocket".to_owned(), json!(false))])),
        )),
    )
}

fn attempt(wait: bool, excluded: bool, cancellation: CancellationToken) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_follow_recovery").unwrap(),
            ClientApiKeyId::new("key_openai_contract").unwrap(),
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(5),
        AccountSelectionPolicy::new(RotationStrategy::Sticky, NonZeroU32::MIN, Duration::ZERO)
            .with_openai_account_affinity(AccountAffinity::Strict),
        AccountAttemptContext::new(
            if excluded {
                [ProviderAccountId::new("acct_scope_old").unwrap()].into()
            } else {
                BTreeSet::new()
            },
            None,
            None,
        )
        .with_account_scope(contract_account_scope()),
        None,
        cancellation,
    )
    .with_request_tuning(RequestTuning {
        account_busy_wait_enabled: wait,
        account_busy_wait_sticky_timeout_seconds: 2,
        account_busy_wait_fallback_timeout_seconds: 1,
        ..Default::default()
    })
}

async fn fixture(spare: bool) -> Fixture {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    let upstream = MockServer::start().await;
    let provider = super::scheduling::waiting_provider_with_affinity_and_limit(
        &store,
        leases.clone(),
        upstream.uri(),
        Arc::new(MemorySessionExclusions::default()),
        affinity.clone(),
        NonZeroU32::MIN,
    );
    drop(
        provider
            .execute(
                request(false),
                attempt(true, false, CancellationToken::new()),
            )
            .await
            .unwrap(),
    );
    let key = ProviderSessionAffinityKey::try_new(affinity.lookup_keys()[0].clone()).unwrap();
    if spare {
        create_account(&store, "acct_scope_new").await;
    }
    leases.capacity.set_load("acct_scope_old", 0);
    leases.capacity.set_load("acct_scope_new", 0);
    Fixture {
        store,
        affinity,
        leases,
        upstream,
        provider,
        key,
    }
}

async fn queued<T>(
    pending: &mut (impl std::future::Future<Output = Result<T, ProviderError>> + Unpin),
    f: &Fixture,
) {
    tokio::select! {
        result = pending => panic!("strict descendant returned before owner recovery: {:?}", result.err()),
        () = async {
            timeout(Duration::from_secs(1), async {
                while f.leases.capacity.waiting.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
            }).await.expect("descendant enters bounded queue");
        } => {}
    }
}

async fn credential_state(f: &Fixture, state: CredentialState) {
    let id = ProviderAccountId::new("acct_scope_old").unwrap();
    let account = f.store.get_account(&id).await.unwrap().unwrap();
    f.store
        .apply_state_change(AccountStateChange {
            account_id: id,
            expected_revision: account.revision(),
            credential_state: state,
            error_reason: (state == CredentialState::Expired)
                .then_some(AccountErrorReason::CredentialExpired),
            message: None,
            observed_at: SystemTime::now(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn strict_expired_or_disabled_owner_waits_and_recovers_with_ordinary_wait_on_or_off() {
    for wait in [false, true] {
        for expired in [false, true] {
            let f = fixture(true).await;
            let id = ProviderAccountId::new("acct_scope_old").unwrap();
            if expired {
                credential_state(&f, CredentialState::Expired).await;
            } else {
                f.store.set_enabled(&id, false).await.unwrap();
            }
            let mut pending = Box::pin(f.provider.execute(
                request(true),
                attempt(wait, false, CancellationToken::new()),
            ));
            queued(&mut pending, &f).await;
            assert_eq!(
                f.leases.capacity.promotions.load(Ordering::SeqCst),
                0,
                "ineligible credentials must not acquire a slot"
            );
            if expired {
                credential_state(&f, CredentialState::Ready).await;
            } else {
                f.store.set_enabled(&id, true).await.unwrap();
            }
            let stream = timeout(Duration::from_secs(2), pending)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stream.metadata().provider_account_id(), &id);
            drop(stream);
            assert_eq!(f.leases.capacity.waiting.load(Ordering::SeqCst), 0);
            assert!(f.upstream.received_requests().await.unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn strict_retry_exclusion_waits_for_root_migration_and_uses_new_account() {
    for wait in [false, true] {
        let f = fixture(true).await;
        credential_state(&f, CredentialState::Expired).await;
        let mut pending = Box::pin(
            f.provider
                .execute(request(true), attempt(wait, true, CancellationToken::new())),
        );
        queued(&mut pending, &f).await;
        Mock::given(method("POST")).and(path("/codex/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("event: response.created\ndata: {}\n\n{CAPTURE_COMPLETED_SSE}", json!({"type":"response.created","response":{"id":"resp_scope_capture","model":"gpt-5.4","status":"in_progress"}})),
                "text/event-stream",
            )).expect(2).mount(&f.upstream).await;
        let mut root = f
            .provider
            .execute(
                request(false),
                attempt(true, true, CancellationToken::new()),
            )
            .await
            .unwrap();
        assert_eq!(
            root.metadata().provider_account_id().as_str(),
            "acct_scope_new"
        );
        while let Some(event) = root.next().await {
            event.unwrap();
        }
        drop(root);
        let mut child = timeout(Duration::from_secs(2), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            child.metadata().provider_account_id().as_str(),
            "acct_scope_new"
        );
        let mut completed = false;
        while let Some(event) = child.next().await {
            completed |= event
                .unwrap()
                .wire_event()
                .is_some_and(|wire| wire.event_type() == Some("response.completed"));
        }
        assert!(completed);
        drop(child);
        assert_eq!(f.leases.capacity.waiting.load(Ordering::SeqCst), 0);
        assert_eq!(
            f.affinity
                .load(&ProviderKind::new("openai").unwrap(), &f.key)
                .await
                .unwrap()
                .unwrap()
                .as_str(),
            "acct_scope_new"
        );
        f.upstream.verify().await;
    }
}

#[tokio::test]
async fn strict_excluded_owner_is_not_readmitted_just_because_credentials_recover() {
    let f = fixture(true).await;
    let cancel = CancellationToken::new();
    let mut pending = Box::pin(
        f.provider
            .execute(request(true), attempt(false, true, cancel.clone())),
    );
    queued(&mut pending, &f).await;
    credential_state(&f, CredentialState::Ready).await;
    assert!(
        timeout(Duration::from_millis(160), pending.as_mut())
            .await
            .is_err()
    );
    cancel.cancel();
    let error = pending.await.err().unwrap();
    assert_eq!(error.kind(), ProviderErrorKind::Cancelled);
    assert!(error.retry_is_prohibited());
    assert_eq!(f.leases.capacity.waiting.load(Ordering::SeqCst), 0);
    assert_eq!(f.leases.capacity.promotions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn strict_confirmed_quota_exhaustion_waits_with_ordinary_wait_on_or_off() {
    for wait in [false, true] {
        let f = fixture(true).await;
        let account = f.store.account("acct_scope_old").unwrap();
        f.store
            .apply_quota_access(QuotaAccessChange {
                account_id: account.id().clone(),
                expected_revision: account.revision(),
                state: QuotaState::exhausted(
                    QuotaEvidence::UsageLimitReached,
                    SystemTime::now(),
                    None,
                ),
            })
            .await
            .unwrap();
        let cancel = CancellationToken::new();
        let mut pending = Box::pin(
            f.provider
                .execute(request(true), attempt(wait, false, cancel.clone())),
        );
        queued(&mut pending, &f).await;
        assert_eq!(f.leases.capacity.promotions.load(Ordering::SeqCst), 0);
        cancel.cancel();
        let error = pending.await.err().unwrap();
        assert_eq!(error.kind(), ProviderErrorKind::Cancelled);
        assert!(error.retry_is_prohibited());
        assert_eq!(f.leases.capacity.waiting.load(Ordering::SeqCst), 0);
        assert!(f.upstream.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn strict_child_waits_after_real_upstream_authentication_or_quota_rejection() {
    for (status, code, message) in [
        (
            401,
            "token_revoked",
            "The authentication token has been revoked.",
        ),
        (
            429,
            "usage_limit_reached",
            "You have reached your usage limit.",
        ),
        (429, "rate_limit_exceeded", "Too many requests."),
    ] {
        let f = fixture(true).await;
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(ResponseTemplate::new(status)
                .insert_header("retry-after", "30")
                .set_body_json(json!({"error": {"type": if status == 401 {"authentication_error"} else {"rate_limit_error"}, "code": code, "message": message}})))
            .expect(1)
            .mount(&f.upstream).await;
        let mut failed = f
            .provider
            .execute(
                request(false),
                attempt(true, false, CancellationToken::new()),
            )
            .await
            .unwrap();
        let mut observed = false;
        while let Some(event) = failed.next().await {
            if let Err(error) = event {
                assert_eq!(error.upstream_status(), Some(status));
                observed = true;
                break;
            }
        }
        assert!(observed);
        drop(failed);
        f.upstream.verify().await;
        f.upstream.reset().await;
        let mut pending = Box::pin(f.provider.execute(
            request(true),
            attempt(false, false, CancellationToken::new()),
        ));
        queued(&mut pending, &f).await;
        assert!(f.upstream.received_requests().await.unwrap().is_empty());
        f.affinity.seed_binding(
            &ProviderKind::new("openai").unwrap(),
            f.key.expose_to_store(),
            ProviderAccountId::new("acct_scope_new").unwrap(),
        );
        let stream = timeout(Duration::from_secs(2), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_scope_new"
        );
        drop(stream);
        assert_eq!(f.leases.capacity.waiting.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn strict_wait_timeout_and_queue_full_cannot_escape_to_provider_fallback() {
    for full in [false, true] {
        let f = fixture(true).await;
        credential_state(&f, CredentialState::Expired).await;
        f.leases.capacity.full.store(full, Ordering::SeqCst);
        let attempt = attempt(false, false, CancellationToken::new());
        let budget = attempt.account_wait_budget().unwrap().clone();
        let error = timeout(
            Duration::from_secs(3),
            f.provider.execute(request(true), attempt),
        )
        .await
        .unwrap()
        .err()
        .unwrap();
        assert_eq!(error.kind(), ProviderErrorKind::AccountCapacityUnavailable);
        assert!(error.retry_is_prohibited());
        assert_eq!(budget.is_exhausted(), !full);
        assert_eq!(f.leases.capacity.waiting.load(Ordering::SeqCst), 0);
        assert!(f.upstream.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn strict_root_migrations_keep_the_original_deadline_and_reload_the_frozen_pool() {
    let f = fixture(true).await;
    f.leases.capacity.set_load("acct_scope_old", 1);
    f.leases.capacity.set_load("acct_scope_new", 1);
    let mut pending = Box::pin(f.provider.execute(
        request(true),
        attempt(true, false, CancellationToken::new()),
    ));
    queued(&mut pending, &f).await;
    let original = f.leases.capacity.waits.lock().unwrap()[0].deadline();
    for (index, id) in [
        "acct_scope_new",
        "acct_scope_old",
        "acct_scope_new",
        "acct_scope_old",
    ]
    .into_iter()
    .enumerate()
    {
        f.affinity.seed_binding(
            &ProviderKind::new("openai").unwrap(),
            f.key.expose_to_store(),
            ProviderAccountId::new(id).unwrap(),
        );
        tokio::select! {
            _ = &mut pending => panic!("root changes must not exhaust snapshot retries"),
            () = async {
                timeout(Duration::from_secs(1), async {
                    while f.leases.capacity.waits.lock().unwrap().len() < index + 2 { tokio::task::yield_now().await; }
                }).await.unwrap();
            } => {}
        }
        assert_eq!(
            f.leases
                .capacity
                .waits
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .deadline(),
            original
        );
    }
    f.leases.capacity.set_load("acct_scope_old", 0);
    let stream = timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_scope_old"
    );
    drop(stream);
    assert_eq!(f.leases.capacity.waiting.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn strict_migration_to_an_account_outside_the_frozen_universe_cannot_acquire_it() {
    let f = fixture(false).await;
    credential_state(&f, CredentialState::Expired).await;
    let cancel = CancellationToken::new();
    let mut pending = Box::pin(
        f.provider
            .execute(request(true), attempt(true, false, cancel.clone())),
    );
    queued(&mut pending, &f).await;
    create_account(&f.store, "acct_scope_new").await;
    f.affinity.seed_binding(
        &ProviderKind::new("openai").unwrap(),
        f.key.expose_to_store(),
        ProviderAccountId::new("acct_scope_new").unwrap(),
    );
    assert!(
        timeout(Duration::from_millis(220), pending.as_mut())
            .await
            .is_err()
    );
    assert!(
        !f.leases
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.account_id().as_str() == "acct_scope_new")
    );
    cancel.cancel();
    assert_eq!(
        pending.await.err().unwrap().kind(),
        ProviderErrorKind::Cancelled
    );
    assert_eq!(f.leases.capacity.waiting.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn strict_root_migration_during_acquisition_releases_old_slot_and_reselects() {
    let f = fixture(true).await;
    let affinity = f.affinity.clone();
    let key = f.key.clone();
    *f.leases.capacity.after_acquire.lock().unwrap() = Some(Box::new(move || {
        affinity.seed_binding(
            &ProviderKind::new("openai").unwrap(),
            key.expose_to_store(),
            ProviderAccountId::new("acct_scope_new").unwrap(),
        );
    }));
    let stream = f
        .provider
        .execute(
            request(true),
            attempt(true, false, CancellationToken::new()),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_scope_new"
    );
    drop(stream);
    assert!(
        f.leases
            .capacity
            .signals
            .lock()
            .unwrap()
            .values()
            .all(|signals| signals.in_flight == 0)
    );
    assert_eq!(f.leases.capacity.waiting.load(Ordering::SeqCst), 0);
    assert!(f.upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn strict_missing_root_binding_can_be_reclaimed_without_sending_to_expired_owner() {
    let f = fixture(true).await;
    credential_state(&f, CredentialState::Expired).await;
    let mut pending = Box::pin(f.provider.execute(
        request(true),
        attempt(true, false, CancellationToken::new()),
    ));
    queued(&mut pending, &f).await;
    f.affinity
        .clear(&ProviderKind::new("openai").unwrap(), &f.key)
        .await
        .unwrap();
    let stream = timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_scope_new"
    );
    drop(stream);
    assert_eq!(f.leases.capacity.waiting.load(Ordering::SeqCst), 0);
}
