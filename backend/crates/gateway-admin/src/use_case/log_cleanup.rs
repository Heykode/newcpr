use super::map_store_error;
use crate::{
    model::{AdminError, MutationContext, log_cleanup::*},
    ports::log_cleanup::LogCleanupStore,
};
use std::sync::Arc;

pub struct LogCleanupService(pub(crate) Option<Arc<dyn LogCleanupStore>>);
impl LogCleanupService {
    fn store(&self) -> Result<&dyn LogCleanupStore, AdminError> {
        self.0
            .as_deref()
            .ok_or_else(|| AdminError::unavailable("日志清理未配置"))
    }
    pub async fn state(&self) -> Result<CleanupState, AdminError> {
        self.store()?
            .state()
            .await
            .map_err(|e| map_store_error(e, "log cleanup"))
    }
    pub async fn footprint(&self) -> Result<CleanupFootprint, AdminError> {
        self.store()?
            .footprint()
            .await
            .map_err(|e| map_store_error(e, "log cleanup"))
    }
    pub async fn configure(
        &self,
        command: CleanupCommand,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        command.config.validate()?;
        self.store()?
            .configure(command, context)
            .await
            .map_err(|e| map_store_error(e, "log cleanup"))
    }
    pub async fn preview(&self) -> Result<CleanupPreview, AdminError> {
        let state = self.state().await?;
        let preview = CleanupPreview {
            revision: state.revision,
            config: state.config,
            cutoff_at: chrono::Utc::now(),
        };
        preview.validate()?;
        Ok(preview)
    }
    pub async fn start(
        &self,
        preview: CleanupPreview,
        context: &MutationContext,
    ) -> Result<CleanupJob, AdminError> {
        preview.validate()?;
        self.store()?
            .start(preview, context)
            .await
            .map_err(|e| map_store_error(e, "log cleanup"))
    }
    pub async fn cancel(&self, id: &str, context: &MutationContext) -> Result<(), AdminError> {
        uuid::Uuid::parse_str(id).map_err(|_| AdminError::invalid("清理任务标识不合法"))?;
        self.store()?
            .cancel(id, context)
            .await
            .map_err(|e| map_store_error(e, "log cleanup"))
    }
    pub async fn start_capture_clear(
        &self,
        confirmed: bool,
        context: &MutationContext,
    ) -> Result<CleanupJob, AdminError> {
        if !confirmed {
            return Err(AdminError::invalid("请确认清理采集材料"));
        }
        self.store()?
            .start_capture_clear(context)
            .await
            .map_err(|e| map_store_error(e, "log cleanup"))
    }
}
