use super::*;
use crate::model::{MutationActor, provider_credentials::ProviderResetCredit};

impl ResetCreditsService {
    pub async fn auto_policy(&self, id: &str) -> Result<AutoResetPolicy, AdminError> {
        self.store()?
            .auto_policy(id)
            .await
            .map_err(|e| map_store_error(e, "auto reset policy"))
    }

    pub async fn save_auto_policy(
        &self,
        id: &str,
        revision: i64,
        config: AutoResetConfig,
        context: &MutationContext,
    ) -> Result<AutoResetPolicy, AdminError> {
        config.validate()?;
        ProviderAccountId::new(id).map_err(|_| AdminError::invalid("账号标识不合法"))?;
        self.store()?
            .save_auto_policy(id, revision, config, context)
            .await
            .map_err(|e| map_store_error(e, "auto reset policy"))
    }

    pub(super) async fn fresh_auto_observation(
        &self,
        id: &ProviderAccountId,
        config: &AutoResetConfig,
    ) -> Result<AutoResetObservation, AdminError> {
        let started = Utc::now();
        self.accounts.quota(id, true).await?;
        let quota = self.accounts.current_quota(id).await?;
        config.observation(&quota, started, Utc::now())
    }

    async fn inspect_auto(
        &self,
        check: &AutoResetCheck,
    ) -> Result<(AutoResetObservation, Option<ProviderResetCredit>), AdminError> {
        let id = ProviderAccountId::new(check.policy.account_id.clone())
            .map_err(|_| AdminError::invalid("账号标识不合法"))?;
        let observation = self
            .fresh_auto_observation(&id, &check.policy.config)
            .await?;
        let credit = if observation.triggered.is_empty() {
            None
        } else {
            let context = MutationContext {
                actor: MutationActor::System,
                request_id: format!("auto-reset-check-{}", check.claim_id),
            };
            let inventory = self.accounts.reset_credits(&context, id).await?;
            Some(earliest_credit(&inventory, None, Utc::now())?)
        };
        Ok((observation, credit))
    }

    pub(crate) async fn check_auto_one(&self) -> Result<bool, AdminError> {
        let Some(store) = self.store.as_deref() else {
            return Ok(false);
        };
        let Some(check) = store
            .claim_auto_check()
            .await
            .map_err(|e| map_store_error(e, "auto reset check"))?
        else {
            return Ok(false);
        };
        let (observation, credit, message) =
            match tokio::time::timeout(Duration::from_secs(90), self.inspect_auto(&check)).await {
                Ok(Ok((observation, credit))) => {
                    (Some(observation), credit, "额度检查完成".to_owned())
                }
                Ok(Err(e)) => (None, None, e.message().to_owned()),
                Err(_) => (None, None, "额度检查超时，本次未消费".to_owned()),
            };
        store
            .finish_auto_check(&check, observation, credit, &message)
            .await
            .map_err(|e| map_store_error(e, "auto reset check"))?;
        Ok(true)
    }
}
