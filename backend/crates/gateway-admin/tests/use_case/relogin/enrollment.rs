use super::*;

fn intent() -> ReloginEnrollment {
    let mut settings = template_config().settings().unwrap();
    settings.custom_name = Some("Imported via 2FA".into());
    settings.responses_upstream = Some(gateway_core::account::ResponsesUpstream::Excel);
    settings.excel_403_action = Some(gateway_core::account::Excel403Action::DisableExcel);
    settings.excel_cache_creation_as_input = Some(true);
    ReloginEnrollment::new(settings, None, context("enroll-account")).unwrap()
}

const MATERIAL: &str = "test@example.invalid----test-only-password----JBSWY3DPEHPK3PXP";

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
    h.cycle().await;
    assert!(h.row(&ids[0]).await.synced_at.is_some());
    assert_eq!(h.accounts.import_settings().len(), 1);
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
