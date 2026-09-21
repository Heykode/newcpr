use super::*;
use gateway_admin::model::relogin::{ReloginSettingsUpdate, ReloginStopReason};

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
async fn relogin_configurable_retry_budget_excludes_the_first_attempt() {
    for max_retries in [0, 2, 10] {
        let h = Harness::new(vec![account(true)]).await;
        let id = h.import("test@example.invalid").await;
        h.services
            .relogin()
            .configure(ReloginSettings {
                max_retries,
                retry_interval_minutes: 17,
                ..ReloginSettings::default()
            })
            .await
            .unwrap();
        *h.provider.relogin_result.lock().unwrap() = None;
        for attempt in 1..=max_retries + 1 {
            let mut row = h.row(&id).await;
            row.next_attempt_at = None;
            row.automatic_started_at.clear();
            h.store.rows.lock().unwrap().insert(id.clone(), row);
            assert_eq!(state(&h, &id).await, "waiting");
            let before = Utc::now();
            h.cycle().await;
            let after = Utc::now();
            let row = h.row(&id).await;
            assert_eq!(row.automatic_attempts, attempt);
            let deadline = row.next_attempt_at.unwrap();
            assert!(deadline >= before + Duration::minutes(17));
            assert!(deadline <= after + Duration::minutes(17));
            assert_eq!(
                state(&h, &id).await,
                if attempt > max_retries {
                    "retry_limit"
                } else {
                    "cooldown"
                }
            );
            let list = h.services.relogin().list().await.unwrap();
            let recovery = &list.items[0].recovery;
            assert_eq!(recovery.retries_used, attempt - 1);
            assert_eq!(recovery.max_retries, max_retries);
        }
        h.cycle().await;
        assert_eq!(
            h.provider.relogin_requests.lock().unwrap().len(),
            (max_retries + 1) as usize
        );
    }
}

#[tokio::test]
async fn relogin_retry_edits_preserve_counts_deadlines_and_legacy_settings_clients() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    *h.provider.relogin_result.lock().unwrap() = None;
    h.cycle().await;
    let before = h.row(&id).await;
    h.services
        .relogin()
        .configure(ReloginSettings {
            max_retries: 0,
            retry_interval_minutes: 60,
            ..ReloginSettings::default()
        })
        .await
        .unwrap();
    assert_eq!(state(&h, &id).await, "retry_limit");
    h.services
        .relogin()
        .configure_update(
            serde_json::from_value::<ReloginSettingsUpdate>(
                serde_json::json!({"concurrency": 2, "paused": false}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let settings = h.services.relogin().list().await.unwrap().settings;
    assert_eq!(settings.concurrency, 2);
    assert_eq!(settings.max_retries, 0);
    assert_eq!(settings.retry_interval_minutes, 60);
    h.services
        .relogin()
        .configure(ReloginSettings {
            max_retries: 4,
            retry_interval_minutes: 60,
            ..ReloginSettings::default()
        })
        .await
        .unwrap();
    let after = h.row(&id).await;
    assert_eq!(after.automatic_attempts, before.automatic_attempts);
    assert_eq!(after.next_attempt_at, before.next_attempt_at);
    assert_eq!(state(&h, &id).await, "cooldown");
    for settings in [
        ReloginSettings {
            max_retries: 11,
            ..ReloginSettings::default()
        },
        ReloginSettings {
            retry_interval_minutes: 0,
            ..ReloginSettings::default()
        },
        ReloginSettings {
            retry_interval_minutes: 1441,
            ..ReloginSettings::default()
        },
    ] {
        assert!(h.services.relogin().configure(settings).await.is_err());
        assert_eq!(
            h.services
                .relogin()
                .list()
                .await
                .unwrap()
                .settings
                .max_retries,
            4
        );
    }
}

#[tokio::test]
async fn relogin_zero_retries_allows_new_failures_after_success_and_manual_retry() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    h.services
        .relogin()
        .configure(ReloginSettings {
            max_retries: 0,
            ..ReloginSettings::default()
        })
        .await
        .unwrap();
    h.cycle().await;
    assert!(h.row(&id).await.synced_at.is_some());
    assert_eq!(h.row(&id).await.automatic_attempts, 0);
    let mut expired = account(true);
    expired.credential_revision = revision(2);
    expired.turn_state_binding_revision = revision(2);
    h.accounts.set_accounts(vec![expired]);
    *h.provider.relogin_result.lock().unwrap() = None;
    h.cycle().await;
    assert_eq!(h.row(&id).await.automatic_attempts, 1);
    assert_eq!(state(&h, &id).await, "retry_limit");
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 2);
    *h.provider.relogin_result.lock().unwrap() = Some(credential());
    h.ready(&id).await;
    assert_eq!(state(&h, &id).await, "awaiting_push");
    assert!(
        h.services
            .relogin()
            .push(
                std::slice::from_ref(&id),
                &BTreeMap::from([(id.clone(), h.row(&id).await.revision)]),
                &context("retry-reset")
            )
            .await
            .unwrap()[0]
            .success
    );
    assert_eq!(h.row(&id).await.automatic_attempts, 0);
    assert!(h.row(&id).await.next_attempt_at.is_none());
}

#[tokio::test]
async fn relogin_terminal_failures_stop_immediately_and_manual_success_clears_them() {
    for reason in [
        ReloginStopReason::WorkspaceUnavailable,
        ReloginStopReason::AccountBanned,
    ] {
        let h = Harness::new(vec![account(true)]).await;
        let id = h.import("test@example.invalid").await;
        *h.provider.relogin_error.lock().unwrap() = Some(
            gateway_admin::ports::provider::ProviderAdminError::new(
                ProviderAdminErrorKind::Unavailable,
            )
            .with_public_message(reason.message())
            .with_relogin_stop_reason(reason),
        );
        h.cycle().await;
        let row = h.row(&id).await;
        assert_eq!(row.stop_reason, Some(reason));
        assert!(row.next_attempt_at.is_none());
        assert_eq!(state(&h, &id).await, "manual_required");
        let roundtrip: ReloginEntry =
            serde_json::from_value(serde_json::to_value(row).unwrap()).unwrap();
        h.store.rows.lock().unwrap().insert(id.clone(), roundtrip);
        h.services
            .relogin()
            .configure(ReloginSettings {
                max_retries: 10,
                retry_interval_minutes: 1,
                ..ReloginSettings::default()
            })
            .await
            .unwrap();
        h.cycle().await;
        assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 1);
        *h.provider.relogin_error.lock().unwrap() = None;
        h.ready(&id).await;
        assert!(h.row(&id).await.stop_reason.is_none());
        assert!(
            h.services
                .relogin()
                .push(
                    std::slice::from_ref(&id),
                    &BTreeMap::from([(id.clone(), h.row(&id).await.revision)]),
                    &context("terminal-recovery")
                )
                .await
                .unwrap()[0]
                .success
        );
        assert_eq!(h.row(&id).await.automatic_attempts, 0);
    }
}

#[tokio::test]
async fn relogin_legacy_workspace_failure_and_banned_pool_are_not_retryable() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    let mut json = serde_json::to_value(h.row(&id).await).unwrap();
    json.as_object_mut().unwrap().remove("stop_reason");
    let mut row: ReloginEntry = serde_json::from_value(json).unwrap();
    row.status = ReloginStatus::Failed;
    row.message = "指定工作区不可访问，未回退到个人空间".into();
    h.store.rows.lock().unwrap().insert(id.clone(), row);
    assert_eq!(state(&h, &id).await, "manual_required");
    h.cycle().await;
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
    h.services
        .relogin()
        .import(
            "test@example.invalid----test-only-password----JBSWY3DPEHPK3PXP",
            true,
        )
        .await
        .unwrap();
    assert_eq!(state(&h, &id).await, "waiting");
    let mut banned = account(true);
    banned.credential_state = CredentialState::Banned;
    banned.last_error_reason = Some(AccountErrorReason::AccountBanned);
    h.accounts.set_accounts(vec![banned]);
    assert_eq!(state(&h, &id).await, "manual_required");
    h.cycle().await;
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn relogin_uncertain_push_precedes_terminal_reason_and_setting_changes() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    let mut row = h.row(&id).await;
    row.status = ReloginStatus::Uncertain;
    row.stop_reason = Some(ReloginStopReason::WorkspaceUnavailable);
    h.store.rows.lock().unwrap().insert(id.clone(), row);
    h.services
        .relogin()
        .configure(ReloginSettings {
            max_retries: 10,
            retry_interval_minutes: 1,
            ..ReloginSettings::default()
        })
        .await
        .unwrap();
    assert_eq!(state(&h, &id).await, "uncertain");
    h.cycle().await;
    assert_eq!(h.row(&id).await.status, ReloginStatus::Uncertain);
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
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
            ..ReloginSettings::default()
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
async fn relogin_retry_settings_do_not_cancel_running_login_and_apply_at_failure() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    *h.provider.relogin_result.lock().unwrap() = None;
    *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_secs(60);
    let task = h.task.clone();
    let running = tokio::spawn(async move { task.run_cycle(cycle_context()).await });
    wait_requests(&h, 1).await;
    let before = h.row(&id).await;
    h.services
        .relogin()
        .configure(ReloginSettings {
            max_retries: 0,
            retry_interval_minutes: 17,
            ..ReloginSettings::default()
        })
        .await
        .unwrap();
    let after = h.row(&id).await;
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.automatic_attempts, 1);
    assert_eq!(after.status, ReloginStatus::Running);
    assert_eq!(h.provider.relogin_active.load(Ordering::SeqCst), 1);
    let failure_started = Utc::now();
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    running.await.unwrap().unwrap();
    let failed = h.row(&id).await;
    assert_eq!(failed.status, ReloginStatus::Failed);
    assert!(failed.next_attempt_at.unwrap() >= failure_started + Duration::minutes(17));
    assert!(failed.next_attempt_at.unwrap() <= Utc::now() + Duration::minutes(17));
    assert_eq!(state(&h, &id).await, "retry_limit");
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn relogin_scans_and_refills_while_a_slow_login_is_running() {
    let h = Harness::new(vec![]).await;
    h.services
        .relogin()
        .configure(ReloginSettings {
            concurrency: 2,
            paused: false,
            ..ReloginSettings::default()
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
            ..ReloginSettings::default()
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
