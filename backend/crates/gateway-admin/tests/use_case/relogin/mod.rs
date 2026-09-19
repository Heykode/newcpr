use super::{
    AdminHarness,
    accounts::{FakeAccountStore, FakeProviderAdmin, account_record, context, events, revision},
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use gateway_admin::{
    AdminServices,
    model::{
        accounts::{AccountRecord, CredentialState},
        relogin::*,
        relogin_templates::{ReloginTemplate, ReloginTemplateConfig, ReloginTemplateSelection},
    },
    ports::{
        provider::ProviderAdminErrorKind,
        relogin::ReloginStore,
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
    },
};
use gateway_core::{
    account::AccountErrorReason,
    lifecycle::CancellationToken,
    task::{
        ScheduledTask, WorkerContribution, WorkerCycleContext, WorkerId, WorkerKind, WorkerRunnable,
    },
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, atomic::Ordering},
};

#[derive(Default)]
struct MemoryStore {
    templates: Mutex<BTreeMap<String, ReloginTemplate>>,
    invalid_template_references: Mutex<bool>,
    rows: Mutex<BTreeMap<String, ReloginEntry>>,
    settings: Mutex<ReloginSettings>,
    fail_after: Mutex<Option<usize>>,
}

fn conflict() -> AdminStoreError {
    AdminStoreError::new(AdminStoreErrorKind::Conflict, "relogin", "revision changed")
}

#[async_trait]
impl ReloginStore for MemoryStore {
    async fn templates(&self) -> AdminStoreResult<Vec<ReloginTemplate>> {
        Ok(self.templates.lock().unwrap().values().cloned().collect())
    }
    async fn save_template(
        &self,
        template: &ReloginTemplate,
        expected: Option<u64>,
    ) -> AdminStoreResult<()> {
        self.validate_template_references(&template.config).await?;
        let mut rows = self.templates.lock().unwrap();
        if rows.get(&template.id).map(|row| row.revision) != expected
            || rows.values().any(|row| {
                row.id != template.id && row.config.name.eq_ignore_ascii_case(&template.config.name)
            })
        {
            return Err(conflict());
        }
        rows.insert(template.id.clone(), template.clone());
        Ok(())
    }
    async fn delete_template(&self, id: &str, expected: u64) -> AdminStoreResult<()> {
        let mut rows = self.templates.lock().unwrap();
        if rows.get(id).map(|row| row.revision) != Some(expected) {
            return Err(conflict());
        }
        rows.remove(id);
        Ok(())
    }
    async fn validate_template_references(
        &self,
        _: &ReloginTemplateConfig,
    ) -> AdminStoreResult<()> {
        if *self.invalid_template_references.lock().unwrap() {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                "template",
                "missing reference",
            ));
        }
        Ok(())
    }
    async fn entries(&self) -> AdminStoreResult<Vec<ReloginEntry>> {
        Ok(self.rows.lock().unwrap().values().cloned().collect())
    }
    async fn save(&self, entry: &ReloginEntry, expected: Option<u64>) -> AdminStoreResult<()> {
        self.save_batch(&[(entry.clone(), expected)]).await
    }
    async fn save_batch(&self, entries: &[(ReloginEntry, Option<u64>)]) -> AdminStoreResult<()> {
        let mut failure = self.fail_after.lock().unwrap();
        if let Some(remaining) = failure.as_mut() {
            if *remaining == 0 {
                *failure = None;
                return Err(conflict());
            }
            *remaining -= 1;
        }
        let mut rows = self.rows.lock().unwrap();
        let mut updated = rows.clone();
        for (entry, expected) in entries {
            if updated.get(&entry.id).map(|row| row.revision) != *expected
                || updated
                    .values()
                    .any(|row| row.id != entry.id && row.email == entry.email)
            {
                return Err(conflict());
            }
            updated.insert(entry.id.clone(), entry.clone());
        }
        *rows = updated;
        Ok(())
    }
    async fn delete(&self, id: &str, expected: u64) -> AdminStoreResult<()> {
        let mut rows = self.rows.lock().unwrap();
        if rows.get(id).map(|row| row.revision) != Some(expected) {
            return Err(conflict());
        }
        rows.remove(id);
        Ok(())
    }
    async fn settings(&self) -> AdminStoreResult<ReloginSettings> {
        Ok(self.settings.lock().unwrap().clone())
    }
    async fn save_settings(&self, settings: &ReloginSettings) -> AdminStoreResult<()> {
        *self.settings.lock().unwrap() = settings.clone();
        Ok(())
    }
}

struct Harness {
    services: AdminServices,
    store: Arc<MemoryStore>,
    accounts: Arc<FakeAccountStore>,
    provider: Arc<FakeProviderAdmin>,
    task: Arc<dyn ScheduledTask>,
}
impl Harness {
    async fn new(pool: Vec<AccountRecord>) -> Self {
        let log = events();
        let accounts = FakeAccountStore::new("openai", log.clone());
        accounts.set_accounts(pool);
        let provider = FakeProviderAdmin::new("openai", log);
        *provider.relogin_result.lock().unwrap() = Some(credential());
        let store = Arc::new(MemoryStore::default());
        let mut bundle = AdminHarness::new()
            .accounts(accounts.clone())
            .provider(provider.clone())
            .relogin(store.clone())
            .build_bundle()
            .await;
        let id = WorkerId::try_new(WorkerKind::OAuthRefresh, "admin_relogin").unwrap();
        let task = bundle
            .take_worker_contributions()
            .into_iter()
            .find_map(|contribution| {
                if let WorkerContribution::Registration(registration) = contribution
                    && registration.id == id
                    && let WorkerRunnable::Scheduled { task, .. } = registration.runnable
                {
                    return Some(Arc::from(task));
                }
                None
            })
            .expect("relogin worker");
        Self {
            services: bundle.services(),
            store,
            accounts,
            provider,
            task,
        }
    }
    async fn import(&self, email: &str) -> String {
        self.services
            .relogin()
            .import(
                &format!("{email}----test-only-password----JBSWY3DPEHPK3PXP"),
                false,
            )
            .await
            .unwrap();
        self.store
            .entries()
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.email == email)
            .unwrap()
            .id
    }
    async fn cycle(&self) {
        self.task.run_cycle(cycle_context()).await.unwrap();
    }
    async fn row(&self, id: &str) -> ReloginEntry {
        self.store
            .entries()
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap()
    }
    async fn ready(&self, id: &str) {
        assert!(self.services.relogin().queue(&[id.into()]).await.unwrap()[0].success);
        self.cycle().await;
        assert_eq!(self.row(id).await.status, ReloginStatus::Ready);
    }
    async fn wait_running(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while self.provider.relogin_active.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn queue_account(
        &self,
        id: &str,
        target: &AccountRecord,
    ) -> Result<(), gateway_admin::model::AdminError> {
        self.services
            .relogin()
            .queue_account(
                id,
                self.row(id).await.revision,
                &ReloginTarget::from_account(target).unwrap(),
                &context("account-menu"),
            )
            .await
    }
}
fn cycle_context() -> WorkerCycleContext {
    WorkerCycleContext::new(
        WorkerId::try_new(WorkerKind::OAuthRefresh, "admin_relogin").unwrap(),
        None,
        CancellationToken::new(),
    )
}
fn account(expired: bool) -> AccountRecord {
    let mut account = account_record("openai");
    account.upstream_account_id = Some("workspace-team".into());
    account.plan_type = Some("team".into());
    if expired {
        account.credential_state = CredentialState::Expired;
        account.last_error_reason = Some(AccountErrorReason::CredentialExpired);
    }
    account
}
fn credential() -> ReloginCredential {
    ReloginCredential {
        document: serde_json::json!({"access_token":"test-only-token"})
            .as_object()
            .unwrap()
            .clone(),
        email: "test@example.invalid".into(),
        user_id: "upstream-user".into(),
        workspace_id: "workspace-team".into(),
        plan_type: "team".into(),
        expires_at: Utc::now() + Duration::hours(1),
        verified_at: Utc::now(),
    }
}

fn template_config() -> ReloginTemplateConfig {
    ReloginTemplateConfig {
        name: "Team defaults".into(),
        enabled: false,
        concurrency_limit: Some(7),
        weight: 13,
        group_ids: vec!["grp_00000000000000000000000000000091".into()],
        outbound_proxy_id: None,
    }
}

fn template_selection(template: &ReloginTemplate) -> ReloginTemplateSelection {
    ReloginTemplateSelection {
        id: template.id.clone(),
        revision: template.revision,
    }
}

#[tokio::test]
async fn relogin_templates_validate_and_fence_edits_deletes_and_pushes() {
    let h = Harness::new(vec![]).await;
    let mut invalid = template_config();
    invalid.concurrency_limit = Some(0);
    assert!(
        h.services
            .relogin()
            .save_template(None, invalid)
            .await
            .is_err()
    );
    let template = h
        .services
        .relogin()
        .save_template(None, template_config())
        .await
        .unwrap();
    assert_eq!(
        h.services.relogin().templates().await.unwrap(),
        vec![template.clone()]
    );
    assert!(
        h.services
            .relogin()
            .save_template(None, template_config())
            .await
            .is_err()
    );
    let mut config = template_config();
    config.weight = 29;
    let updated = h
        .services
        .relogin()
        .save_template(Some(template_selection(&template)), config)
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert!(
        h.services
            .relogin()
            .delete_template(template_selection(&template))
            .await
            .is_err()
    );
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    let before = h.row(&id).await;
    let versions = BTreeMap::from([(id.clone(), before.revision)]);
    assert!(
        h.services
            .relogin()
            .push_with_template(
                std::slice::from_ref(&id),
                &versions,
                Some(template_selection(&template)),
                &context("stale-template")
            )
            .await
            .is_err()
    );
    assert_eq!(h.row(&id).await.revision, before.revision);
    assert!(h.accounts.audit_requests().is_empty());
    h.services
        .relogin()
        .delete_template(template_selection(&updated))
        .await
        .unwrap();
    assert!(
        h.services
            .relogin()
            .push_with_template(
                std::slice::from_ref(&id),
                &versions,
                Some(template_selection(&updated)),
                &context("deleted-template")
            )
            .await
            .is_err()
    );
    assert_eq!(h.row(&id).await.status, ReloginStatus::Ready);
}

#[tokio::test]
async fn relogin_templates_reject_missing_references_before_push_fence() {
    let h = Harness::new(vec![]).await;
    let template = h
        .services
        .relogin()
        .save_template(None, template_config())
        .await
        .unwrap();
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    let before = h.row(&id).await;
    *h.store.invalid_template_references.lock().unwrap() = true;
    let result = h
        .services
        .relogin()
        .push_with_template(
            std::slice::from_ref(&id),
            &BTreeMap::from([(id.clone(), before.revision)]),
            Some(template_selection(&template)),
            &context("invalid-reference"),
        )
        .await
        .unwrap();
    assert!(!result[0].success);
    assert!(result[0].message.contains("模板"));
    assert_eq!(h.row(&id).await.revision, before.revision);
    assert_eq!(h.row(&id).await.status, ReloginStatus::Ready);
    assert!(h.accounts.audit_requests().is_empty());
}

#[tokio::test]
async fn relogin_template_mixed_batch_only_configures_new_accounts() {
    let mut existing = account(false);
    existing.email = Some("existing@example.invalid".into());
    existing.enabled = false;
    let h = Harness::new(vec![existing]).await;
    let template = h
        .services
        .relogin()
        .save_template(None, template_config())
        .await
        .unwrap();
    let old = h.import("existing@example.invalid").await;
    let mut old_credential = credential();
    old_credential.email = "existing@example.invalid".into();
    *h.provider.relogin_result.lock().unwrap() = Some(old_credential);
    h.ready(&old).await;
    *h.provider.relogin_result.lock().unwrap() = Some(credential());
    let new = h.import("test@example.invalid").await;
    h.ready(&new).await;
    let versions = BTreeMap::from([
        (old.clone(), h.row(&old).await.revision),
        (new.clone(), h.row(&new).await.revision),
    ]);
    let result = h
        .services
        .relogin()
        .push_with_template(
            &[old.clone(), new.clone()],
            &versions,
            Some(template_selection(&template)),
            &context("mixed-template"),
        )
        .await
        .unwrap();
    assert!(result.iter().all(|result| result.success), "{result:?}");
    assert_eq!(
        h.accounts.import_settings(),
        vec![Some(template.config.settings().unwrap())]
    );
    assert!(h.row(&old).await.synced_at.is_some());
    assert!(h.row(&new).await.synced_at.is_some());
    assert_eq!(h.row(&old).await.target.unwrap().account_id, "acct_test");
    assert_eq!(
        h.row(&new).await.target.unwrap().account_id,
        "acct_prepared"
    );
    // A repeated confirmation must not create another account or apply settings twice.
    let result = h
        .services
        .relogin()
        .push_with_template(
            &[old, new],
            &versions,
            Some(template_selection(&template)),
            &context("repeat-template"),
        )
        .await
        .unwrap();
    assert!(result.iter().all(|result| !result.success));
    assert_eq!(h.accounts.import_settings().len(), 1);
}

#[tokio::test]
async fn relogin_existing_account_ignores_template_references_and_settings() {
    let h = Harness::new(vec![account(false)]).await;
    let template = h
        .services
        .relogin()
        .save_template(None, template_config())
        .await
        .unwrap();
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    *h.store.invalid_template_references.lock().unwrap() = true;
    let result = h
        .services
        .relogin()
        .push_with_template(
            std::slice::from_ref(&id),
            &BTreeMap::from([(id.clone(), h.row(&id).await.revision)]),
            Some(template_selection(&template)),
            &context("existing-template"),
        )
        .await
        .unwrap();
    assert!(result[0].success);
    assert!(h.accounts.import_settings().is_empty());
}

#[tokio::test]
async fn relogin_import_defaults_redacts_and_requires_explicit_new_account_queue() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    h.cycle().await;
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
    let list = h.services.relogin().list().await.unwrap();
    assert_eq!(list.items[0].relogin_count, Some(0));
    assert!(list.items[0].last_relogin_at.is_none());
    assert!(list.items[0].automatic);
    assert_eq!(list.items[0].status, ReloginStatus::Pending);
    let serialized = serde_json::to_string(&list).unwrap();
    for secret in ["password", "mfa_secret", "JBSWY", "test-only"] {
        assert!(!serialized.contains(secret));
    }
    h.ready(&id).await;
    let serialized = serde_json::to_string(&h.services.relogin().list().await.unwrap()).unwrap();
    assert!(!serialized.contains("test-only-token"));
    assert!(!serialized.contains("test-only-password"));
    assert!(
        h.accounts.audit_requests().is_empty(),
        "manual login must not push"
    );
}

#[tokio::test]
async fn relogin_import_is_validated_before_mutating_existing_rows() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    assert!(
        h.services
            .relogin()
            .import(
                "test@example.invalid----changed----JBSWY3DPEHPK3PXP\ninvalid",
                true
            )
            .await
            .is_err()
    );
    assert_eq!(h.row(&id).await.password, "test-only-password");
    assert!(
        h.services
            .relogin()
            .import("test@example.invalid----changed----JBSWY3DPEHPK3PXP", false)
            .await
            .is_err()
    );
    h.services
        .relogin()
        .automatic(std::slice::from_ref(&id), false)
        .await
        .unwrap();
    h.services
        .relogin()
        .import("test@example.invalid----changed----JBSWY3DPEHPK3PXP", true)
        .await
        .unwrap();
    assert!(!h.row(&id).await.automatic);
    assert_eq!(h.row(&id).await.password, "changed");
}

#[tokio::test]
async fn relogin_legacy_mailbox_rows_cannot_login_or_push_until_totp_is_imported() {
    for status in [
        ReloginStatus::Pending,
        ReloginStatus::Queued,
        ReloginStatus::Running,
        ReloginStatus::Ready,
        ReloginStatus::Pushing,
        ReloginStatus::Uncertain,
    ] {
        let h = Harness::new(vec![account(true)]).await;
        let id = h.import("test@example.invalid").await;
        let mut legacy = serde_json::to_value(h.row(&id).await).unwrap();
        legacy["mfa_secret"] = "".into();
        legacy["mailbox"] = serde_json::json!({
            "client_id": "123e4567-e89b-12d3-a456-426614174000",
            "refresh_token": "synthetic-mailbox-token",
        });
        legacy["status"] = serde_json::to_value(status).unwrap();
        legacy["credential"] = serde_json::to_value(credential()).unwrap();
        let row: ReloginEntry = serde_json::from_value(legacy).unwrap();
        assert!(row.validate_totp().is_err());
        assert!(serde_json::to_value(&row).unwrap().get("mailbox").is_none());
        h.store.rows.lock().unwrap().insert(id.clone(), row);

        let view = h.services.relogin().list().await.unwrap().items.remove(0);
        assert!(!view.automatic);
        assert!(!view.has_totp);
        assert_eq!(view.credential_status, "none");
        if matches!(status, ReloginStatus::Pushing | ReloginStatus::Uncertain) {
            assert_eq!(view.status, status);
        } else {
            assert_eq!(view.status, ReloginStatus::Failed);
            assert!(view.message.contains("2FA"));
        }
        assert!(
            !h.services
                .relogin()
                .queue(std::slice::from_ref(&id))
                .await
                .unwrap()[0]
                .success
        );
        assert!(
            h.services
                .relogin()
                .automatic(std::slice::from_ref(&id), true)
                .await
                .is_err()
        );
        let versions = BTreeMap::from([(id.clone(), h.row(&id).await.revision)]);
        assert!(
            !h.services
                .relogin()
                .push(
                    std::slice::from_ref(&id),
                    &versions,
                    &context("legacy-push"),
                )
                .await
                .unwrap()[0]
                .success
        );
        h.cycle().await;
        h.cycle().await;
        assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
        assert!(h.accounts.audit_requests().is_empty());

        h.services
            .relogin()
            .import(
                "test@example.invalid----test-only-password----JBSWY3DPEHPK3PXP",
                true,
            )
            .await
            .unwrap();
        assert!(h.row(&id).await.validate_totp().is_ok());
        assert!(h.row(&id).await.credential.is_none());
        h.ready(&id).await;
        assert!(h.accounts.audit_requests().is_empty());
    }
}

#[tokio::test]
async fn relogin_new_account_push_uses_normal_import_then_marks_synced() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    let versions = BTreeMap::from([(id.clone(), h.row(&id).await.revision)]);
    assert!(
        h.services
            .relogin()
            .push(std::slice::from_ref(&id), &versions, &context("new-push"))
            .await
            .unwrap()[0]
            .success
    );
    let row = h.row(&id).await;
    assert!(row.synced_at.is_some());
    assert_eq!(row.target.unwrap().account_id, "acct_prepared");
    assert_eq!(h.accounts.import_settings(), vec![None]);
    assert_eq!(h.accounts.audit_requests(), vec!["new-push"]);
    assert_eq!(
        h.services.relogin().list().await.unwrap().items[0].pool_status,
        "synced"
    );
    assert_eq!(
        h.services.relogin().list().await.unwrap().items[0].relogin_count,
        Some(0)
    );
}

#[tokio::test]
async fn relogin_auto_only_recovers_expired_credentials_and_locks_workspace() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    h.cycle().await;
    let row = h.row(&id).await;
    assert!(row.synced_at.is_some());
    assert_eq!(h.accounts.audit_requests().len(), 1);
    assert_eq!(
        *h.provider.relogin_requests.lock().unwrap(),
        vec![Some("workspace-team".into())]
    );
    assert_eq!(row.target.unwrap().account_id, "acct_test");
    let view = h.services.relogin().list().await.unwrap().items.remove(0);
    assert_eq!(view.relogin_count, Some(1));
    assert!(view.last_relogin_at.is_some());
}

#[tokio::test]
async fn relogin_does_not_recover_generic_errors_disabled_or_non_expired_accounts() {
    for variant in 0..3 {
        let mut record = account(true);
        match variant {
            0 => record.enabled = false,
            1 => record.credential_state = CredentialState::Ready,
            2 => record.last_error_reason = None,
            _ => unreachable!("fixed relogin regression variants"),
        }
        let h = Harness::new(vec![record]).await;
        h.import("test@example.invalid").await;
        h.cycle().await;
        assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn relogin_auto_recovers_access_token_expiry_with_refresh_token_when_totp_is_available() {
    let mut record = account(true);
    record.has_refresh_token = true;
    record.last_error_reason = Some(AccountErrorReason::AccessTokenExpired);
    let h = Harness::new(vec![record]).await;
    let id = h.import("test@example.invalid").await;

    h.cycle().await;

    assert!(h.row(&id).await.synced_at.is_some());
    assert_eq!(
        *h.provider.relogin_requests.lock().unwrap(),
        vec![Some("workspace-team".into())]
    );
    assert_eq!(h.accounts.audit_requests().len(), 1);
}

#[tokio::test]
async fn relogin_access_token_expiry_still_requires_valid_material_and_automatic_enabled() {
    for variant in 0..4 {
        let mut record = account(true);
        record.has_refresh_token = true;
        record.last_error_reason = Some(AccountErrorReason::AccessTokenExpired);
        let h = Harness::new(vec![record]).await;
        if variant != 0 {
            let id = h.import("test@example.invalid").await;
            match variant {
                1 => h
                    .store
                    .rows
                    .lock()
                    .unwrap()
                    .get_mut(&id)
                    .unwrap()
                    .mfa_secret
                    .clear(),
                2 => {
                    h.services
                        .relogin()
                        .automatic(std::slice::from_ref(&id), false)
                        .await
                        .unwrap();
                }
                3 => {
                    h.services
                        .relogin()
                        .configure(ReloginSettings {
                            concurrency: 1,
                            paused: true,
                        })
                        .await
                        .unwrap();
                }
                _ => unreachable!("fixed relogin regression variants"),
            }
        }

        h.cycle().await;

        assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
        assert!(h.accounts.audit_requests().is_empty());
    }
}

#[tokio::test]
async fn relogin_access_token_expiry_does_not_overwrite_recovered_or_rotated_credentials() {
    for recovered in [true, false] {
        let mut record = account(true);
        record.has_refresh_token = true;
        record.last_error_reason = Some(AccountErrorReason::AccessTokenExpired);
        let h = Harness::new(vec![record.clone()]).await;
        let id = h.import("test@example.invalid").await;
        *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_millis(100);
        let task = h.task.clone();
        let running = tokio::spawn(async move { task.run_cycle(cycle_context()).await });
        h.wait_running().await;
        if recovered {
            record.credential_state = CredentialState::Ready;
            record.last_error_reason = None;
        } else {
            record.credential_revision = revision(record.credential_revision.get() + 1);
        }
        h.accounts.set_accounts(vec![record]);
        running.await.unwrap().unwrap();

        assert!(h.accounts.audit_requests().is_empty());
        assert!(h.row(&id).await.synced_at.is_none());
    }
}

#[tokio::test]
async fn relogin_list_separates_cached_verification_from_current_pool_state() {
    let h = Harness::new(vec![account(false)]).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    {
        let mut rows = h.store.rows.lock().unwrap();
        rows.get_mut(&id).unwrap().status = ReloginStatus::Uncertain;
    }
    let mut expired = account(true);
    expired.enabled = false;
    expired.last_error_message = Some("upstream rejected authentication".into());
    h.accounts.set_accounts(vec![expired]);
    let view = h.services.relogin().list().await.unwrap().items.remove(0);
    assert!(view.has_totp);
    let serialized = serde_json::to_value(&view).unwrap();
    assert_eq!(serialized["hasTotp"], true);
    assert!(serialized.get("password").is_none());
    assert!(serialized.get("mfaSecret").is_none());
    assert_eq!(view.status, ReloginStatus::Uncertain);
    assert_eq!(view.credential_status, "verified");
    assert_eq!(view.pool_accounts.len(), 1);
    let pool = &view.pool_accounts[0];
    assert_eq!(pool.status, "error");
    assert_eq!(pool.error_reason, Some("credential_expired"));
    assert!(!pool.enabled);
    assert_eq!(pool.workspace_id.as_deref(), Some("workspace-team"));
    h.accounts.set_accounts(vec![account(false)]);
    let view = h.services.relogin().list().await.unwrap().items.remove(0);
    assert_eq!(view.pool_accounts[0].status, "normal");
    assert!(view.pool_accounts[0].enabled);
    assert_eq!(view.status, ReloginStatus::Uncertain);
    assert!(h.accounts.audit_requests().is_empty());
}

#[tokio::test]
async fn relogin_import_order_does_not_follow_background_updates() {
    let h = Harness::new(vec![]).await;
    let old = h.import("old@example.invalid").await;
    let new = h.import("new@example.invalid").await;
    let now = Utc::now();
    {
        let mut rows = h.store.rows.lock().unwrap();
        rows.get_mut(&old).unwrap().imported_at = Some(now - Duration::hours(1));
        rows.get_mut(&old).unwrap().updated_at = now + Duration::hours(1);
        rows.get_mut(&new).unwrap().imported_at = Some(now);
    }
    let before = h.services.relogin().list().await.unwrap();
    assert_eq!(before.items[0].id, new);
    h.services
        .relogin()
        .automatic(std::slice::from_ref(&old), false)
        .await
        .unwrap();
    let after = h.services.relogin().list().await.unwrap();
    assert_eq!(after.items[0].id, new);
    assert_eq!(after.items[1].imported_at, before.items[1].imported_at);
    h.services
        .relogin()
        .import(
            "old@example.invalid----test-only-password----JBSWY3DPEHPK3PXP",
            true,
        )
        .await
        .unwrap();
    assert_eq!(h.services.relogin().list().await.unwrap().items[0].id, old);

    let row = h.row(&old).await;
    let mut legacy = serde_json::to_value(&row).unwrap();
    legacy.as_object_mut().unwrap().remove("imported_at");
    let mut legacy: ReloginEntry = serde_json::from_value(legacy).unwrap();
    let created = legacy.import_time().expect("legacy UUIDv7 creation time");
    legacy.updated_at += Duration::days(1);
    assert_eq!(legacy.import_time(), Some(created));
    legacy.id = "relogin_non_timestamp_id".into();
    assert!(legacy.import_time().is_none());
}

#[tokio::test]
async fn relogin_workspace_rejects_unknown_choices_without_clearing_credentials() {
    let h = Harness::new(vec![account(false)]).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    let before = h.row(&id).await;
    assert!(
        h.services
            .relogin()
            .workspace(&id, Some("unknown-workspace".into()))
            .await
            .is_err()
    );
    let after = h.row(&id).await;
    assert_eq!(after.revision, before.revision);
    assert!(after.credential.is_some());
    h.services
        .relogin()
        .workspace(&id, Some("workspace-team".into()))
        .await
        .unwrap();
    let after = h.row(&id).await;
    assert_eq!(after.status, ReloginStatus::Pending);
    assert!(after.credential.is_none());
    assert_eq!(
        after.preferred_workspace_id.as_deref(),
        Some("workspace-team")
    );
}

#[tokio::test]
async fn relogin_multi_workspace_requires_selection_and_never_falls_back() {
    let mut second = account(false);
    second.id = "acct_other".into();
    second.upstream_account_id = Some("workspace-other".into());
    let h = Harness::new(vec![account(false), second]).await;
    let id = h.import("test@example.invalid").await;
    assert!(
        !h.services
            .relogin()
            .queue(std::slice::from_ref(&id))
            .await
            .unwrap()[0]
            .success
    );
    h.services
        .relogin()
        .workspace(&id, Some("workspace-team".into()))
        .await
        .unwrap();
    h.ready(&id).await;
    assert_eq!(h.row(&id).await.target.unwrap().account_id, "acct_test");
}

#[tokio::test]
async fn relogin_push_fences_confirmation_and_pool_revision() {
    let h = Harness::new(vec![account(false)]).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    let row = h.row(&id).await;
    let ids = [id.clone()];
    let stale = BTreeMap::from([(id.clone(), row.revision - 1)]);
    assert!(
        !h.services
            .relogin()
            .push(&ids, &stale, &context("stale"))
            .await
            .unwrap()[0]
            .success
    );
    let mut newer = account(false);
    newer.credential_revision = revision(2);
    h.accounts.set_accounts(vec![newer]);
    let current = BTreeMap::from([(id, row.revision)]);
    assert!(
        !h.services
            .relogin()
            .push(&ids, &current, &context("pool-newer"))
            .await
            .unwrap()[0]
            .success
    );
    assert!(h.accounts.audit_requests().is_empty());
}

#[tokio::test]
async fn relogin_push_refuses_target_that_appears_after_login() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    h.accounts.set_accounts(vec![account(false)]);
    let row = h.row(&id).await;
    let revisions = BTreeMap::from([(id.clone(), row.revision)]);
    assert!(
        !h.services
            .relogin()
            .push(&[id], &revisions, &context("appeared"))
            .await
            .unwrap()[0]
            .success
    );
    assert!(h.accounts.audit_requests().is_empty());
}

#[tokio::test]
async fn relogin_rotation_preparation_failure_keeps_cached_credentials_retryable() {
    for kind in [
        ProviderAdminErrorKind::Invalid,
        ProviderAdminErrorKind::Conflict,
        ProviderAdminErrorKind::Unavailable,
    ] {
        let h = Harness::new(vec![account(false)]).await;
        let id = h.import("test@example.invalid").await;
        h.ready(&id).await;
        let before = h.row(&id).await;
        let revisions = BTreeMap::from([(id.clone(), before.revision)]);
        h.provider.fail_next(kind);
        assert!(
            !h.services
                .relogin()
                .push(
                    std::slice::from_ref(&id),
                    &revisions,
                    &context("prepare-fail")
                )
                .await
                .unwrap()[0]
                .success
        );
        let after = h.row(&id).await;
        assert_eq!(after.status, ReloginStatus::Ready);
        assert_eq!(after.revision, before.revision);
        assert!(after.synced_at.is_none());
        assert_eq!(
            after.credential.unwrap().document,
            before.credential.unwrap().document
        );
        assert!(h.accounts.audit_requests().is_empty());
        assert!(
            h.services
                .relogin()
                .push(
                    std::slice::from_ref(&id),
                    &revisions,
                    &context("prepare-retry")
                )
                .await
                .unwrap()[0]
                .success
        );
        assert_eq!(h.accounts.audit_requests(), vec!["prepare-retry"]);
        assert!(h.row(&id).await.synced_at.is_some());
    }
}

#[tokio::test]
async fn relogin_unknown_push_is_not_replayed_and_successful_push_is_idempotent() {
    let h = Harness::new(vec![account(false)]).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    let row = h.row(&id).await;
    h.accounts.fail_next_commit();
    let revisions = BTreeMap::from([(id.clone(), row.revision)]);
    assert!(
        !h.services
            .relogin()
            .push(std::slice::from_ref(&id), &revisions, &context("fail"))
            .await
            .unwrap()[0]
            .success
    );
    assert_eq!(h.row(&id).await.status, ReloginStatus::Uncertain);
    let attempts = h.provider.relogin_requests.lock().unwrap().len();
    h.accounts.set_accounts(vec![account(true)]);
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), attempts);
    h.ready(&id).await;
    let revisions = BTreeMap::from([(id.clone(), h.row(&id).await.revision)]);
    assert!(
        h.services
            .relogin()
            .push(std::slice::from_ref(&id), &revisions, &context("success"))
            .await
            .unwrap()[0]
            .success
    );
    let commits = h.accounts.audit_requests().len();
    let revisions = BTreeMap::from([(id.clone(), h.row(&id).await.revision)]);
    assert!(
        h.services
            .relogin()
            .push(&[id], &revisions, &context("repeat"))
            .await
            .unwrap()[0]
            .success
    );
    assert_eq!(h.accounts.audit_requests().len(), commits);
}

#[tokio::test]
async fn relogin_cancel_delete_pause_and_auto_off_fence_inflight_results() {
    for operation in ["delete", "pause", "automatic", "workspace"] {
        let h = Harness::new(vec![account(true)]).await;
        let id = h.import("test@example.invalid").await;
        *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_secs(30);
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
                    concurrency: 1,
                    paused: true,
                })
                .await
                .unwrap(),
            "automatic" => h
                .services
                .relogin()
                .automatic(std::slice::from_ref(&id), false)
                .await
                .unwrap(),
            _ => h.services.relogin().workspace(&id, None).await.unwrap(),
        }
        tokio::time::timeout(std::time::Duration::from_secs(3), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(h.accounts.audit_requests().is_empty());
        if operation != "delete" {
            assert!(h.row(&id).await.credential.is_none());
        } else {
            assert!(h.store.entries().await.unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn relogin_queue_is_deduplicated_and_shares_bounded_concurrency() {
    let h = Harness::new(vec![account(true)]).await;
    let first = h.import("test@example.invalid").await;
    let second = h.import("two@example.invalid").await;
    let third = h.import("three@example.invalid").await;
    h.services
        .relogin()
        .queue(&[second.clone(), third])
        .await
        .unwrap();
    assert!(!h.services.relogin().queue(&[second]).await.unwrap()[0].success);
    *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_millis(20);
    h.services
        .relogin()
        .configure(ReloginSettings {
            concurrency: 2,
            paused: false,
        })
        .await
        .unwrap();
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 2);
    assert_eq!(h.provider.relogin_peak.load(Ordering::SeqCst), 2);
    h.cycle().await;
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 3);
    assert!(h.row(&first).await.synced_at.is_some());
}

#[tokio::test]
async fn relogin_failed_automatic_attempts_stop_after_three_and_orphan_push_is_uncertain() {
    let h = Harness::new(vec![account(true)]).await;
    *h.provider.relogin_result.lock().unwrap() = None;
    let id = h.import("test@example.invalid").await;
    for _ in 0..5 {
        h.store
            .rows
            .lock()
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .next_attempt_at = None;
        h.cycle().await;
    }
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 3);
    h.store.rows.lock().unwrap().get_mut(&id).unwrap().status = ReloginStatus::Pushing;
    h.cycle().await;
    assert_eq!(h.row(&id).await.status, ReloginStatus::Uncertain);
}

#[tokio::test]
async fn relogin_partial_claim_failure_cannot_leave_phantom_active_jobs() {
    let h = Harness::new(vec![]).await;
    let first = h.import("one@example.invalid").await;
    let second = h.import("two@example.invalid").await;
    h.services
        .relogin()
        .queue(&[first.clone(), second.clone()])
        .await
        .unwrap();
    h.services
        .relogin()
        .configure(ReloginSettings {
            concurrency: 2,
            paused: false,
        })
        .await
        .unwrap();
    *h.store.fail_after.lock().unwrap() = Some(1);
    assert!(h.task.run_cycle(cycle_context()).await.is_err());
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
    h.cycle().await;
    assert_eq!(h.row(&second).await.status, ReloginStatus::Ready);
    assert_eq!(h.row(&first).await.status, ReloginStatus::Failed);
    h.ready(&first).await;
}

#[tokio::test]
async fn relogin_auto_rechecks_enabled_and_expired_state_before_push() {
    let h = Harness::new(vec![account(true)]).await;
    let id = h.import("test@example.invalid").await;
    *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_millis(100);
    let task = h.task.clone();
    let running = tokio::spawn(async move { task.run_cycle(cycle_context()).await });
    h.wait_running().await;
    let mut disabled = account(true);
    disabled.enabled = false;
    h.accounts.set_accounts(vec![disabled]);
    running.await.unwrap().unwrap();
    assert!(h.accounts.audit_requests().is_empty());
    assert!(h.row(&id).await.synced_at.is_none());
}

#[tokio::test]
async fn relogin_counts_only_successful_existing_pushes_and_survives_material_reimport() {
    let h = Harness::new(vec![account(false)]).await;
    let id = h.import("test@example.invalid").await;
    assert_eq!(
        h.services.relogin().list().await.unwrap().items[0].relogin_count,
        Some(0)
    );
    for expected in 1..=2 {
        h.provider
            .set_current_credential_revision(revision(expected));
        h.ready(&id).await;
        assert_eq!(
            h.services.relogin().list().await.unwrap().items[0].relogin_count,
            Some(expected - 1)
        );
        for _ in 0..2 {
            let revisions = BTreeMap::from([(id.clone(), h.row(&id).await.revision)]);
            assert!(
                h.services
                    .relogin()
                    .push(std::slice::from_ref(&id), &revisions, &context("count"))
                    .await
                    .unwrap()[0]
                    .success
            );
            let view = h.services.relogin().list().await.unwrap().items.remove(0);
            assert_eq!(view.relogin_count, Some(expected));
            assert_eq!(view.relogin_account_id.as_deref(), Some("acct_test"));
            assert!(view.last_relogin_at.is_some());
        }
    }
    h.services
        .relogin()
        .import("test@example.invalid----changed----JBSWY3DPEHPK3PXP", true)
        .await
        .unwrap();
    assert_eq!(
        h.services.relogin().list().await.unwrap().items[0].relogin_count,
        Some(2)
    );
    h.services.relogin().delete(&[id]).await.unwrap();
    h.import("test@example.invalid").await;
    assert_eq!(
        h.services.relogin().list().await.unwrap().items[0].relogin_count,
        Some(2)
    );
}

#[tokio::test]
async fn relogin_committed_count_survives_failed_library_settlement_and_is_not_replayed() {
    let h = Harness::new(vec![account(false)]).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    // Allow the Pushing fence, then fail the library save after the pool commit.
    *h.store.fail_after.lock().unwrap() = Some(1);
    let revisions = BTreeMap::from([(id.clone(), h.row(&id).await.revision)]);
    assert!(
        !h.services
            .relogin()
            .push(
                std::slice::from_ref(&id),
                &revisions,
                &context("settlement")
            )
            .await
            .unwrap()[0]
            .success
    );
    assert_eq!(h.row(&id).await.status, ReloginStatus::Pushing);
    h.cycle().await;
    let view = h.services.relogin().list().await.unwrap().items.remove(0);
    assert_eq!(view.status, ReloginStatus::Uncertain);
    assert_eq!(view.relogin_count, Some(1));
    let revisions = BTreeMap::from([(id.clone(), h.row(&id).await.revision)]);
    assert!(
        !h.services
            .relogin()
            .push(&[id], &revisions, &context("repeat"))
            .await
            .unwrap()[0]
            .success
    );
    assert_eq!(
        h.services.relogin().list().await.unwrap().items[0].relogin_count,
        Some(1)
    );
}

#[tokio::test]
async fn relogin_statistics_resolve_identity_and_workspace_without_combining_accounts() {
    let mut first = account(false);
    first.relogin_count = 3;
    first.last_relogin_at = Some(Utc::now());
    let mut second = first.clone();
    second.id = "acct_other".into();
    second.upstream_account_id = Some("workspace-other".into());
    second.relogin_count = 7;
    let h = Harness::new(vec![first.clone(), second]).await;
    let id = h.import("test@example.invalid").await;
    let view = h.services.relogin().list().await.unwrap().items.remove(0);
    assert_eq!(view.relogin_count, None);
    assert_eq!(view.relogin_account_id, None);
    h.services
        .relogin()
        .workspace(&id, Some("workspace-other".into()))
        .await
        .unwrap();
    let view = h.services.relogin().list().await.unwrap().items.remove(0);
    assert_eq!(view.relogin_count, Some(7));
    assert_eq!(view.relogin_account_id.as_deref(), Some("acct_other"));
    h.services
        .relogin()
        .workspace(&id, Some("workspace-team".into()))
        .await
        .unwrap();
    h.ready(&id).await;
    first.upstream_user_id = Some("different-user".into());
    h.accounts.set_accounts(vec![first]);
    assert_eq!(
        h.services.relogin().list().await.unwrap().items[0].relogin_count,
        None
    );
}

#[tokio::test]
async fn account_relogin_locks_selected_workspace_preserves_preferences_and_counts_once() {
    let first = account(false);
    let mut second = first.clone();
    second.id = "acct_other".into();
    second.upstream_account_id = Some("workspace-other".into());
    let h = Harness::new(vec![first.clone(), second]).await;
    let id = h.import("test@example.invalid").await;
    h.services
        .relogin()
        .automatic(std::slice::from_ref(&id), false)
        .await
        .unwrap();
    h.services
        .relogin()
        .workspace(&id, Some("workspace-other".into()))
        .await
        .unwrap();
    h.queue_account(&id, &first).await.unwrap();
    assert!(h.queue_account(&id, &first).await.is_err());
    assert!(
        !h.services
            .relogin()
            .queue(std::slice::from_ref(&id))
            .await
            .unwrap()[0]
            .success
    );
    let actions = h
        .services
        .relogin()
        .account_actions(&["acct_test".into(), "acct_other".into()])
        .await
        .unwrap();
    assert!(
        actions
            .iter()
            .all(|action| action.busy && action.blocked_reason.is_some())
    );
    h.cycle().await;
    h.cycle().await;
    let row = h.row(&id).await;
    assert!(row.synced_at.is_some());
    assert!(!row.automatic);
    assert_eq!(
        row.preferred_workspace_id.as_deref(),
        Some("workspace-other")
    );
    assert_eq!(row.target.unwrap().account_id, first.id);
    assert_eq!(h.accounts.audit_requests(), vec!["account-menu"]);
    assert_eq!(
        *h.provider.relogin_requests.lock().unwrap(),
        vec![Some("workspace-team".into())]
    );
    let view = h.services.relogin().list().await.unwrap().items.remove(0);
    assert_eq!(view.relogin_count, Some(1));
}

#[tokio::test]
async fn account_relogin_queries_hide_missing_invalid_and_non_oauth_material_without_secrets() {
    let first = account(false);
    let mut other = first.clone();
    other.id = "acct_other".into();
    other.email = Some("absent@example.invalid".into());
    let mut key = first.clone();
    key.id = "acct_key".into();
    key.authentication_kind = "api_key".into();
    let h = Harness::new(vec![first, other, key]).await;
    let ids = ["acct_test".into(), "acct_other".into(), "acct_key".into()];
    assert!(
        h.services
            .relogin()
            .account_actions(&ids)
            .await
            .unwrap()
            .is_empty()
    );
    let id = h.import("test@example.invalid").await;
    let actions = h.services.relogin().account_actions(&ids).await.unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].account_id, "acct_test");
    let wire = serde_json::to_string(&actions).unwrap();
    for secret in ["password", "mfa_secret", "test-only", "JBSWY"] {
        assert!(!wire.contains(secret));
    }
    h.store
        .rows
        .lock()
        .unwrap()
        .get_mut(&id)
        .unwrap()
        .mfa_secret
        .clear();
    assert!(
        h.services
            .relogin()
            .account_actions(&ids)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(h.services.relogin().account_actions(&[]).await.is_err());
}

#[tokio::test]
async fn account_relogin_rejects_stale_material_target_wrong_email_and_pause_before_login() {
    for variant in [
        "material",
        "revision",
        "identity",
        "workspace",
        "email",
        "deleted",
        "paused",
    ] {
        let original = account(false);
        let h = Harness::new(vec![original.clone()]).await;
        let id = h.import("test@example.invalid").await;
        let version = h.row(&id).await.revision;
        let mut changed = original.clone();
        match variant {
            "material" => h.services.relogin().workspace(&id, None).await.unwrap(),
            "revision" => changed.credential_revision = revision(2),
            "identity" => changed.upstream_user_id = Some("changed-user".into()),
            "workspace" => changed.upstream_account_id = Some("changed-workspace".into()),
            "email" => changed.email = Some("changed@example.invalid".into()),
            "paused" => h
                .services
                .relogin()
                .configure(ReloginSettings {
                    concurrency: 1,
                    paused: true,
                })
                .await
                .unwrap(),
            _ => {}
        }
        h.accounts.set_accounts(if variant == "deleted" {
            vec![]
        } else {
            vec![changed]
        });
        assert!(
            h.services
                .relogin()
                .queue_account(
                    &id,
                    version,
                    &ReloginTarget::from_account(&original).unwrap(),
                    &context("stale")
                )
                .await
                .is_err(),
            "{variant}"
        );
        assert!(h.accounts.audit_requests().is_empty());
        assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn account_relogin_failed_verification_or_changed_pool_never_overwrites() {
    for variant in [
        "login",
        "workspace",
        "identity",
        "email",
        "expired",
        "revision",
        "deleted",
    ] {
        let original = account(false);
        let h = Harness::new(vec![original.clone()]).await;
        let id = h.import("test@example.invalid").await;
        let mut result = credential();
        match variant {
            "workspace" => result.workspace_id = "wrong".into(),
            "identity" => result.user_id = "wrong".into(),
            "expired" => result.expires_at = Utc::now() - Duration::hours(1),
            _ => {}
        }
        *h.provider.relogin_result.lock().unwrap() = (variant != "login").then_some(result);
        h.queue_account(&id, &original).await.unwrap();
        *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_millis(30);
        let task = h.task.clone();
        let running = tokio::spawn(async move { task.run_cycle(cycle_context()).await });
        h.wait_running().await;
        if variant == "revision" || variant == "email" {
            let mut newer = original;
            if variant == "revision" {
                newer.credential_revision = revision(2);
            } else {
                newer.email = Some("changed@example.invalid".into());
            }
            h.accounts.set_accounts(vec![newer]);
        } else if variant == "deleted" {
            h.accounts.set_accounts(vec![]);
        }
        running.await.unwrap().unwrap();
        assert!(h.accounts.audit_requests().is_empty(), "{variant}");
        assert!(h.row(&id).await.synced_at.is_none());
        assert_eq!(h.row(&id).await.status, ReloginStatus::Failed, "{variant}");
    }
}

#[tokio::test]
async fn account_relogin_cancel_and_library_queue_do_not_reuse_manual_push_intent() {
    let original = account(false);
    for operation in ["delete", "pause", "automatic", "workspace", "import"] {
        let h = Harness::new(vec![original.clone()]).await;
        let id = h.import("test@example.invalid").await;
        h.queue_account(&id, &original).await.unwrap();
        *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_secs(30);
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
                    concurrency: 1,
                    paused: true,
                })
                .await
                .unwrap(),
            "automatic" => h
                .services
                .relogin()
                .automatic(std::slice::from_ref(&id), false)
                .await
                .unwrap(),
            "import" => {
                h.services
                    .relogin()
                    .import("test@example.invalid----changed----JBSWY3DPEHPK3PXP", true)
                    .await
                    .unwrap();
            }
            _ => h.services.relogin().workspace(&id, None).await.unwrap(),
        }
        tokio::time::timeout(std::time::Duration::from_secs(3), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(h.accounts.audit_requests().is_empty());
        if operation != "delete" {
            assert!(h.row(&id).await.manual_push_context.is_none());
        }
    }
    let h = Harness::new(vec![original.clone()]).await;
    let id = h.import("test@example.invalid").await;
    h.queue_account(&id, &original).await.unwrap();
    h.cycle().await;
    h.ready(&id).await;
    assert!(h.row(&id).await.manual_push_context.is_none());
    assert_eq!(h.accounts.audit_requests().len(), 1);
}

#[tokio::test]
async fn account_relogin_legacy_rows_default_to_no_push_and_context_roundtrips() {
    let h = Harness::new(vec![account(false)]).await;
    let id = h.import("test@example.invalid").await;
    let mut legacy = serde_json::to_value(h.row(&id).await).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("manual_push_context");
    let row: ReloginEntry = serde_json::from_value(legacy).unwrap();
    assert!(row.manual_push_context.is_none());
    h.queue_account(&id, &account(false)).await.unwrap();
    let row: ReloginEntry =
        serde_json::from_value(serde_json::to_value(h.row(&id).await).unwrap()).unwrap();
    assert_eq!(row.manual_push_context, Some(context("account-menu")));
}

#[tokio::test]
async fn account_relogin_committed_rotation_survives_lost_settlement_without_replay() {
    let original = account(false);
    let h = Harness::new(vec![original.clone()]).await;
    let id = h.import("test@example.invalid").await;
    h.queue_account(&id, &original).await.unwrap();
    // Running, Ready and Pushing persist; the post-commit library settlement fails.
    *h.store.fail_after.lock().unwrap() = Some(3);
    assert!(h.task.run_cycle(cycle_context()).await.is_err());
    assert_eq!(h.row(&id).await.status, ReloginStatus::Pushing);
    assert_eq!(h.accounts.audit_requests(), vec!["account-menu"]);
    h.cycle().await;
    let view = h.services.relogin().list().await.unwrap().items.remove(0);
    assert_eq!(view.status, ReloginStatus::Uncertain);
    assert_eq!(view.relogin_count, Some(1));
    assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 1);
}
