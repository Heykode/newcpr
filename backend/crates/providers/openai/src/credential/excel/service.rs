use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use chrono::{TimeDelta, Utc};
use gateway_core::account::{
    AccountErrorReason, AccountStateChange, CredentialCasOutcome, CredentialState,
    ExcelCredentialCasUpdate, OutboundProxy, ProviderAccount, ProviderAccountId,
    ProviderAccountIdentity, ProviderAccountStore,
};
use gateway_core::error::StoreErrorKind;

use super::{
    ExcelAuthError, ExcelCredentialCodec, ExcelCredentialData, ExcelIdentity, ExcelTokenClient,
};
use crate::transport::{
    egress::CodexEgressRuntime, profile::CodexWireProfileState,
    tls::build_reqwest_native_client_with_custom_ca,
};

/// Excel owns its own token revision and persistent one-attempt refresh fence.
pub(crate) struct ExcelCredentialService {
    accounts: Arc<dyn ProviderAccountStore>,
    profile: CodexWireProfileState,
    egress: Option<Arc<CodexEgressRuntime>>,
}

impl ExcelCredentialService {
    pub(crate) fn new(
        accounts: Arc<dyn ProviderAccountStore>,
        profile: CodexWireProfileState,
        egress: Option<Arc<CodexEgressRuntime>>,
    ) -> Self {
        Self {
            accounts,
            profile,
            egress,
        }
    }

    pub(crate) fn client(
        &self,
        account: Option<&ProviderAccount>,
        proxy: Option<&OutboundProxy>,
    ) -> Result<reqwest::Client, ExcelAuthError> {
        if let (Some(runtime), Some(account)) = (&self.egress, account)
            && let Some(route) = runtime
                .select(account)
                .map_err(|_| ExcelAuthError::Transport)?
        {
            return runtime
                .http_client(&route, &self.profile.snapshot())
                .map_err(|_| ExcelAuthError::Transport);
        }
        build_reqwest_native_client_with_custom_ca(
            gateway_egress::http_client(proxy)
                .map_err(|_| ExcelAuthError::Transport)?
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15)),
        )
        .map_err(|_| ExcelAuthError::Transport)
    }

    pub(crate) async fn load(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<ExcelCredentialData, ExcelAuthError> {
        let loaded = self
            .accounts
            .load_current_excel_credential(account_id)
            .await
            .map_err(|error| match error.kind() {
                StoreErrorKind::InvalidState => ExcelAuthError::RequiresAuthorization,
                _ => ExcelAuthError::Transport,
            })?;
        let data = ExcelCredentialCodec::decode(&loaded.credential)?;
        data.matches_identity(&ExcelIdentity {
            user_id: loaded
                .account
                .upstream_user_id()
                .ok_or(ExcelAuthError::IdentityMismatch)?
                .into(),
            account_id: loaded
                .account
                .upstream_account_id()
                .ok_or(ExcelAuthError::IdentityMismatch)?
                .into(),
        })?;
        Ok(data)
    }

    pub(crate) async fn refresh(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<(), ExcelAuthError> {
        let loaded = self
            .accounts
            .load_current_excel_credential(account_id)
            .await
            .map_err(|error| match error.kind() {
                StoreErrorKind::InvalidState => ExcelAuthError::RequiresAuthorization,
                _ => ExcelAuthError::Transport,
            })?;
        let old = ExcelCredentialCodec::decode(&loaded.credential)?;
        old.matches_identity(&ExcelIdentity {
            user_id: loaded
                .account
                .upstream_user_id()
                .ok_or(ExcelAuthError::IdentityMismatch)?
                .into(),
            account_id: loaded
                .account
                .upstream_account_id()
                .ok_or(ExcelAuthError::IdentityMismatch)?
                .into(),
        })?;
        if old.expires_at > Utc::now() + TimeDelta::seconds(300) {
            return Ok(());
        }
        if old.refresh_token.is_none() {
            return Err(ExcelAuthError::RequiresAuthorization);
        }
        let client = self.client(Some(&loaded.account), loaded.account.outbound_proxy())?;
        if !self
            .accounts
            .claim_excel_refresh(account_id, loaded.metadata.revision)
            .await
            .map_err(|_| ExcelAuthError::Transport)?
        {
            return Err(ExcelAuthError::RequiresAuthorization);
        }
        let next = match ExcelTokenClient::refresh(&client, &old).await {
            Ok(next) => next,
            Err(error) => {
                // A process crash or an uncertain response must never reuse this rotating RT.
                self.accounts
                    .apply_excel_state_change(AccountStateChange {
                        account_id: account_id.clone(),
                        expected_revision: loaded.metadata.revision,
                        credential_state: CredentialState::Invalid,
                        observed_at: SystemTime::now(),
                        error_reason: Some(AccountErrorReason::CredentialInvalid),
                        message: Some(
                            "Excel refresh was not confirmed; authorize Excel again".into(),
                        ),
                    })
                    .await
                    .map_err(|_| ExcelAuthError::Transport)?;
                return Err(error);
            }
        };
        let material = ExcelCredentialCodec::encode(&next)?;
        let update = ExcelCredentialCasUpdate {
            account_id: account_id.clone(),
            expected_revision: loaded.metadata.revision,
            identity: ProviderAccountIdentity::new(
                next.user_id.clone(),
                Some(next.chatgpt_account_id.clone()),
            ),
            credential: material,
            email: next.email,
            plan_type: next.plan_type,
            has_refresh_token: next.refresh_token.is_some(),
            access_token_expires_at: Some(next.expires_at.into()),
            next_refresh_at: Some((next.expires_at - TimeDelta::seconds(300)).into()),
            observed_at: SystemTime::now(),
        };
        match self
            .accounts
            .compare_and_swap_excel_credential(update)
            .await
            .map_err(|_| ExcelAuthError::Transport)?
        {
            CredentialCasOutcome::Updated(_) => Ok(()),
            CredentialCasOutcome::Conflict => Err(ExcelAuthError::ExchangeUncertain),
        }
    }

    pub(crate) async fn refresh_due(&self) -> Result<(), ExcelAuthError> {
        let due = self
            .accounts
            .list_due_excel_credentials(100, SystemTime::now())
            .await
            .map_err(|_| ExcelAuthError::Transport)?;
        for id in due {
            if let Err(error) = self.refresh(&id).await {
                tracing::warn!(reason = %error, "Excel credential refresh did not complete");
            }
        }
        Ok(())
    }
}
