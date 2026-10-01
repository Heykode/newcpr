use super::store::AdminStoreResult;
use crate::model::{
    MutationContext,
    provider_credentials::{ConsumeProviderResetCredit, ProviderResetCreditResult},
    reset_credits::*,
};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait ResetCreditsStore: Send + Sync {
    async fn inventories(&self, ids: &[String]) -> AdminStoreResult<Vec<ResetInventory>>;
    async fn save_inventory(&self, inventory: ResetInventory) -> AdminStoreResult<()>;
    async fn save_preview(&self, batch: ResetBatch) -> AdminStoreResult<ResetBatch>;
    async fn batches(&self) -> AdminStoreResult<Vec<ResetBatch>>;
    async fn confirm(&self, id: Uuid, context: &MutationContext) -> AdminStoreResult<ResetBatch>;
    async fn retry(
        &self,
        id: Uuid,
        account_id: &str,
        context: &MutationContext,
    ) -> AdminStoreResult<()>;
    async fn claim(&self) -> AdminStoreResult<Option<(ResetBatch, ResetBatchItem)>>;
    async fn finish_item(&self, batch: Uuid, item: ResetBatchItem) -> AdminStoreResult<()>;
    async fn begin_consume(
        &self,
        command: &ConsumeProviderResetCredit,
        context: &MutationContext,
    ) -> AdminStoreResult<ResetConsumePermit>;
    async fn finish_consume(
        &self,
        command: &ConsumeProviderResetCredit,
        claim: Uuid,
        result: Option<&ProviderResetCreditResult>,
    ) -> AdminStoreResult<()>;
}
