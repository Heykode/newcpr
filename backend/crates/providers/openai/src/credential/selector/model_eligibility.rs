//! Native Responses catalog qualification and credential-scoped model refusal.

use super::*;
use gateway_core::account::ResponsesUpstream;
use gateway_core::provider_ports::{ProviderCooldownScope, ProviderScopedCooldown};
use gateway_core::routing::UpstreamModelId;

const MODEL_UNSUPPORTED_COOLDOWN: Duration = Duration::from_secs(30 * 60);

impl CodexCredentialSelector {
    fn native_model<'a>(
        account: &ProviderAccount,
        request: &CredentialSelectionInput<'a>,
    ) -> Option<&'a str> {
        (request.responses_model
            && !request.attempt.is_diagnostic_required_account()
            && Self::request_upstream(account, request) == ResponsesUpstream::Codex)
            .then_some(request.upstream_model)
            .flatten()
    }

    pub(super) fn retain_model_eligible(
        &self,
        accounts: &mut Vec<ProviderAccount>,
        request: &CredentialSelectionInput<'_>,
    ) {
        let Some(model) = request.upstream_model.filter(|_| {
            request.responses_model && !request.attempt.is_diagnostic_required_account()
        }) else {
            return;
        };
        let excluded = self
            .catalog
            .scheduling_exclusions(accounts, model, SystemTime::now());
        let before = accounts.len();
        accounts.retain(|account| {
            Self::native_model(account, request).is_none() || !excluded.contains(account.id())
        });
        let rejected = before - accounts.len();
        if rejected > 0 && request.attempt.trace().is_enabled() {
            request.attempt.trace().record(
                "account.model_catalog",
                serde_json::json!({"rejectedCount": rejected}),
            );
        }
    }

    pub(super) async fn scheduling_cooldown(
        &self,
        account: &ProviderAccount,
        request: &CredentialSelectionInput<'_>,
    ) -> Result<Option<SystemTime>, super::super::quota::CodexCredentialQuotaError> {
        let model_cooldown = async {
            let model = Self::native_model(account, request)?;
            let scope = ProviderCooldownScope::upstream_model(UpstreamModelId::new(model).ok()?);
            self.cooldowns
                .read_scoped(account.id(), &scope)
                .await
                .ok()
                .flatten()
                .filter(|cooldown| {
                    cooldown.credential_revision() == account.revision()
                        && cooldown.until() > SystemTime::now()
                })
        };
        let (account_cooldown, model_cooldown) =
            futures::join!(self.quota.scheduling_cooldown(account), model_cooldown);
        if let Some(model_cooldown) = model_cooldown {
            // A failed account cooldown read must not erase a known model refusal.
            return Ok(account_cooldown
                .unwrap_or(None)
                .max(Some(model_cooldown.until())));
        }
        account_cooldown
    }

    pub(crate) async fn record_model_unsupported(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
    ) -> Result<(), ProviderStoreError> {
        let now = SystemTime::now();
        self.cooldowns
            .put_scoped_if_later(ProviderScopedCooldown::new(
                account.id().clone(),
                account.revision(),
                ProviderCooldownScope::upstream_model(model.clone()),
                now.checked_add(MODEL_UNSUPPORTED_COOLDOWN).unwrap_or(now),
            ))
            .await?;
        Ok(())
    }
}
