use super::*;
use gateway_core::account::{CredentialSelection, CredentialTarget};

#[tokio::test]
async fn sibling_can_queue_while_codex_runs_without_starting_a_concurrent_login() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("test@example.invalid").await;
    *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_millis(80);
    h.services
        .relogin()
        .queue(std::slice::from_ref(&id))
        .await
        .unwrap();
    let task = Arc::clone(&h.task);
    let running = tokio::spawn(async move { task.run_cycle(cycle_context()).await });
    h.wait_running().await;
    let excel = h
        .services
        .relogin()
        .credential_channel(CredentialTarget::Excel)
        .unwrap();
    assert!(excel.queue(std::slice::from_ref(&id)).await.unwrap()[0].success);
    running.await.unwrap().unwrap();
    h.cycle().await;
    let row = h.row(&id).await;
    assert_eq!(row.status, ReloginStatus::Ready);
    assert_eq!(row.excel.unwrap().status, ReloginStatus::Ready);
    assert_eq!(h.provider.relogin_peak.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn excel_acquisition_preserves_codex_cache_and_safe_history_labels() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    let before = h.row(&id).await;
    let excel = h
        .services
        .relogin()
        .credential_channel(CredentialTarget::Excel)
        .unwrap();
    assert!(excel.queue(std::slice::from_ref(&id)).await.unwrap()[0].success);
    h.cycle().await;
    let after = h.row(&id).await;
    assert_eq!(after.status, ReloginStatus::Ready);
    assert_eq!(
        after.credential.unwrap().document,
        before.credential.unwrap().document
    );
    assert_eq!(
        after.last_attempt.unwrap().id,
        before.last_attempt.unwrap().id
    );
    let channel = after.excel.unwrap();
    assert_eq!(channel.status, ReloginStatus::Ready);
    assert!(channel.credential.is_some());
    assert_eq!(
        channel.last_attempt.unwrap().credential_target,
        CredentialTarget::Excel
    );
    let history = h.services.relogin().history(&id, None).await.unwrap();
    assert_eq!(history.items.len(), 2);
    let wire = serde_json::to_string(&h.services.relogin().list().await.unwrap()).unwrap();
    assert!(wire.contains("\"credentialTarget\":\"excel\""));
    assert!(!wire.contains("test-only-token"));
    assert!(!wire.contains("test-only-password"));
}

#[tokio::test]
async fn failed_excel_relogin_keeps_prior_cached_material() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("test@example.invalid").await;
    let excel = h
        .services
        .relogin()
        .credential_channel(CredentialTarget::Excel)
        .unwrap();
    excel.queue(std::slice::from_ref(&id)).await.unwrap();
    h.cycle().await;
    let cached = h.row(&id).await.excel.unwrap().credential.unwrap();
    *h.provider.relogin_result.lock().unwrap() = None;
    excel.queue(std::slice::from_ref(&id)).await.unwrap();
    h.cycle().await;
    let row = h.row(&id).await;
    assert!(row.credential.is_none());
    let channel = row.excel.unwrap();
    assert_eq!(channel.status, ReloginStatus::Failed);
    assert_eq!(channel.credential.unwrap().document, cached.document);
}

#[tokio::test]
async fn codex_setting_change_does_not_cancel_running_excel_attempt() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("test@example.invalid").await;
    *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_millis(80);
    let excel = h
        .services
        .relogin()
        .credential_channel(CredentialTarget::Excel)
        .unwrap();
    excel.queue(std::slice::from_ref(&id)).await.unwrap();
    let task = Arc::clone(&h.task);
    let running = tokio::spawn(async move { task.run_cycle(cycle_context()).await });
    h.wait_running().await;
    h.services
        .relogin()
        .automatic(std::slice::from_ref(&id), false)
        .await
        .unwrap();
    running.await.unwrap().unwrap();
    let row = h.row(&id).await;
    assert!(!row.automatic);
    assert_eq!(row.excel.unwrap().status, ReloginStatus::Ready);
}

#[tokio::test]
async fn global_pause_cancels_excel_without_clearing_codex_cache() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_secs(3);
    {
        let mut rows = h.store.rows.lock().unwrap();
        rows.get_mut(&id).unwrap().manual_push_context = Some(context("pending-codex-push"));
    }
    h.services
        .relogin()
        .credential_channel(CredentialTarget::Excel)
        .unwrap()
        .queue(std::slice::from_ref(&id))
        .await
        .unwrap();
    let task = Arc::clone(&h.task);
    let running = tokio::spawn(async move { task.run_cycle(cycle_context()).await });
    h.wait_running().await;
    let mut settings = h.services.relogin().list().await.unwrap().settings;
    settings.paused = true;
    h.services.relogin().configure(settings).await.unwrap();
    running.await.unwrap().unwrap();
    let row = h.row(&id).await;
    assert_eq!(row.status, ReloginStatus::Ready);
    assert!(row.credential.is_some());
    assert_eq!(row.manual_push_context, Some(context("pending-codex-push")));
    assert_eq!(row.excel.unwrap().status, ReloginStatus::Failed);
}

#[tokio::test]
async fn account_menu_both_channels_queue_atomically_with_independent_targets() {
    let account = account(false);
    let h = Harness::new(vec![account.clone()]).await;
    let id = h.import("test@example.invalid").await;
    let revision = h.row(&id).await.revision;
    let targets = [
        ReloginTarget::from_account_for_target(&account, CredentialTarget::Codex).unwrap(),
        ReloginTarget::from_account_for_target(&account, CredentialTarget::Excel).unwrap(),
    ];
    h.services
        .relogin()
        .queue_account_channels(&id, revision, &targets, &context("both-menu"))
        .await
        .unwrap();
    let row = h.row(&id).await;
    assert_eq!(row.revision, revision + 1);
    assert_eq!(row.status, ReloginStatus::Queued);
    let excel = row.excel.unwrap();
    assert_eq!(excel.status, ReloginStatus::Queued);
    assert_eq!(excel.target.unwrap().credential_revision, 0);
    assert!(
        h.services
            .relogin()
            .queue_account_channels(&id, revision, &targets, &context("stale-menu"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn missing_excel_export_is_reported_without_discarding_codex_export() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    let native = h
        .services
        .relogin()
        .export(
            std::slice::from_ref(&id),
            ReloginExportFormat::Json,
            &context("export-native"),
        )
        .await
        .unwrap();
    let excel = h
        .services
        .relogin()
        .credential_channel(CredentialTarget::Excel)
        .unwrap()
        .export(&[id], ReloginExportFormat::Json, &context("export-excel"))
        .await
        .unwrap();
    assert_eq!(native.files.len(), 1);
    assert!(excel.files.is_empty());
    assert_eq!(excel.skipped.len(), 1);
    assert_eq!(excel.skipped[0].credential_target, CredentialTarget::Excel);
}

async fn ready_excel(h: &Harness, id: &str) {
    h.services
        .relogin()
        .credential_channel(CredentialTarget::Excel)
        .unwrap()
        .queue(&[id.to_owned()])
        .await
        .unwrap();
    h.cycle().await;
    assert_eq!(h.row(id).await.excel.unwrap().status, ReloginStatus::Ready);
}

#[tokio::test]
async fn excel_push_only_attaches_sidecar_and_preserves_codex_cached_material() {
    let original = account(false);
    let h = Harness::new(vec![original.clone()]).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    ready_excel(&h, &id).await;
    let before = h.row(&id).await;
    let result = h
        .services
        .relogin()
        .credential_channel(CredentialTarget::Excel)
        .unwrap()
        .push(
            std::slice::from_ref(&id),
            &BTreeMap::from([(id.clone(), before.revision)]),
            &context("excel-push"),
        )
        .await
        .unwrap();
    assert!(result[0].success, "{result:?}");
    let stored = h.accounts.accounts.lock().unwrap()[0].clone();
    assert_eq!(stored.credential_revision, original.credential_revision);
    assert_eq!(stored.credential_state, original.credential_state);
    assert_eq!(stored.responses_upstream, original.responses_upstream);
    assert_eq!(stored.enabled, original.enabled);
    assert!(stored.excel_credential.is_some());
    assert!(h.accounts.rotation_attempts.lock().unwrap().is_empty());
    let after = h.row(&id).await;
    assert_eq!(
        after.credential.unwrap().document,
        before.credential.unwrap().document
    );
    assert_eq!(after.synced_at, before.synced_at);
    assert_eq!(after.excel.as_ref().unwrap().status, ReloginStatus::Ready);
    assert!(after.excel.as_ref().unwrap().synced_at.is_some());
    assert_eq!(after.excel.unwrap().target.unwrap().account_id, original.id);
}

#[tokio::test]
async fn push_all_creates_codex_then_binds_excel_to_the_same_new_account() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    ready_excel(&h, &id).await;
    let before = h.row(&id).await;
    let results = h
        .services
        .relogin()
        .push_channels(
            std::slice::from_ref(&id),
            &BTreeMap::from([(id.clone(), before.revision)]),
            Default::default(),
            &BTreeMap::new(),
            CredentialSelection::All,
            &context("push-all-new"),
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result.success), "{results:?}");
    let account_id = {
        let stored = h.accounts.accounts.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert!(stored[0].has_codex_credential());
        assert!(stored[0].excel_credential.is_some());
        stored[0].id.clone()
    };
    {
        let targets = h.accounts.excel_import_targets.lock().unwrap();
        assert_eq!(targets[0].as_ref().unwrap().account_id, account_id);
        assert!(targets[0].as_ref().unwrap().expected_revision.is_none());
    }
    let row = h.row(&id).await;
    assert!(row.synced_at.is_some());
    assert!(row.excel.as_ref().unwrap().synced_at.is_some());
    assert_eq!(
        row.target.unwrap().account_id,
        row.excel.unwrap().target.unwrap().account_id
    );
}

#[tokio::test]
async fn excel_push_prevalidation_failure_keeps_ready_cache_and_never_commits() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("test@example.invalid").await;
    ready_excel(&h, &id).await;
    let before = h.row(&id).await;
    h.provider.fail_next(ProviderAdminErrorKind::Invalid);
    let result = h
        .services
        .relogin()
        .credential_channel(CredentialTarget::Excel)
        .unwrap()
        .push(
            std::slice::from_ref(&id),
            &BTreeMap::from([(id.clone(), before.revision)]),
            &context("excel-prepare-rejected"),
        )
        .await
        .unwrap();
    assert!(!result[0].success);
    assert!(h.accounts.accounts.lock().unwrap().is_empty());
    assert!(h.accounts.excel_import_targets.lock().unwrap().is_empty());
    let after = h.row(&id).await.excel.unwrap();
    assert_eq!(after.status, ReloginStatus::Ready);
    assert_eq!(
        after.credential.unwrap().document,
        before.excel.unwrap().credential.unwrap().document
    );
    assert!(after.synced_at.is_none());
    assert_eq!(
        after.last_attempt.unwrap().push_result,
        ReloginPushResult::Failed
    );
}

#[tokio::test]
async fn push_all_keeps_each_channel_success_when_the_other_fails() {
    for failed in [CredentialTarget::Codex, CredentialTarget::Excel] {
        let h = Harness::new(Vec::new()).await;
        let id = h.import("test@example.invalid").await;
        h.ready(&id).await;
        ready_excel(&h, &id).await;
        if failed == CredentialTarget::Codex {
            h.provider.fail_next(ProviderAdminErrorKind::Invalid);
        } else {
            h.store
                .rows
                .lock()
                .unwrap()
                .get_mut(&id)
                .unwrap()
                .excel
                .as_mut()
                .unwrap()
                .credential
                .as_mut()
                .unwrap()
                .expires_at = Utc::now() - Duration::seconds(1);
        }
        let before = h.row(&id).await;
        let results = h
            .services
            .relogin()
            .push_channels(
                std::slice::from_ref(&id),
                &BTreeMap::from([(id.clone(), before.revision)]),
                Default::default(),
                &BTreeMap::new(),
                CredentialSelection::All,
                &context("push-all-partial"),
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        for result in &results {
            assert_eq!(
                result.success,
                result.credential_target != failed,
                "{results:?}"
            );
        }
        let row = h.row(&id).await;
        assert_eq!(row.synced_at.is_some(), failed != CredentialTarget::Codex);
        assert_eq!(
            row.excel.as_ref().unwrap().synced_at.is_some(),
            failed != CredentialTarget::Excel
        );
        assert!(row.credential.is_some());
        assert!(row.excel.unwrap().credential.is_some());
        let pool = h.accounts.accounts.lock().unwrap();
        assert_eq!(pool.len(), 1);
        assert_eq!(
            pool[0].has_codex_credential(),
            failed != CredentialTarget::Codex
        );
        assert_eq!(
            pool[0].excel_credential.is_some(),
            failed != CredentialTarget::Excel
        );
    }
}
