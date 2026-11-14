//! Native Responses catalog qualification and credential-scoped model refusal.

use super::*;
use gateway_core::account::ResponsesUpstream;
use gateway_core::provider_ports::{ProviderCooldownScope, ProviderScopedCooldown};
use gateway_core::routing::UpstreamModelId;

const MODEL_UNSUPPORTED_COOLDOWN: Duration = Duration::from_secs(30 * 60);

impl CodexCredentialSelector {
    pub(super) async fn scheduling_cooldowns(
        &self,
        accounts: &[ProviderAccount],
        request: &CredentialSelectionInput<'_>,
    ) -> HashMap<ProviderAccountId, Option<SystemTime>> {
        let ids = accounts
            .iter()
            .filter(|account| account.credential_route() == ResponsesUpstream::Codex)
            .map(|account| account.id().clone())
            .collect::<Vec<_>>();
        let revisions = accounts
            .iter()
            .filter(|account| account.credential_route() == ResponsesUpstream::Codex)
            .map(|account| (account.id().clone(), account.revision()))
            .collect::<Vec<_>>();
        let native = accounts
            .iter()
            .filter(|account| Self::native_model(account, request).is_some())
            .collect::<Vec<_>>();
        let native_ids = native
            .iter()
            .map(|account| account.id().clone())
            .collect::<Vec<_>>();
        let scope = request
            .upstream_model
            .and_then(|model| UpstreamModelId::new(model).ok())
            .map(ProviderCooldownScope::upstream_model);
        let model_cooldowns = async {
            match scope.as_ref() {
                Some(scope) => self.cooldowns.read_scoped_many(&native_ids, scope).await,
                None => Vec::new(),
            }
        };
        let (cooldowns, protections, models) = futures::join!(
            self.cooldowns.read_many(&ids),
            self.cooldowns.read_protection_many(&revisions),
            model_cooldowns
        );
        let now = SystemTime::now();
        let mut limits = HashMap::with_capacity(accounts.len());
        for ((id, cooldown), protection) in ids.iter().zip(cooldowns).zip(protections) {
            let until = cooldown
                .ok()
                .flatten()
                .map(|value| value.until())
                .filter(|until| *until > now);
            let protected = protection
                .ok()
                .flatten()
                .map(|value| value.scheduling_until(now));
            limits.insert(id.clone(), until.max(protected));
        }
        for (account, cooldown) in native.into_iter().zip(models) {
            if let Some(cooldown) = cooldown.ok().flatten().filter(|value| {
                value.credential_revision() == account.revision() && value.until() > now
            }) {
                let until = limits.entry(account.id().clone()).or_default();
                *until = (*until).max(Some(cooldown.until()));
            }
        }
        limits
    }

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
        let mut excluded = self
            .catalog
            .scheduling_exclusions(accounts, model, SystemTime::now());
        if let Some(scope) = request.attempt.account_scope() {
            excluded.retain(|id| {
                scope.directory().account(id).is_none_or(|account| {
                    account.model_access().mode() != AccountModelAccessMode::Allowlist
                })
            });
        }
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

    pub(crate) async fn record_model_unsupported(
        &self,
        account: &ProviderAccount,
        model: &UpstreamModelId,
    ) -> Result<(), ProviderStoreError> {
        if account.credential_route() != ResponsesUpstream::Codex {
            return Ok(());
        }
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
