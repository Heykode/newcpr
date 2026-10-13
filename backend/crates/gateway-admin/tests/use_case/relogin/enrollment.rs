use super::*;

fn purchase_cost(amount: &str) -> gateway_admin::model::account_purchase::AccountPurchaseUpdate {
    gateway_admin::model::account_purchase::AccountPurchaseUpdate {
        amount_cny: Some(amount.into()),
        cycle_start: chrono::NaiveDate::from_ymd_opt(2026, 1, 31),
    }
}

fn intent() -> ReloginEnrollment {
    let mut settings = template_config().settings().unwrap();
    settings.purchase_cost = Some(purchase_cost("50.125"));
    settings.custom_name = Some("Imported via 2FA".into());
    settings.responses_upstream = Some(gateway_core::account::ResponsesUpstream::Excel);
    settings.excel_403_action = Some(gateway_core::account::Excel403Action::DisableExcel);
    settings.excel_cache_creation_as_input = Some(true);
    ReloginEnrollment::new(settings, None, context("enroll-account")).unwrap()
}

const MATERIAL: &str = "test@example.invalid----test-only-password----JBSWY3DPEHPK3PXP";

async fn expire_wait(h: &Harness, id: &str) {
    let mut row = h.row(id).await;
    let revision = row.revision;
    row.revision += 1;
    row.next_attempt_at = Some(Utc::now() - Duration::seconds(1));
    h.store.save(&row, Some(revision)).await.unwrap();
}

#[tokio::test]
async fn enrollment_retry_keeps_the_selected_proxy_on_every_login() {
    let proxies = Arc::new(super::super::proxies::TestProxies {
        events: Some(events()),
        ..Default::default()
    });
    let h = Harness::with_proxies(vec![], Some(proxies)).await;
    let mut enrollment = intent();
    enrollment.config.outbound_proxy_id = Some("proxy-fixture".into());
    let ids = h
        .services
        .relogin()
        .enroll(MATERIAL, false, enrollment.clone())
        .await
        .unwrap();
    *h.provider.relogin_result.lock().unwrap() = None;
    h.cycle().await;
    expire_wait(&h, &ids[0]).await;
    *h.provider.relogin_result.lock().unwrap() = Some(credential());
    h.cycle().await;
    {
        let proxies = h.provider.relogin_proxies.lock().unwrap();
        assert_eq!(proxies.len(), 2);
        assert!(proxies[0].is_some());
        assert_eq!(proxies[0], proxies[1]);
    }
    assert!(h.row(&ids[0]).await.synced_at.is_some());
    assert_eq!(
        h.accounts.import_settings(),
        vec![Some(enrollment.settings().unwrap())]
    );
}

#[tokio::test]
async fn unavailable_enrollment_proxy_uses_retry_budget_without_direct_fallback() {
    let h = Harness::new(vec![]).await;
    let ids = h
        .services
        .relogin()
        .enroll(MATERIAL, false, intent())
        .await
        .unwrap();
    let mut row = h.row(&ids[0]).await;
    row.enrollment.as_mut().unwrap().config.outbound_proxy_id = Some("missing-proxy".into());
    h.store.save(&row, Some(row.revision)).await.unwrap();
    for attempt in 1..=3 {
        let before = Utc::now();
        h.cycle().await;
        let row = h.row(&ids[0]).await;
        assert_eq!(row.automatic_attempts, attempt);
        assert!(row.next_attempt_at.unwrap() >= before + Duration::seconds(30));
        h.cycle().await;
        assert_eq!(h.row(&ids[0]).await.automatic_attempts, attempt);
        expire_wait(&h, &ids[0]).await;
    }
    h.cycle().await;
    assert_eq!(h.row(&ids[0]).await.automatic_attempts, 3);
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
    assert!(h.accounts.import_settings().is_empty());
}

#[tokio::test]
async fn enrollment_uses_shared_retry_count_and_seconds_without_resetting_the_budget() {
    for retries in [0, 2, 10] {
        let h = Harness::new(vec![]).await;
        h.services
            .relogin()
            .configure(ReloginSettings {
                max_retries: retries,
                retry_interval_seconds: 30,
                ..ReloginSettings::default()
            })
            .await
            .unwrap();
        let ids = h
            .services
            .relogin()
            .enroll(MATERIAL, false, intent())
            .await
            .unwrap();
        *h.provider.relogin_result.lock().unwrap() = None;
        for attempt in 1..=retries + 1 {
            let before = Utc::now();
            h.cycle().await;
            let after = Utc::now();
            let row = h.row(&ids[0]).await;
            assert_eq!(row.automatic_attempts, attempt);
            assert!(!row.automatic_job);
            assert!(row.enrollment.is_some());
            assert!(row.next_attempt_at.unwrap() >= before + Duration::seconds(30));
            assert!(row.next_attempt_at.unwrap() <= after + Duration::seconds(30));
            let list = h.services.relogin().list().await.unwrap();
            assert_eq!(list.items[0].recovery.retries_used, attempt - 1);
            assert_eq!(
                list.items[0].recovery.state,
                if attempt > retries {
                    "retry_limit"
                } else {
                    "cooldown"
                }
            );
            h.cycle().await;
            assert_eq!(
                h.provider.relogin_requests.lock().unwrap().len(),
                attempt as usize
            );
            expire_wait(&h, &ids[0]).await;
        }
        h.cycle().await;
        assert_eq!(
            h.provider.relogin_requests.lock().unwrap().len(),
            (retries + 1) as usize
        );
        assert!(h.accounts.import_settings().is_empty());
    }
}

#[tokio::test]
async fn enrollment_retry_survives_restart_and_preserves_workspace_and_import_settings() {
    let h = Harness::new(vec![]).await;
    let ids = h
        .services
        .relogin()
        .enroll(MATERIAL, false, intent())
        .await
        .unwrap();
    let mut row = h.row(&ids[0]).await;
    row.selected_workspace_id = Some("workspace-team".into());
    h.store.save(&row, Some(row.revision)).await.unwrap();
    *h.provider.relogin_result.lock().unwrap() = None;
    h.cycle().await;
    let failed = h.row(&ids[0]).await;
    let restored: ReloginEntry =
        serde_json::from_value(serde_json::to_value(&failed).unwrap()).unwrap();
    let restarted = Harness::new(vec![]).await;
    restarted.store.save(&restored, None).await.unwrap();
    restarted.cycle().await;
    assert!(
        restarted
            .provider
            .relogin_requests
            .lock()
            .unwrap()
            .is_empty()
    );
    expire_wait(&restarted, &ids[0]).await;
    restarted.cycle().await;
    let ready = restarted.row(&ids[0]).await;
    assert!(ready.synced_at.is_some(), "{}", ready.message);
    assert_eq!(ready.automatic_attempts, 0);
    assert!(ready.next_attempt_at.is_none());
    assert!(ready.enrollment.is_none());
    assert_eq!(
        *restarted.provider.relogin_requests.lock().unwrap(),
        vec![Some("workspace-team".into())]
    );
    assert_eq!(
        restarted.accounts.import_settings(),
        vec![Some(intent().settings().unwrap())]
    );
    restarted.cycle().await;
    assert_eq!(restarted.accounts.import_settings().len(), 1);
}

#[tokio::test]
async fn enrollment_retry_keeps_pause_terminal_and_ambiguous_push_fences() {
    for state in [
        "paused",
        "terminal",
        "uncertain",
        "verified",
        "workspace",
        "pool_changed",
        "disabled",
    ] {
        let h = Harness::new(vec![]).await;
        let ids = h
            .services
            .relogin()
            .enroll(MATERIAL, false, intent())
            .await
            .unwrap();
        *h.provider.relogin_result.lock().unwrap() = None;
        h.cycle().await;
        expire_wait(&h, &ids[0]).await;
        let mut row = h.row(&ids[0]).await;
        match state {
            "paused" => h.store.settings.lock().unwrap().paused = true,
            "terminal" => row.stop_reason = Some(ReloginStopReason::AccountBanned),
            "uncertain" => row.status = ReloginStatus::Uncertain,
            "verified" => row.credential = Some(credential()),
            "workspace" => row.status = ReloginStatus::AwaitingWorkspace,
            "pool_changed" => h.accounts.set_accounts(vec![account(false)]),
            "disabled" => row.automatic = false,
            _ => unreachable!(),
        }
        h.store.save(&row, Some(row.revision)).await.unwrap();
        h.cycle().await;
        assert_eq!(
            h.provider.relogin_requests.lock().unwrap().len(),
            1,
            "{state}"
        );
        assert!(h.accounts.import_settings().is_empty(), "{state}");
        if state == "paused" {
            h.store.settings.lock().unwrap().paused = false;
            h.cycle().await;
            assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn legacy_failed_enrollment_counts_its_initial_attempt_and_obeys_zero_retries() {
    for retries in [0, 1] {
        let h = Harness::new(vec![]).await;
        let ids = h
            .services
            .relogin()
            .enroll(MATERIAL, false, intent())
            .await
            .unwrap();
        let mut row = h.row(&ids[0]).await;
        row.status = ReloginStatus::Failed;
        row.automatic_attempts = 0;
        h.store.save(&row, Some(row.revision)).await.unwrap();
        h.store.settings.lock().unwrap().max_retries = retries;
        *h.provider.relogin_result.lock().unwrap() = None;
        h.cycle().await;
        h.cycle().await;
        assert_eq!(
            h.provider.relogin_requests.lock().unwrap().len(),
            retries as usize
        );
        if retries > 0 {
            assert_eq!(h.row(&ids[0]).await.automatic_attempts, 2);
        }
        assert_eq!(
            h.services.relogin().list().await.unwrap().items[0]
                .recovery
                .state,
            "retry_limit"
        );
    }
}

#[test]
fn enrollment_serializes_and_replays_purchase_cost_without_changing_other_settings() {
    for cost in [
        None,
        Some(purchase_cost("0")),
        Some(purchase_cost("50.1234567890")),
        Some(
            gateway_admin::model::account_purchase::AccountPurchaseUpdate {
                amount_cny: Some("25".into()),
                cycle_start: None,
            },
        ),
    ] {
        let mut settings = template_config().settings().unwrap();
        settings.purchase_cost = cost;
        let enrollment =
            ReloginEnrollment::new(settings.clone(), None, context("cost-enroll")).unwrap();
        assert_eq!(enrollment.settings().unwrap(), settings);
        let replay: ReloginEnrollment =
            serde_json::from_value(serde_json::to_value(enrollment).unwrap()).unwrap();
        assert_eq!(replay.settings().unwrap(), settings);
    }
}

#[test]
fn enrollment_rejects_invalid_purchase_cost_on_creation_and_replay() {
    let mut future_cost = purchase_cost("25");
    future_cost.cycle_start = Some((Utc::now() + chrono::Duration::days(2)).date_naive());
    for cost in [
        purchase_cost("-1"),
        purchase_cost("NaN"),
        purchase_cost("10000000000"),
        purchase_cost("0.00000000001"),
        future_cost,
    ] {
        let mut settings = template_config().settings().unwrap();
        settings.purchase_cost = Some(cost.clone());
        assert!(ReloginEnrollment::new(settings, None, context("cost-enroll")).is_err());
        let mut serialized = serde_json::to_value(intent()).unwrap();
        serialized["purchase_cost"] = serde_json::to_value(cost).unwrap();
        let replay: ReloginEnrollment = serde_json::from_value(serialized).unwrap();
        assert!(replay.settings().is_err());
    }
}

#[test]
fn legacy_enrollment_without_purchase_cost_still_restores_other_settings() {
    let mut serialized = serde_json::to_value(intent()).unwrap();
    serialized.as_object_mut().unwrap().remove("purchase_cost");
    let replay: ReloginEnrollment = serde_json::from_value(serialized).unwrap();
    let restored = replay.settings().unwrap();
    assert!(restored.purchase_cost.is_none());
    assert_eq!(restored.custom_name.as_deref(), Some("Imported via 2FA"));
    assert_eq!(
        restored.responses_upstream,
        Some(gateway_core::account::ResponsesUpstream::Excel)
    );
}

#[test]
fn enrollment_serializes_and_replays_all_egress_intents() {
    use gateway_core::provider_ports::egress::EgressMode;
    for mode in [
        None,
        Some(None),
        Some(Some(EgressMode::Unchanged)),
        Some(Some(EgressMode::FixedIpv6Reuse)),
        Some(Some(EgressMode::RandomIpv6Reuse)),
        Some(Some(EgressMode::FixedIpv6Fresh)),
        Some(Some(EgressMode::RandomIpv6Fresh)),
    ] {
        for clear in [false, true] {
            let mut settings = template_config().settings().unwrap();
            settings.egress_mode = mode;
            settings.clear_outbound_proxy = clear;
            let intent = ReloginEnrollment::new(settings, None, context("egress-enroll")).unwrap();
            let replay: ReloginEnrollment =
                serde_json::from_value(serde_json::to_value(intent).unwrap()).unwrap();
            assert_eq!(replay.settings().unwrap().egress_mode, mode);
            assert_eq!(replay.settings().unwrap().clear_outbound_proxy, clear);
        }
    }
    let mut conflicting = template_config().settings().unwrap();
    conflicting.clear_outbound_proxy = true;
    assert!(
        ReloginEnrollment::new(
            conflicting,
            Some("proxy-fixture".into()),
            context("conflict")
        )
        .is_err()
    );
}

#[tokio::test]
async fn enrollment_resumes_selected_workspace_and_keeps_the_import_intent() {
    let h = Harness::new(vec![]).await;
    let ids = h
        .services
        .relogin()
        .enroll(MATERIAL, false, intent())
        .await
        .unwrap();
    *h.provider.relogin_error.lock().unwrap() = Some(
        gateway_admin::ports::provider::ProviderAdminError::new(
            ProviderAdminErrorKind::Unavailable,
        )
        .with_relogin_workspace_choices(
            ["workspace-team", "workspace-other"]
                .into_iter()
                .map(|id| ReloginWorkspaceChoice {
                    id: id.into(),
                    name: id.into(),
                    plan_type: "business".into(),
                })
                .collect(),
        ),
    );
    h.cycle().await;
    let waiting = h.row(&ids[0]).await;
    assert_eq!(waiting.status, ReloginStatus::AwaitingWorkspace);
    assert!(waiting.enrollment.is_some());
    assert!(h.accounts.import_settings().is_empty());
    *h.provider.relogin_error.lock().unwrap() = None;
    h.services
        .relogin()
        .resume_workspace(&ids[0], waiting.revision, "workspace-team")
        .await
        .unwrap();
    *h.provider.relogin_result.lock().unwrap() = None;
    h.cycle().await;
    assert_eq!(h.row(&ids[0]).await.automatic_attempts, 1);
    assert_eq!(
        h.row(&ids[0]).await.selected_workspace_id.as_deref(),
        Some("workspace-team")
    );
    expire_wait(&h, &ids[0]).await;
    *h.provider.relogin_result.lock().unwrap() = Some(credential());
    h.cycle().await;
    assert!(h.row(&ids[0]).await.synced_at.is_some());
    assert_eq!(h.accounts.import_settings().len(), 1);
    assert_eq!(
        h.accounts.import_settings()[0]
            .as_ref()
            .unwrap()
            .purchase_cost,
        Some(purchase_cost("50.125"))
    );
}

#[tokio::test]
async fn verified_enrollment_waits_for_pause_to_end_without_logging_in_again() {
    let h = Harness::new(vec![]).await;
    let ids = h
        .services
        .relogin()
        .enroll(MATERIAL, false, intent())
        .await
        .unwrap();
    let mut entry = h.row(&ids[0]).await;
    let revision = entry.revision;
    entry.revision += 1;
    entry.status = ReloginStatus::Ready;
    entry.credential = Some(credential());
    h.store.save(&entry, Some(revision)).await.unwrap();
    h.store.settings.lock().unwrap().paused = true;
    h.cycle().await;
    assert!(h.accounts.import_settings().is_empty());
    h.store.settings.lock().unwrap().paused = false;
    h.cycle().await;
    assert!(h.row(&ids[0]).await.synced_at.is_some());
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
    assert_eq!(
        h.accounts.import_settings()[0]
            .as_ref()
            .unwrap()
            .purchase_cost,
        Some(purchase_cost("50.125"))
    );
}

#[tokio::test]
async fn enrollment_queues_then_imports_with_confirmed_settings_and_retains_2fa() {
    let h = Harness::new(vec![]).await;
    let ids = h
        .services
        .relogin()
        .enroll(MATERIAL, false, intent())
        .await
        .unwrap();
    let entry = h.row(&ids[0]).await;
    assert_eq!(entry.status, ReloginStatus::Queued);
    assert!(entry.enrollment.is_some());
    let restored: ReloginEntry =
        serde_json::from_value(serde_json::to_value(&entry).unwrap()).unwrap();
    assert_eq!(
        restored
            .enrollment
            .as_ref()
            .unwrap()
            .settings()
            .unwrap()
            .purchase_cost,
        Some(purchase_cost("50.125"))
    );
    h.store.save(&restored, Some(entry.revision)).await.unwrap();
    assert!(h.accounts.import_settings().is_empty());
    assert!(
        h.services
            .relogin()
            .enroll(MATERIAL, true, intent())
            .await
            .is_err()
    );
    let wire = serde_json::to_string(&h.services.relogin().list().await.unwrap()).unwrap();
    assert!(!wire.contains("test-only-password"));
    assert!(!wire.contains("JBSWY3DPEHPK3PXP"));
    h.cycle().await;
    let ready = h.row(&ids[0]).await;
    assert!(ready.synced_at.is_some(), "{}", ready.message);
    assert!(ready.enrollment.is_none());
    assert!(ready.automatic);
    assert_eq!(ready.password, "test-only-password");
    assert_eq!(
        h.accounts.import_settings(),
        vec![Some(intent().settings().unwrap())]
    );
    assert_eq!(h.accounts.audit_requests(), vec!["enroll-account"]);
    assert_eq!(
        h.accounts.import_settings()[0]
            .as_ref()
            .unwrap()
            .purchase_cost,
        Some(purchase_cost("50.125"))
    );
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn enrollment_failure_never_creates_account_and_paused_queue_rejects_new_work() {
    let h = Harness::new(vec![]).await;
    h.store.settings.lock().unwrap().paused = true;
    assert!(
        h.services
            .relogin()
            .enroll(MATERIAL, false, intent())
            .await
            .is_err()
    );
    assert!(h.store.entries().await.unwrap().is_empty());
    h.store.settings.lock().unwrap().paused = false;
    let ids = h
        .services
        .relogin()
        .enroll(MATERIAL, false, intent())
        .await
        .unwrap();
    *h.provider.relogin_error.lock().unwrap() =
        Some(gateway_admin::ports::provider::ProviderAdminError::new(
            ProviderAdminErrorKind::Unavailable,
        ));
    h.cycle().await;
    assert_eq!(h.row(&ids[0]).await.status, ReloginStatus::Failed);
    assert!(h.accounts.import_settings().is_empty());
}

#[tokio::test]
async fn enrollment_updates_same_identity_without_changing_existing_settings() {
    let h = Harness::new(vec![account(false)]).await;
    let ids = h
        .services
        .relogin()
        .enroll(MATERIAL, false, intent())
        .await
        .unwrap();
    h.cycle().await;
    let row = h.row(&ids[0]).await;
    assert!(row.synced_at.is_some(), "{}", row.message);
    assert!(h.accounts.import_settings().is_empty());
}

#[tokio::test]
async fn exports_require_selected_rows_and_never_login_implicitly() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    assert!(
        h.services
            .relogin()
            .export(&[], ReloginExportFormat::TwoFa, &context("export"))
            .await
            .is_err()
    );
    let ids = [id.clone()];
    let output = h
        .services
        .relogin()
        .export(&ids, ReloginExportFormat::TwoFa, &context("export"))
        .await
        .unwrap();
    assert_eq!(output.files.len(), 1);
    assert_eq!(output.files[0].content.trim(), MATERIAL);
    assert!(
        h.services
            .relogin()
            .export(&ids, ReloginExportFormat::Json, &context("export"))
            .await
            .is_err()
    );
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
    h.ready(&id).await;
    let output = h
        .services
        .relogin()
        .export(&ids, ReloginExportFormat::Json, &context("export"))
        .await
        .unwrap();
    assert_eq!(output.files.len(), 1);
    assert!(output.files[0].content.contains("test-only-token"));
    assert!(!output.files[0].content.contains("password"));
    assert!(!output.files[0].content.contains("JBSWY"));
}
