use crate::model::{AdminError, MutationContext, mihomo::*};
use async_trait::async_trait;

/// Host owns the managed process and files; admin owns authenticated access.
#[async_trait]
pub trait MihomoManagement: Send + Sync {
    async fn status(&self) -> Result<MihomoStatus, AdminError>;
    async fn submit(
        &self,
        command: MihomoCommand,
        context: &MutationContext,
    ) -> Result<MihomoStatus, AdminError>;
    async fn test_node(
        &self,
        node: &str,
        quality: bool,
        context: &MutationContext,
    ) -> Result<MihomoNodeCheck, AdminError>;
}
