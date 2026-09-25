use async_trait::async_trait;

use super::store::AdminStoreResult;
use crate::model::{
    MutationContext,
    token_guard::{TokenGuardConfig, TokenGuardEvent},
};

#[async_trait]
pub trait TokenGuardStore: Send + Sync {
    async fn config(&self) -> AdminStoreResult<TokenGuardConfig>;
    async fn configure(
        &self,
        config: &TokenGuardConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<()>;
    async fn latest(&self) -> AdminStoreResult<Vec<TokenGuardEvent>>;
    async fn events(&self) -> AdminStoreResult<Vec<TokenGuardEvent>>;
    async fn record(&self, event: &TokenGuardEvent) -> AdminStoreResult<()>;
    async fn audit_run(&self, context: &MutationContext) -> AdminStoreResult<()>;
}
