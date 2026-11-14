use super::*;
use gateway_core::account::ResponsesUpstream;

#[tokio::test]
async fn inactive_credential_rotation_does_not_invalidate_the_selected_snapshot() {
    for active_route in [ResponsesUpstream::Codex, ResponsesUpstream::Excel] {
        let store = Arc::new(MemoryAccountStore::default());
        create_excel_account(&store, "acct_primary", "codex-fixture");
        store.set_responses_upstream("acct_primary", active_route);
        let selected = store
            .account("acct_primary")
            .unwrap()
            .for_credential_route(active_route)
            .unwrap();
        let repository = store.repository();
        match active_route {
            ResponsesUpstream::Codex => {
                let rotated = store.seed_excel_credential("acct_primary", true);
                persist_credential_state(&store, &rotated, CredentialState::Expired);
                persist_excel_quota_exhaustion(&store, &rotated, None);
            }
            ResponsesUpstream::Excel => {
                let codex = store
                    .account("acct_primary")
                    .unwrap()
                    .for_credential_route(ResponsesUpstream::Codex)
                    .unwrap();
                repository
                    .rotate_refreshed_oauth_secret(
                        &codex,
                        secret("new-codex-fixture"),
                        Some(SystemTime::now() + Duration::from_secs(7200)),
                        None,
                    )
                    .await
                    .unwrap();
                let rotated = store.account("acct_primary").unwrap();
                let rotated = rotated
                    .for_credential_route(ResponsesUpstream::Codex)
                    .unwrap();
                persist_credential_state(&store, &rotated, CredentialState::Expired);
                persist_quota_exhaustion(&store, &rotated, None);
                store.set_quota_paused("acct_primary", true);
            }
        }
        repository
            .reload_selected_credential(&selected)
            .await
            .expect("inactive credential changes must not invalidate this snapshot");
        store.set_scheduling("acct_primary", None, AccountWeight::new(2).unwrap());
        assert!(
            repository
                .reload_selected_credential(&selected)
                .await
                .is_err(),
            "shared scheduling changes must still invalidate the snapshot"
        );
        store.set_scheduling("acct_primary", None, AccountWeight::DEFAULT);
        store.set_workspace("acct_primary", "changed-shared-workspace");
        assert!(
            repository
                .reload_selected_credential(&selected)
                .await
                .is_err(),
            "shared identity changes must still invalidate the selected credential"
        );
    }
}

#[tokio::test]
async fn connection_test_keeps_the_route_selected_before_an_account_toggle() {
    for frozen_route in [ResponsesUpstream::Codex, ResponsesUpstream::Excel] {
        let store = Arc::new(MemoryAccountStore::default());
        create_excel_account(&store, "acct_primary", "codex-fixture");
        let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
        let context = AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_connection_route").unwrap(),
                ClientApiKeyId::new("key_codex_contract").unwrap(),
            ),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(30),
            account_policy(),
            AccountAttemptContext::diagnostic(
                BTreeSet::new(),
                gateway_core::engine::probe::AccountProbeTarget::ConnectionTestRoute(
                    ProviderAccountId::new("acct_primary").unwrap(),
                    frozen_route,
                ),
                None,
            ),
            None,
            CancellationToken::new(),
        );
        let later_route = match frozen_route {
            ResponsesUpstream::Codex => ResponsesUpstream::Excel,
            ResponsesUpstream::Excel => ResponsesUpstream::Codex,
        };
        store.set_responses_upstream("acct_primary", later_route);
        let lease = capacity_select(&selector, &context, None).await.unwrap();
        assert_eq!(lease.selected_route(), frozen_route);
        assert_eq!(lease.account().credential_route(), frozen_route);
    }
}

#[tokio::test]
async fn selected_route_uses_its_own_state_and_never_falls_back() {
    for waiting in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_excel_account(&store, "acct_primary", "codex-fixture");
        let codex = store.account("acct_primary").unwrap();
        persist_credential_state(&store, &codex, CredentialState::Expired);
        store.set_quota_paused("acct_primary", true);
        let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()))
            .with_account_concurrency(capacity_handle(&["acct_primary"], 1));
        let mut context = attempt(BTreeSet::new());
        if waiting {
            context = context.with_request_tuning(capacity_tuning());
        }
        let lease = capacity_select(&selector, &context, None).await.unwrap();
        assert_eq!(lease.selected_route(), ResponsesUpstream::Excel);
        assert_eq!(lease.account().credential_state(), CredentialState::Ready);
        assert_eq!(
            lease.account().revision(),
            lease.account().credential_revision()
        );
        assert!(lease.cookies().is_empty());
        assert!(lease.installation_id().is_empty());
        store.set_responses_upstream("acct_primary", ResponsesUpstream::Codex);
        assert_eq!(lease.selected_route(), ResponsesUpstream::Excel);
        drop(lease);
        tokio::time::sleep(Duration::from_millis(15)).await;
        assert!(
            capacity_select(&selector, &attempt(BTreeSet::new()), None)
                .await
                .is_err()
        );
        persist_credential_state(&store, &codex, CredentialState::Ready);
        store.set_quota_paused("acct_primary", false);
        persist_credential_state(
            &store,
            &store.excel_account("acct_primary"),
            CredentialState::Expired,
        );
        assert_eq!(
            store
                .account("acct_primary")
                .unwrap()
                .status_projection(SystemTime::now(), None)
                .status,
            gateway_core::account::AccountStatus::Normal
        );
        store
            .repository()
            .reload_selected_credential(&store.account("acct_primary").unwrap())
            .await
            .expect("native runtime remains usable");
        let lease = capacity_select(&selector, &attempt(BTreeSet::new()), None)
            .await
            .unwrap();
        assert_eq!(lease.selected_route(), ResponsesUpstream::Codex);
        drop(lease);
        tokio::time::sleep(Duration::from_millis(15)).await;
        store.set_responses_upstream("acct_primary", ResponsesUpstream::Excel);
        assert!(
            capacity_select(&selector, &attempt(BTreeSet::new()), None)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn legacy_excel_account_without_independent_material_fails_closed() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "codex-fixture");
    store.set_responses_upstream("acct_primary", ResponsesUpstream::Excel);
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    assert!(
        capacity_select(&selector, &attempt(BTreeSet::new()), None)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .credential_loads
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn excel_success_failure_and_quota_leave_codex_facts_unchanged() {
    let store = Arc::new(MemoryAccountStore::default());
    create_excel_account(&store, "acct_primary", "codex-fixture");
    let codex = store.account("acct_primary").unwrap();
    persist_credential_state(&store, &codex, CredentialState::Expired);
    persist_quota_exhaustion(
        &store,
        &codex,
        Some(SystemTime::now() + Duration::from_secs(3600)),
    );
    let before = store
        .account("acct_primary")
        .unwrap()
        .with_excel_credential(None);
    let excel = store.excel_account("acct_primary");
    let selector = Arc::new(selector(&store, Arc::new(TestLeaseCoordinator::default())));
    selector
        .record_excel_authentication_failure(&excel, false, false)
        .await;
    selector
        .record_diagnostic_success(&excel, SystemTime::now())
        .await;
    selector
        .record_failure(&excel, CodexAccountFailure::QuotaExhausted, None)
        .await
        .unwrap();
    assert!(store.excel_account("acct_primary").quota().is_exhausted());
    assert_eq!(
        store
            .account("acct_primary")
            .unwrap()
            .with_excel_credential(None),
        before
    );
}

#[tokio::test]
async fn capacity_wait_reloads_the_excel_revision_after_refresh() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_excel_account(&store, "acct_primary", "codex-fixture");
    let old = store.excel_account("acct_primary");
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    leases.capacity.set_load("acct_primary", 1);
    let selector = selector(&store, leases.clone())
        .with_account_concurrency(capacity_handle(&["acct_primary"], 1));
    let context = attempt(BTreeSet::new()).with_request_tuning(capacity_tuning());
    let selected = capacity_select(&selector, &context, None);
    tokio::pin!(selected);
    tokio::select! {
        result = &mut selected => panic!("must wait: {result:?}"),
        () = wait_until_queued(&leases) => {}
    }
    let refreshed = store.seed_excel_credential("acct_primary", true);
    leases.capacity.set_load("acct_primary", 0);
    let lease = selected.await.unwrap();
    assert_eq!(lease.selected_route(), ResponsesUpstream::Excel);
    assert_eq!(
        lease.account().credential_revision(),
        refreshed.credential_revision()
    );
    assert!(lease.account().credential_revision() > old.credential_revision());
    assert_eq!(lease.account().revision(), old.revision());
}
