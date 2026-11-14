//! Project the selected credential before scheduling; logical capacity stays shared.

use super::*;

impl CodexCredentialSelector {
    pub(super) async fn record_excel_quota(
        &self,
        account: &ProviderAccount,
        state: gateway_core::account::QuotaState,
        observed_at: SystemTime,
    ) -> Result<(), CredentialSelectionError> {
        self.repository
            .store()
            .compare_and_swap_excel_quota(gateway_core::account::QuotaObservation {
                quota_pause: None,
                account_id: account.id().clone(),
                expected_revision: account.credential_revision(),
                quota: gateway_core::account::OpaqueProviderData::new(serde_json::Map::new()),
                plan_type: None,
                observed_at,
                state,
            })
            .await
            .map_err(|source| CredentialSelectionError::Store(Some(source.into())))?;
        Ok(())
    }

    pub(super) fn project_request_account(
        account: ProviderAccount,
        request: &CredentialSelectionInput<'_>,
    ) -> Option<ProviderAccount> {
        let route = Self::request_upstream(&account, request);
        account.for_credential_route(route)
    }

    pub(super) async fn prepare_route_scheduling(&self, accounts: &[ProviderAccount]) {
        let native = accounts
            .iter()
            .filter(|account| account.credential_route() == ResponsesUpstream::Codex)
            .cloned()
            .collect::<Vec<_>>();
        if !native.is_empty() {
            self.quota.prepare_scheduling(&native).await;
        }
    }

    pub(super) fn route_scheduling_signals(
        &self,
        account: &ProviderAccount,
        mut signals: AccountRuntimeSignals,
    ) -> AccountRuntimeSignals {
        if account.credential_route() == ResponsesUpstream::Codex {
            let health = self
                .account_feedback
                .scheduling_signals(&self.provider_kind, account.id());
            return signals
                .with_provider_quota(self.quota.scheduling_signals(account))
                .with_runtime_health(health.0, health.1);
        }
        signals.quota_reset_at = account.quota().reset_at();
        signals.quota_reset_windows = None;
        signals.quota_remaining_rank = None;
        signals.with_runtime_health(None, None)
    }
}
