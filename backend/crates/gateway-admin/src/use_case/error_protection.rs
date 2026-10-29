//! Recovery uses the existing fixed-account diagnostic path and never clears quota state.
use super::{map_provider_error, map_store_error};
use crate::{
    model::{
        AdminError,
        accounts::{AccountRuntimeSnapshot, ConnectionTestEndpoint},
    },
    ports::{
        provider::ProviderAdminRegistry,
        store::{AccountRuntimeStore, AccountStore, SettingsStore},
    },
};
use chrono::{TimeDelta, Utc};
use gateway_core::{
    engine::probe::{AccountProbe, AccountProbeRequest},
    lifecycle::CancellationToken,
    provider_ports::AccountProtectionState,
    routing::UpstreamModelId,
};
use std::{sync::Arc, time::Duration};

pub(crate) struct ErrorProtectionRecoveryService {
    pub accounts: Arc<dyn AccountStore>,
    pub runtime: Arc<dyn AccountRuntimeStore>,
    pub settings: Arc<dyn SettingsStore>,
    pub providers: ProviderAdminRegistry,
    pub probe: Arc<dyn AccountProbe>,
}

impl ErrorProtectionRecoveryService {
    async fn policy(&self) -> Result<gateway_core::account::AccountErrorProtection, AdminError> {
        let settings = self
            .settings
            .load_runtime_settings()
            .await
            .map_err(|e| map_store_error(e, "protection settings"))?;
        Ok(settings
            .request_tuning
            .account_error_protection
            .unwrap_or_default())
    }

    async fn execute(&self, state: &AccountProtectionState) -> Result<Option<bool>, AdminError> {
        let item = self
            .accounts
            .load_account(state.account_id.as_str(), AccountRuntimeSnapshot::default())
            .await
            .map_err(|e| map_store_error(e, "protection account"))?;
        let Some(item) = item else {
            return Ok(Some(true));
        };
        if item.account.credential_revision.get() != state.credential_revision.get() {
            return Ok(Some(true));
        }
        if !item.account.enabled {
            return Ok(None);
        }
        let provider = self
            .providers
            .require(&item.account.provider_kind)
            .map_err(|e| map_provider_error(e, "protection provider"))?;
        let model = if let Some(model) = &state.probe_model {
            UpstreamModelId::new(model.clone())
                .map_err(|_| AdminError::invalid("invalid protection model"))?
        } else {
            let models = provider
                .models(&state.account_id, false)
                .await
                .map_err(|e| map_provider_error(e, "protection models"))?;
            let Some(model) = models.models.into_iter().next() else {
                return Ok(Some(false));
            };
            model.id
        };
        let operation = provider
            .connection_test_operation_with_options(
                &model,
                "Reply with OK only.",
                ConnectionTestEndpoint::Responses,
                true,
            )
            .map_err(|e| map_provider_error(e, "protection request"))?;
        let result = self
            .probe
            .probe(AccountProbeRequest {
                account_id: state.account_id.clone(),
                provider_kind: item.account.provider_kind,
                upstream_model: model,
                operation,
            })
            .await;
        Ok(Some(result.is_ok()))
    }

    pub(crate) async fn run_one(
        &self,
        cancellation: CancellationToken,
    ) -> Result<bool, AdminError> {
        let Some(state) = self
            .runtime
            .due_protections()
            .await
            .map_err(|e| map_store_error(e, "protection claim"))?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        let policy = self.policy().await?;
        let outcome = if !policy.enabled || !policy.probe_enabled {
            Some(true)
        } else {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => return Ok(false),
                result = tokio::time::timeout(Duration::from_secs(45), self.execute(&state)) =>
                    match result { Ok(Ok(value)) => value, _ => Some(false) },
            }
        };
        let Some(succeeded) = outcome else {
            return Ok(true);
        };
        if cancellation.is_cancelled() {
            return Ok(false);
        }
        let current = self.policy().await?;
        let postpone = (!succeeded && current.enabled && current.probe_enabled)
            .then(|| Utc::now() + TimeDelta::seconds(state.duration_seconds as i64));
        self.runtime
            .finish_protection(&state, postpone)
            .await
            .map_err(|e| map_store_error(e, "protection recovery"))?;
        Ok(true)
    }
}
