use super::store::AdminStoreResult;
use crate::model::{MutationContext, log_cleanup::*};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

#[async_trait]
pub trait LogCleanupStore: Send + Sync {
    async fn state(&self) -> AdminStoreResult<CleanupState>;
    async fn footprint(&self) -> AdminStoreResult<CleanupFootprint>;
    async fn configure(
        &self,
        command: CleanupCommand,
        context: &MutationContext,
    ) -> AdminStoreResult<()>;
    async fn start(
        &self,
        preview: CleanupPreview,
        context: &MutationContext,
    ) -> AdminStoreResult<CleanupJob>;
    async fn cancel(&self, id: &str, context: &MutationContext) -> AdminStoreResult<()>;
    async fn start_capture_clear(&self, context: &MutationContext) -> AdminStoreResult<CleanupJob>;
}
/// Host-owned access to a fixed directory, never an administrator-supplied path.
#[async_trait]
pub trait LogFileMaintenance: Send + Sync {
    async fn bytes(&self) -> AdminStoreResult<u64>;
    async fn clean(&self, cutoff: DateTime<Utc>, limit: usize) -> AdminStoreResult<CleanupBatch>;
}
