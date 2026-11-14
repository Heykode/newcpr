use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use gateway_core::account::{ExcelCredentialMetadata, ResponsesUpstream};
use provider_openai::credential::excel::{EXCEL_CLIENT_ID, ExcelCredentialCodec};

impl MemoryAccountStore {
    pub(crate) fn set_workspace(&self, id: &str, workspace: &str) {
        let mut accounts = self.accounts.lock().unwrap();
        let stored = accounts
            .get_mut(&ProviderAccountId::new(id).unwrap())
            .unwrap();
        stored.account = stored.account.clone().with_profile(
            stored.account.email().map(str::to_owned),
            Some(workspace.into()),
            stored.account.plan_type().map(str::to_owned),
        );
    }

    pub(crate) fn seed_excel_credential(&self, id: &str, refreshable: bool) -> ProviderAccount {
        let mut accounts = self.accounts.lock().unwrap();
        let stored = accounts
            .get_mut(&ProviderAccountId::new(id).unwrap())
            .unwrap();
        let base = &stored.account;
        let revision = base
            .excel_credential()
            .map_or(CredentialRevision::new(1).unwrap(), |old| {
                old.revision.next().unwrap()
            });
        let expires = chrono::Utc::now() + chrono::Duration::hours(1);
        let user = base.upstream_user_id().unwrap();
        let workspace = base.upstream_account_id().unwrap_or("fixture-workspace");
        let claims = serde_json::json!({
            "client_id": EXCEL_CLIENT_ID,
            "exp": expires.timestamp(),
            "https://api.openai.com/auth": {
                "chatgpt_account_id": workspace, "chatgpt_user_id": user,
                "chatgpt_plan_type": "plus",
            },
        });
        let token = format!(
            "e30.{}.fixture",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        );
        let document = serde_json::json!({
            "schema_version": 1, "type": "cpa-bps", "access_token": token,
            "refresh_token": refreshable.then_some("fixture-excel-refresh"),
        });
        let data = ExcelCredentialCodec::import_document(document.as_object().unwrap()).unwrap();
        let material = ExcelCredentialCodec::encode(&data).unwrap();
        let metadata = ExcelCredentialMetadata {
            revision,
            state: CredentialState::Ready,
            upstream_user_id: user.into(),
            upstream_account_id: Some(workspace.into()),
            email: base.email().map(str::to_owned),
            plan_type: Some("plus".into()),
            access_token_expires_at: Some(expires.into()),
            has_refresh_token: refreshable,
            next_refresh_at: None,
            refresh_attempted: false,
            observed_at: SystemTime::now(),
            last_error_reason: None,
            last_error_message: None,
            suspended: false,
            quota: QuotaState::unknown(),
        };
        stored.account = stored.account.clone().with_excel_credential(Some(metadata));
        self.excel_credentials
            .lock()
            .unwrap()
            .insert(stored.account.id().clone(), material);
        stored
            .account
            .clone()
            .for_credential_route(ResponsesUpstream::Excel)
            .unwrap()
    }

    pub(crate) fn excel_account(&self, id: &str) -> ProviderAccount {
        self.account(id)
            .unwrap()
            .for_credential_route(ResponsesUpstream::Excel)
            .unwrap()
    }

    pub(super) async fn write_excel_state(
        &self,
        change: AccountStateChange,
        diagnostic: bool,
    ) -> Result<(), StoreError> {
        self.state_write_started.notify_one();
        if self.pause_state_write.load(Ordering::SeqCst) {
            self.state_write_gate.notified().await;
        }
        if self.fail_state_write.load(Ordering::SeqCst) {
            return Err(store_error(StoreErrorKind::Unavailable));
        }
        let mut accounts = self.accounts.lock().unwrap();
        let stored = accounts
            .get_mut(&change.account_id)
            .ok_or_else(|| store_error(StoreErrorKind::InvalidData))?;
        let mut excel = stored
            .account
            .excel_credential()
            .cloned()
            .ok_or_else(|| store_error(StoreErrorKind::InvalidData))?;
        if excel.revision != change.expected_revision
            || excel.observed_at > change.observed_at
            || (!diagnostic && !stored.account.enabled())
        {
            return Err(store_error(StoreErrorKind::Conflict));
        }
        excel.state = change.credential_state;
        excel.observed_at = change.observed_at;
        excel.last_error_reason = change.error_reason;
        excel.last_error_message = change.message;
        stored.account = stored.account.clone().with_excel_credential(Some(excel));
        Ok(())
    }
}
