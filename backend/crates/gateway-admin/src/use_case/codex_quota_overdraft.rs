use std::{sync::Arc, time::Duration};

use chrono::{TimeDelta, Utc};
use gateway_core::{
    engine::probe::{AccountProbe, AccountProbeRequest},
    lifecycle::CancellationToken,
};

use crate::{
    model::{
        AdminError, accounts::ConnectionTestEndpoint,
        codex_quota_overdraft::CodexQuotaOverdraftOutcome,
    },
    ports::{provider::ProviderAdmin, store::AccountStore},
    use_case::{map_provider_error, map_store_error},
};

pub struct CodexQuotaOverdraftService {
    pub accounts: Arc<dyn AccountStore>,
    pub provider: Arc<dyn ProviderAdmin>,
    pub probe: Arc<dyn AccountProbe>,
}

impl CodexQuotaOverdraftService {
    pub async fn run_one(&self, cancellation: CancellationToken) -> Result<bool, AdminError> {
        let Some(claim) = self
            .accounts
            .claim_codex_quota_overdraft()
            .await
            .map_err(|error| map_store_error(error, "Codex quota overdraft"))?
        else {
            return Ok(false);
        };
        let operation = self
            .provider
            .connection_test_operation_with_options(
                &claim.probe_model,
                "Reply with OK only.",
                ConnectionTestEndpoint::Responses,
                true,
            )
            .map_err(|error| map_provider_error(error, "Codex quota overdraft probe"))?;
        let request = AccountProbeRequest {
            account_id: claim.account_id.clone(),
            provider_kind: claim.provider_kind.clone(),
            upstream_model: claim.probe_model.clone(),
            operation,
        };
        let tested_at = Utc::now();
        let attempt_cancellation = CancellationToken::new();
        let probe = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                attempt_cancellation.cancel();
                return Ok(false);
            }
            result = tokio::time::timeout(
                Duration::from_secs(20),
                self.probe
                    .codex_quota_overdraft(request, attempt_cancellation.clone()),
            ) => result,
        };
        if probe.is_err() {
            attempt_cancellation.cancel();
        }
        let outcome = match probe {
            Ok(Ok(_)) => CodexQuotaOverdraftOutcome::Passed { tested_at },
            Err(_) => CodexQuotaOverdraftOutcome::Inconclusive {
                reason_code: "timeout".to_owned(),
                tested_at,
                retry_at: tested_at + TimeDelta::minutes(1),
            },
            Ok(Err(error)) => CodexQuotaOverdraftOutcome::from_probe_error(error, tested_at),
        };
        self.accounts
            .finish_codex_quota_overdraft(&claim, outcome)
            .await
            .map_err(|error| map_store_error(error, "Codex quota overdraft"))?;
        Ok(true)
    }
}
