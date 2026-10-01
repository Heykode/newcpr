//! Real BPS health checks for opt-in paused accounts, separate from native quality.

use super::{map_provider_error, map_store_error};
use crate::{
    model::{AdminError, excel_recovery::*},
    ports::{provider::ProviderAdminRegistry, store::AccountStore},
};
use gateway_core::{
    account::ProviderAccountId,
    engine::probe::{AccountProbe, AccountProbeRequest},
    lifecycle::CancellationToken,
    routing::{ProviderKind, UpstreamModelId},
};
use std::{sync::Arc, time::Duration};

pub(crate) struct ExcelRecoveryService {
    pub accounts: Arc<dyn AccountStore>,
    pub providers: ProviderAdminRegistry,
    pub probe: Arc<dyn AccountProbe>,
}

impl ExcelRecoveryService {
    async fn execute(
        &self,
        claim: &ExcelRecoveryClaim,
        cancel: CancellationToken,
    ) -> Result<ExcelRecoveryOutcome, AdminError> {
        let provider = ProviderKind::new("openai")
            .map_err(|_| AdminError::internal("invalid recovery provider"))?;
        let model = UpstreamModelId::new(claim.model.clone())
            .map_err(|_| AdminError::invalid("invalid recovery model"))?;
        let nonce = uuid::Uuid::now_v7().to_string();
        let operation = self
            .providers
            .require(&provider)
            .map_err(|error| map_provider_error(error, "Excel recovery provider"))?
            .excel_recovery_operation(
                &model,
                &format!("Reply with exactly this nonce and nothing else: {nonce}"),
            )
            .map_err(|error| map_provider_error(error, "Excel recovery request"))?;
        let result = self
            .probe
            .excel_recovery(
                AccountProbeRequest {
                    account_id: ProviderAccountId::new(claim.account_id.clone())
                        .map_err(|_| AdminError::invalid("invalid recovery account"))?,
                    provider_kind: provider,
                    upstream_model: model,
                    operation,
                },
                claim.credential_revision,
                claim.config_revision,
                nonce.clone(),
                cancel.clone(),
            )
            .await;
        if cancel.is_cancelled() {
            return Ok(ExcelRecoveryOutcome::Cancelled);
        }
        Ok(match result {
            Ok(response) if response.text.concat().trim() == nonce => {
                ExcelRecoveryOutcome::Recovered
            }
            Ok(_) => ExcelRecoveryOutcome::ResponseMismatch,
            Err(_) => ExcelRecoveryOutcome::RequestFailed,
        })
    }

    pub(crate) async fn run_one(
        &self,
        cancellation: CancellationToken,
    ) -> Result<bool, AdminError> {
        let Some(claim) = self
            .accounts
            .claim_excel_recovery()
            .await
            .map_err(|e| map_store_error(e, "Excel recovery claim"))?
        else {
            return Ok(false);
        };
        let local = CancellationToken::new();
        let work = self.execute(&claim, local.clone());
        tokio::pin!(work);
        let deadline = tokio::time::sleep(Duration::from_secs(45));
        tokio::pin!(deadline);
        let watch_claim = async {
            loop {
                let current = tokio::time::timeout(
                    Duration::from_secs(2),
                    self.accounts.excel_recovery_current(&claim),
                )
                .await;
                if !matches!(current, Ok(Ok(true))) {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        };
        tokio::pin!(watch_claim);
        let outcome = tokio::select! {
                biased;
                () = cancellation.cancelled() => { local.cancel(); let _ = work.await; ExcelRecoveryOutcome::Cancelled }
                () = &mut deadline => { local.cancel(); let _ = work.await; ExcelRecoveryOutcome::RequestFailed }
                result = &mut work => result.unwrap_or(ExcelRecoveryOutcome::RequestFailed),
                () = &mut watch_claim => {
                    local.cancel(); let _ = work.await; ExcelRecoveryOutcome::Cancelled
                }
        };
        self.accounts
            .finish_excel_recovery(&claim, outcome)
            .await
            .map_err(|e| map_store_error(e, "Excel recovery result"))?;
        Ok(true)
    }
}
