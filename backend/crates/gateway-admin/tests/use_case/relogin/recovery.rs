use super::*;

async fn state(h: &Harness, id: &str) -> String {
    h.services
        .relogin()
        .list()
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap()
        .recovery
        .state
        .to_owned()
}

#[tokio::test]
async fn relogin_success_clears_retry_delay_but_preserves_rolling_start_budget() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    for generation in 1..=3 {
        let mut expired = account(true);
        expired.credential_revision = revision(generation);
        expired.turn_state_binding_revision = revision(generation);
        h.accounts.set_accounts(vec![expired]);
        assert_eq!(state(&h, &id).await, "waiting");
        h.cycle().await;
        let row = h.row(&id).await;
        assert!(row.synced_at.is_some());
        assert!(row.next_attempt_at.is_none());
        assert!(row.attempted_target.is_none());
        assert_eq!(row.automatic_attempts, 0);
        assert_eq!(row.automatic_started_at.len(), generation as usize);
    }
    let mut expired = account(true);
    expired.credential_revision = revision(4);
    expired.turn_state_binding_revision = revision(4);
    h.accounts.set_accounts(vec![expired]);
    assert_eq!(state(&h, &id).await, "loop_guard");
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 3);
    // A restart/JSONB roundtrip must not reset the protection.
    let row: ReloginEntry =
        serde_json::from_value(serde_json::to_value(h.row(&id).await).unwrap()).unwrap();
    assert_eq!(row.automatic_started_at.len(), 3);
    let mut row = row;
    row.automatic_started_at = vec![Utc::now() - Duration::minutes(16); 3];
    h.store.rows.lock().unwrap().insert(id.clone(), row);
    assert_eq!(state(&h, &id).await, "waiting");
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 4);
    assert_eq!(h.row(&id).await.automatic_started_at.len(), 1);
}

#[tokio::test]
async fn relogin_new_binding_ignores_old_failure_delay_but_cookie_writes_do_not() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    *h.provider.relogin_result.lock().unwrap() = None;
    h.cycle().await;
    assert_eq!(state(&h, &id).await, "cooldown");
    assert!(h.row(&id).await.next_attempt_at.unwrap() > Utc::now());
    let mut expired = account(true);
    expired.credential_revision = revision(2);
    h.accounts.set_accounts(vec![expired.clone()]);
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 1);
    expired.turn_state_binding_revision = revision(2);
    h.accounts.set_accounts(vec![expired]);
    assert_eq!(state(&h, &id).await, "waiting");
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 2);
    assert_eq!(h.row(&id).await.automatic_attempts, 1);
}

#[tokio::test]
async fn relogin_legacy_success_does_not_inherit_cooldown_or_require_new_json_fields() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    let mut legacy = serde_json::to_value(h.row(&id).await).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("automatic_started_at");
    let mut row: ReloginEntry = serde_json::from_value(legacy).unwrap();
    assert!(row.automatic_started_at.is_empty());
    row.status = ReloginStatus::Ready;
    row.synced_at = Some(Utc::now());
    row.automatic_attempts = 3;
    row.next_attempt_at = Some(Utc::now() + Duration::minutes(15));
    row.attempted_target = Some(ReloginTarget::from_account(&account(true)).unwrap());
    h.store.rows.lock().unwrap().insert(id.clone(), row);
    assert_eq!(state(&h, &id).await, "waiting");
    h.cycle().await;
    assert!(h.row(&id).await.synced_at.is_some());
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn relogin_cached_manual_success_waits_for_confirmation_without_repeating_login() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    assert!(h.row(&id).await.next_attempt_at.is_none());
    assert_eq!(state(&h, &id).await, "awaiting_push");
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 1);
    assert!(h.accounts.audit_requests().is_empty());
}

#[tokio::test]
async fn relogin_definite_push_failure_retains_delay_and_unknown_push_never_replays() {
    for ambiguous in [false, true] {
        let h = Harness::new(vec![account(true)]).await;
        let id = h.import("test@example.invalid").await;
        if ambiguous {
            h.accounts.fail_next_commit();
        } else {
            h.provider.fail_next(ProviderAdminErrorKind::Invalid);
        }
        h.cycle().await;
        let row = h.row(&id).await;
        assert!(row.synced_at.is_none());
        assert!(row.next_attempt_at.unwrap() > Utc::now());
        assert_eq!(
            state(&h, &id).await,
            if ambiguous { "uncertain" } else { "cooldown" }
        );
        h.cycle().await;
        assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn relogin_diagnostics_explain_safe_skip_reasons_without_secrets() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    assert_eq!(state(&h, &id).await, "waiting");
    h.services
        .relogin()
        .automatic(std::slice::from_ref(&id), false)
        .await
        .unwrap();
    assert_eq!(state(&h, &id).await, "disabled");
    h.services
        .relogin()
        .automatic(std::slice::from_ref(&id), true)
        .await
        .unwrap();
    h.services
        .relogin()
        .configure(ReloginSettings {
            concurrency: 1,
            paused: true,
        })
        .await
        .unwrap();
    assert_eq!(state(&h, &id).await, "paused");
    h.services
        .relogin()
        .configure(ReloginSettings::default())
        .await
        .unwrap();
    let mut disabled = account(true);
    disabled.enabled = false;
    h.accounts.set_accounts(vec![disabled]);
    assert_eq!(state(&h, &id).await, "account_disabled");
    let mut second = account(true);
    second.id = "acct_other".into();
    second.upstream_account_id = Some("other-workspace".into());
    h.accounts.set_accounts(vec![account(true), second]);
    assert_eq!(state(&h, &id).await, "workspace_required");
    h.cycle().await;
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
    h.accounts.set_accounts(vec![account(false)]);
    assert_eq!(state(&h, &id).await, "idle");
    let json = serde_json::to_string(&h.services.relogin().list().await.unwrap()).unwrap();
    for secret in [
        "test-only-password",
        "JBSWY",
        "automaticStartedAt",
        "mfaSecret",
    ] {
        assert!(!json.contains(secret));
    }
}

async fn wait_requests(h: &Harness, count: usize) {
    for _ in 0..1000 {
        if h.provider.relogin_requests.lock().unwrap().len() >= count {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("login did not start");
}

#[tokio::test(start_paused = true)]
async fn relogin_scans_and_refills_while_a_slow_login_is_running() {
    let h = Harness::new(vec![]).await;
    h.services
        .relogin()
        .configure(ReloginSettings {
            concurrency: 2,
            paused: false,
        })
        .await
        .unwrap();
    let slow = h.import("slow@example.invalid").await;
    h.services
        .relogin()
        .queue(std::slice::from_ref(&slow))
        .await
        .unwrap();
    *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_secs(60);
    let task = h.task.clone();
    let running = tokio::spawn(async move { task.run_cycle(cycle_context()).await });
    wait_requests(&h, 1).await;
    *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_secs(1);
    let fast = h.import("fast@example.invalid").await;
    h.services
        .relogin()
        .queue(std::slice::from_ref(&fast))
        .await
        .unwrap();
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    wait_requests(&h, 2).await;
    assert_eq!(h.row(&slow).await.status, ReloginStatus::Running);
    let next = h.import("next@example.invalid").await;
    h.services
        .relogin()
        .queue(std::slice::from_ref(&next))
        .await
        .unwrap();
    assert_eq!(state(&h, &next).await, "waiting");
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    wait_requests(&h, 3).await;
    assert_eq!(h.row(&slow).await.status, ReloginStatus::Running);
    assert_eq!(h.row(&fast).await.status, ReloginStatus::Ready);
    assert_eq!(h.provider.relogin_peak.load(Ordering::SeqCst), 2);
    h.services
        .relogin()
        .configure(ReloginSettings {
            concurrency: 2,
            paused: true,
        })
        .await
        .unwrap();
    running.await.unwrap().unwrap();
    assert_eq!(h.provider.relogin_active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn relogin_dropped_cycle_releases_slots_and_marks_orphan_without_replay() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    h.services
        .relogin()
        .queue(std::slice::from_ref(&id))
        .await
        .unwrap();
    *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_secs(60);
    let task = h.task.clone();
    let running = tokio::spawn(async move { task.run_cycle(cycle_context()).await });
    h.wait_running().await;
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    h.cycle().await;
    assert_eq!(h.row(&id).await.status, ReloginStatus::Failed);
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 1);
    *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::ZERO;
    h.ready(&id).await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn relogin_scan_polls_settlement_while_waiting_for_its_gate() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    *h.store.ready_save_delay.lock().unwrap() = std::time::Duration::from_secs(5);
    tokio::time::timeout(std::time::Duration::from_secs(20), h.cycle())
        .await
        .expect("scan must not deadlock the settlement holding gate");
    assert!(h.row(&id).await.synced_at.is_some());
    assert_eq!(h.accounts.audit_requests().len(), 1);
}
