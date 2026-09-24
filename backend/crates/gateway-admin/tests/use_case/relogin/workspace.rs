use super::*;

fn choices() -> Vec<ReloginWorkspaceChoice> {
    ["workspace-team", "workspace-second"]
        .map(|id| ReloginWorkspaceChoice {
            id: id.into(),
            name: format!("Team {id}"),
            plan_type: "business".into(),
        })
        .into()
}

async fn await_selection(h: &Harness, id: &str) {
    *h.provider.relogin_error.lock().unwrap() = Some(
        gateway_admin::ports::provider::ProviderAdminError::new(
            ProviderAdminErrorKind::Unavailable,
        )
        .with_public_message("Select a workspace")
        .with_relogin_workspace_choices(choices()),
    );
    highest(h, id).await;
    assert_eq!(h.row(id).await.status, ReloginStatus::AwaitingWorkspace);
}

#[tokio::test]
async fn relogin_workspace_wait_is_durable_and_does_not_retry_or_hold_a_slot() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    await_selection(&h, &id).await;
    let waiting = h.row(&id).await;
    assert!(waiting.next_attempt_at.is_none());
    assert!(waiting.credential.is_none());
    assert_eq!(waiting.workspace_choices, choices());
    let restored: ReloginEntry =
        serde_json::from_value(serde_json::to_value(&waiting).unwrap()).unwrap();
    assert_eq!(restored.status, ReloginStatus::AwaitingWorkspace);
    assert_eq!(restored.workspace_choices, choices());
    for _ in 0..3 {
        h.cycle().await;
    }
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 1);
    assert_eq!(h.provider.relogin_active.load(Ordering::SeqCst), 0);
    let list = h.services.relogin().list().await.unwrap();
    assert_eq!(list.items[0].recovery.state, "awaiting_workspace");
    assert_eq!(list.items[0].workspace_choices, choices());
    let wire = serde_json::to_string(&list).unwrap();
    for secret in ["password", "mfa_secret", "test-only-token", "JBSWY"] {
        assert!(!wire.contains(secret));
    }

    // Waiting is not an automatic recovery candidate, even if a pool row later appears.
    h.accounts.set_accounts(vec![account(true)]);
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 1);
    h.accounts.set_accounts(vec![]);
    *h.provider.relogin_error.lock().unwrap() = None;
    h.services
        .relogin()
        .resume_workspace(&id, waiting.revision, "workspace-team")
        .await
        .unwrap();
    assert!(
        h.services
            .relogin()
            .resume_workspace(&id, waiting.revision, "workspace-team")
            .await
            .is_err()
    );
    h.cycle().await;
    let ready = h.row(&id).await;
    assert_eq!(ready.status, ReloginStatus::Ready);
    assert_eq!(ready.credential.unwrap().workspace_id, "workspace-team");
    assert!(ready.synced_at.is_none());
    assert!(ready.workspace_choices.is_empty());
    assert!(ready.manual_push_context.is_none());
    assert!(!ready.automatic_job);
    assert!(h.accounts.accounts.lock().unwrap().is_empty());
    assert!(h.accounts.audit_requests().is_empty());
    assert_eq!(
        *h.provider.relogin_requests.lock().unwrap(),
        vec![None, Some("workspace-team".into())]
    );
}

#[tokio::test]
async fn relogin_workspace_resume_rejects_stale_unknown_paused_and_uncertain_state() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    await_selection(&h, &id).await;
    let waiting = h.row(&id).await;
    for (version, workspace) in [
        (waiting.revision - 1, "workspace-team"),
        (waiting.revision, "unlisted-workspace"),
        (waiting.revision, ""),
    ] {
        assert!(
            h.services
                .relogin()
                .resume_workspace(&id, version, workspace)
                .await
                .is_err()
        );
        assert_eq!(h.row(&id).await.revision, waiting.revision);
    }
    h.services
        .relogin()
        .configure(ReloginSettings {
            paused: true,
            ..ReloginSettings::default()
        })
        .await
        .unwrap();
    assert!(
        h.services
            .relogin()
            .resume_workspace(&id, waiting.revision, "workspace-team")
            .await
            .is_err()
    );
    h.services
        .relogin()
        .configure(ReloginSettings::default())
        .await
        .unwrap();
    h.store.rows.lock().unwrap().get_mut(&id).unwrap().status = ReloginStatus::Uncertain;
    assert!(
        h.services
            .relogin()
            .resume_workspace(&id, waiting.revision, "workspace-team")
            .await
            .is_err()
    );
    assert!(h.accounts.audit_requests().is_empty());
}

#[tokio::test]
async fn relogin_workspace_resume_preserves_highest_replacement_targets_and_requires_manual_push() {
    let original = free();
    let h = Harness::new(vec![original.clone()]).await;
    let id = h.import("test@example.invalid").await;
    await_selection(&h, &id).await;
    let waiting = h.row(&id).await;
    *h.provider.relogin_error.lock().unwrap() = None;
    h.services
        .relogin()
        .resume_workspace(&id, waiting.revision, "workspace-team")
        .await
        .unwrap();
    h.cycle().await;
    let ready = h.row(&id).await;
    assert_eq!(ready.status, ReloginStatus::Ready);
    assert_eq!(ready.workspace_mode, ReloginWorkspaceMode::Highest);
    assert_eq!(ready.workspace_targets, waiting.workspace_targets);
    assert!(h.accounts.rotation_attempts.lock().unwrap().is_empty());
    assert!(!push(&h, &id, &original, false).await);
    assert!(push(&h, &id, &original, true).await);
    assert_eq!(h.accounts.accounts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn relogin_workspace_resume_cannot_adopt_changed_targets_or_mismatched_credentials() {
    for variant in ["deleted", "binding", "wrong-workspace", "original"] {
        let original = free();
        let h = Harness::new(if variant == "original" {
            vec![]
        } else {
            vec![original.clone()]
        })
        .await;
        let id = h.import("test@example.invalid").await;
        await_selection(&h, &id).await;
        let waiting = h.row(&id).await;
        if variant == "deleted" {
            h.accounts.set_accounts(vec![]);
        }
        if variant == "binding" {
            let mut changed = original.clone();
            changed.credential_revision = revision(9);
            changed.turn_state_binding_revision = revision(9);
            h.accounts.set_accounts(vec![changed]);
        }
        if variant == "original" {
            h.store
                .rows
                .lock()
                .unwrap()
                .get_mut(&id)
                .unwrap()
                .workspace_mode = ReloginWorkspaceMode::Original;
        }
        let result = h
            .services
            .relogin()
            .resume_workspace(&id, waiting.revision, "workspace-team")
            .await;
        if matches!(variant, "deleted" | "binding") {
            assert!(result.is_err(), "{variant}");
            continue;
        }
        result.unwrap();
        *h.provider.relogin_error.lock().unwrap() = None;
        if variant == "wrong-workspace" {
            h.provider
                .relogin_result
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .workspace_id = "workspace-second".into();
        }
        h.cycle().await;
        assert_eq!(
            h.row(&id).await.status,
            if variant == "original" {
                ReloginStatus::Ready
            } else {
                ReloginStatus::Failed
            }
        );
        assert!(h.accounts.audit_requests().is_empty());
    }
}

#[tokio::test]
async fn relogin_workspace_reimport_clears_choices_and_old_rows_default_them() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    await_selection(&h, &id).await;
    let old_revision = h.row(&id).await.revision;
    h.services
        .relogin()
        .import("test@example.invalid----changed----JBSWY3DPEHPK3PXP", true)
        .await
        .unwrap();
    let row = h.row(&id).await;
    assert!(row.workspace_choices.is_empty());
    assert!(row.selected_workspace_id.is_none());
    assert!(
        h.services
            .relogin()
            .resume_workspace(&id, old_revision, "workspace-team")
            .await
            .is_err()
    );
    let mut legacy = serde_json::to_value(row).unwrap();
    legacy.as_object_mut().unwrap().remove("workspace_choices");
    legacy
        .as_object_mut()
        .unwrap()
        .remove("selected_workspace_id");
    let decoded: ReloginEntry = serde_json::from_value(legacy).unwrap();
    assert!(decoded.workspace_choices.is_empty());
    assert!(decoded.selected_workspace_id.is_none());
}

#[tokio::test]
async fn relogin_workspace_locked_and_automatic_jobs_never_offer_switching() {
    for automatic in [false, true] {
        let h = Harness::new(vec![account(true)]).await;
        let id = h.import("test@example.invalid").await;
        *h.provider.relogin_error.lock().unwrap() = Some(
            gateway_admin::ports::provider::ProviderAdminError::new(
                ProviderAdminErrorKind::Unavailable,
            )
            .with_relogin_workspace_choices(choices()),
        );
        if !automatic {
            h.services
                .relogin()
                .queue(std::slice::from_ref(&id))
                .await
                .unwrap();
        }
        h.cycle().await;
        let failed = h.row(&id).await;
        assert_eq!(failed.status, ReloginStatus::Failed);
        assert!(failed.workspace_choices.is_empty());
        assert!(failed.credential.is_none());
        assert_eq!(failed.automatic_job, automatic);
        assert!(h.accounts.audit_requests().is_empty());
    }
}

#[tokio::test]
async fn relogin_workspace_cancelled_exchange_cannot_publish_old_choices() {
    for operation in ["delete", "pause", "reimport"] {
        let h = Harness::new(vec![]).await;
        let id = h.import("test@example.invalid").await;
        *h.provider.relogin_error.lock().unwrap() = Some(
            gateway_admin::ports::provider::ProviderAdminError::new(
                ProviderAdminErrorKind::Unavailable,
            )
            .with_relogin_workspace_choices(choices()),
        );
        *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_secs(30);
        h.services
            .relogin()
            .queue_with_workspace(std::slice::from_ref(&id), ReloginWorkspaceMode::Highest)
            .await
            .unwrap();
        let task = h.task.clone();
        let running = tokio::spawn(async move { task.run_cycle(cycle_context()).await });
        h.wait_running().await;
        match operation {
            "delete" => h
                .services
                .relogin()
                .delete(std::slice::from_ref(&id))
                .await
                .unwrap(),
            "pause" => h
                .services
                .relogin()
                .configure(ReloginSettings {
                    paused: true,
                    ..ReloginSettings::default()
                })
                .await
                .unwrap(),
            _ => {
                h.services
                    .relogin()
                    .import("test@example.invalid----changed----JBSWY3DPEHPK3PXP", true)
                    .await
                    .unwrap();
            }
        }
        tokio::time::timeout(std::time::Duration::from_secs(3), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let rows = h.store.entries().await.unwrap();
        assert!(
            rows.iter()
                .all(|row| row.status != ReloginStatus::AwaitingWorkspace
                    && row.workspace_choices.is_empty()
                    && row.credential.is_none())
        );
        assert_eq!(h.provider.relogin_active.load(Ordering::SeqCst), 0);
        assert!(h.accounts.audit_requests().is_empty());
    }
}

fn free() -> AccountRecord {
    let mut account = account(false);
    account.upstream_account_id = Some("workspace-free".into());
    account.plan_type = Some("free".into());
    account.custom_name = Some("Preserved name".into());
    account.enabled = false;
    account
}

async fn highest(h: &Harness, id: &str) {
    let result = h
        .services
        .relogin()
        .queue_with_workspace(&[id.into()], ReloginWorkspaceMode::Highest)
        .await
        .unwrap();
    assert!(result[0].success, "{}", result[0].message);
    h.cycle().await;
}

async fn push(h: &Harness, id: &str, target: &AccountRecord, switch_workspace: bool) -> bool {
    let row = h.row(id).await;
    h.services
        .relogin()
        .push_with_selection(
            &[id.into()],
            &BTreeMap::from([(id.into(), row.revision)]),
            None,
            Some("Must not rename existing".into()),
            &BTreeMap::from([(
                id.into(),
                ReloginPushSelection {
                    account_id: target.id.clone(),
                    switch_workspace,
                },
            )]),
            &context("workspace-confirm"),
        )
        .await
        .unwrap()[0]
        .success
}

#[tokio::test]
async fn relogin_highest_switches_free_in_place_only_after_explicit_confirmation() {
    let original = free();
    let h = Harness::new(vec![original.clone()]).await;
    let id = h.import("test@example.invalid").await;
    h.services
        .relogin()
        .workspace(&id, Some("workspace-free".into()))
        .await
        .unwrap();
    highest(&h, &id).await;
    let row = h.row(&id).await;
    assert_eq!(row.status, ReloginStatus::Ready);
    assert_eq!(*h.provider.relogin_requests.lock().unwrap(), vec![None]);
    assert!(h.accounts.rotation_attempts.lock().unwrap().is_empty());
    assert!(
        !h.services
            .relogin()
            .push(
                std::slice::from_ref(&id),
                &BTreeMap::from([(id.clone(), row.revision)]),
                &context("unconfirmed"),
            )
            .await
            .unwrap()[0]
            .success
    );
    assert!(!push(&h, &id, &original, false).await);
    assert!(push(&h, &id, &original, true).await);
    let entry = h.row(&id).await;
    assert!(entry.synced_at.is_some());
    assert_eq!(entry.target.as_ref().unwrap().account_id, original.id);
    assert_eq!(
        entry.target.as_ref().unwrap().workspace_id,
        "workspace-team"
    );
    assert_eq!(
        entry.preferred_workspace_id.as_deref(),
        Some("workspace-team")
    );
    assert_eq!(entry.workspace_mode, ReloginWorkspaceMode::Original);
    assert!(entry.workspace_targets.is_empty());
    let listed = h.services.relogin().list().await.unwrap();
    assert_eq!(listed.items[0].pool_status, "synced");
    assert_eq!(listed.items[0].relogin_count, Some(1));
    let pool = h.accounts.accounts.lock().unwrap();
    assert_eq!(pool.len(), 1);
    assert_eq!(pool[0].custom_name, original.custom_name);
    assert_eq!(pool[0].enabled, original.enabled);
    assert_eq!(pool[0].groups, original.groups);
    assert_eq!(pool[0].concurrency_limit, original.concurrency_limit);
}

#[tokio::test]
async fn relogin_highest_existing_destination_requires_selecting_that_original_row() {
    let original = free();
    let mut business = account(false);
    business.id = "acct_business".into();
    let h = Harness::new(vec![original.clone(), business.clone()]).await;
    let id = h.import("test@example.invalid").await;
    highest(&h, &id).await;
    let view = h.services.relogin().list().await.unwrap();
    let targets = &view.items[0].push_targets;
    assert_eq!(targets.len(), 2);
    assert!(
        !targets
            .iter()
            .find(|target| target.account_id == original.id)
            .unwrap()
            .available
    );
    assert!(
        targets
            .iter()
            .find(|target| target.account_id == business.id)
            .unwrap()
            .available
    );
    assert!(!push(&h, &id, &original, true).await);
    assert!(push(&h, &id, &business, false).await);
    assert_eq!(h.accounts.accounts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn relogin_highest_rejects_other_user_and_changed_deleted_or_racing_targets() {
    for change in ["other-user", "binding", "deleted", "new-destination"] {
        let original = free();
        let h = Harness::new(vec![original.clone()]).await;
        let id = h.import("test@example.invalid").await;
        if change == "other-user" {
            h.provider
                .relogin_result
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .user_id = "different-user".into();
        }
        highest(&h, &id).await;
        if change == "other-user" {
            assert_eq!(h.row(&id).await.status, ReloginStatus::Failed);
            continue;
        }
        match change {
            "binding" => {
                let mut changed = original.clone();
                changed.credential_revision = revision(9);
                changed.turn_state_binding_revision = revision(9);
                h.accounts.set_accounts(vec![changed]);
            }
            "deleted" => h.accounts.set_accounts(vec![]),
            "new-destination" => {
                let mut business = account(false);
                business.id = "acct_racing".into();
                h.accounts.set_accounts(vec![original.clone(), business]);
            }
            _ => unreachable!(),
        }
        assert!(!push(&h, &id, &original, true).await, "{change}");
        assert_eq!(h.row(&id).await.status, ReloginStatus::Ready);
        assert!(h.accounts.rotation_attempts.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn relogin_highest_new_account_uses_create_only_and_original_mode_remains_locked() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    highest(&h, &id).await;
    let row = h.row(&id).await;
    assert!(
        h.services
            .relogin()
            .push(
                std::slice::from_ref(&id),
                &BTreeMap::from([(id.clone(), row.revision)]),
                &context("new-highest"),
            )
            .await
            .unwrap()[0]
            .success
    );

    let h = Harness::new(vec![free()]).await;
    let id = h.import("test@example.invalid").await;
    h.services
        .relogin()
        .queue(std::slice::from_ref(&id))
        .await
        .unwrap();
    h.cycle().await;
    assert_eq!(
        *h.provider.relogin_requests.lock().unwrap(),
        vec![Some("workspace-free".into())]
    );
    assert_eq!(h.row(&id).await.status, ReloginStatus::Failed);
    assert!(h.accounts.rotation_attempts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn relogin_highest_defaults_legacy_rows_and_rechecks_claim_without_direct_proxy_fallback() {
    let original = free();
    let h = Harness::new(vec![original.clone()]).await;
    let id = h.import("test@example.invalid").await;
    let mut legacy = serde_json::to_value(h.row(&id).await).unwrap();
    legacy.as_object_mut().unwrap().remove("workspace_mode");
    legacy.as_object_mut().unwrap().remove("workspace_targets");
    let decoded: ReloginEntry = serde_json::from_value(legacy).unwrap();
    assert_eq!(decoded.workspace_mode, ReloginWorkspaceMode::Original);
    assert!(decoded.workspace_targets.is_empty());
    h.services
        .relogin()
        .queue_with_workspace(std::slice::from_ref(&id), ReloginWorkspaceMode::Highest)
        .await
        .unwrap();
    h.accounts.set_accounts(vec![]);
    h.cycle().await;
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
    assert_eq!(h.row(&id).await.status, ReloginStatus::Failed);
}

#[tokio::test]
async fn relogin_highest_cookie_races_and_unknown_commit_keep_existing_fences() {
    for ambiguous in [false, true] {
        let original = free();
        let h = Harness::new(vec![original.clone()]).await;
        let id = h.import("test@example.invalid").await;
        highest(&h, &id).await;
        if ambiguous {
            h.accounts.fail_next_commit();
        } else {
            let mut cookie = original.clone();
            cookie.credential_revision = revision(original.credential_revision.get() + 1);
            h.accounts.set_accounts(vec![cookie]);
        }
        let result = push(&h, &id, &original, true).await;
        assert_eq!(result, !ambiguous);
        if ambiguous {
            assert_eq!(h.row(&id).await.status, ReloginStatus::Uncertain);
            let before = h.accounts.rotation_attempts.lock().unwrap().len();
            assert!(!push(&h, &id, &original, true).await);
            h.cycle().await;
            assert_eq!(h.accounts.rotation_attempts.lock().unwrap().len(), before);
        }
    }
}
