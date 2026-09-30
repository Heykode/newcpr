use super::map_store_error;
use crate::{
    model::{AdminError, MutationContext, request_capture::*},
    ports::request_capture::{CaptureExport, RequestCaptureStore},
};
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait RequestCaptureService: Send + Sync {
    async fn settings(&self) -> Result<RequestCaptureSettings, AdminError>;
    async fn for_request(&self, request_id: &str) -> Result<Vec<CaptureRecord>, AdminError>;
    async fn status(&self) -> Result<RequestCaptureStatus, AdminError>;
    async fn configure(
        &self,
        config: RequestCaptureConfig,
        context: &MutationContext,
    ) -> Result<(), AdminError>;
    async fn create(
        &self,
        task: CreateCaptureTask,
        context: &MutationContext,
    ) -> Result<CaptureTask, AdminError>;
    async fn stop(&self, id: &str, context: &MutationContext) -> Result<(), AdminError>;
    async fn delete(&self, id: &str, context: &MutationContext) -> Result<(), AdminError>;
    async fn clear(
        &self,
        input: ClearCaptures,
        context: &MutationContext,
    ) -> Result<CaptureClearResult, AdminError>;
    async fn read(
        &self,
        id: &str,
        offset: u64,
        context: &MutationContext,
    ) -> Result<CapturePage, AdminError>;
    async fn export(
        &self,
        id: &str,
        context: &MutationContext,
    ) -> Result<CaptureExport, AdminError>;
    async fn export_task(
        &self,
        id: &str,
        context: &MutationContext,
    ) -> Result<CaptureExport, AdminError>;
}

pub(crate) struct DefaultRequestCaptureService(pub Option<Arc<dyn RequestCaptureStore>>);
impl DefaultRequestCaptureService {
    fn store(&self) -> Result<&dyn RequestCaptureStore, AdminError> {
        self.0
            .as_deref()
            .ok_or_else(|| AdminError::unavailable("请求采集存储未配置"))
    }
}

fn validate_id(id: &str) -> Result<(), AdminError> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| AdminError::invalid("采集标识不合法"))
}

#[async_trait]
impl RequestCaptureService for DefaultRequestCaptureService {
    async fn settings(&self) -> Result<RequestCaptureSettings, AdminError> {
        self.store()?
            .settings()
            .await
            .map_err(|e| map_store_error(e, "request capture"))
    }

    async fn for_request(&self, request_id: &str) -> Result<Vec<CaptureRecord>, AdminError> {
        if request_id.trim().is_empty()
            || request_id.len() > 256
            || request_id.chars().any(char::is_control)
        {
            return Err(AdminError::invalid("请求标识不合法"));
        }
        self.store()?
            .for_request(request_id)
            .await
            .map_err(|e| map_store_error(e, "request capture"))
    }
    async fn status(&self) -> Result<RequestCaptureStatus, AdminError> {
        self.store()?
            .status()
            .await
            .map_err(|e| map_store_error(e, "request capture"))
    }
    async fn configure(
        &self,
        config: RequestCaptureConfig,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        config.validate()?;
        self.store()?
            .configure(config, context)
            .await
            .map_err(|e| map_store_error(e, "request capture"))
    }
    async fn create(
        &self,
        task: CreateCaptureTask,
        context: &MutationContext,
    ) -> Result<CaptureTask, AdminError> {
        task.validate()?;
        self.store()?
            .create(task, context)
            .await
            .map_err(|e| map_store_error(e, "request capture"))
    }
    async fn stop(&self, id: &str, context: &MutationContext) -> Result<(), AdminError> {
        validate_id(id)?;
        self.store()?
            .stop(id, context)
            .await
            .map_err(|e| map_store_error(e, "request capture"))
    }
    async fn delete(&self, id: &str, context: &MutationContext) -> Result<(), AdminError> {
        validate_id(id)?;
        self.store()?
            .delete(id, context)
            .await
            .map_err(|e| map_store_error(e, "request capture"))
    }
    async fn clear(
        &self,
        input: ClearCaptures,
        context: &MutationContext,
    ) -> Result<CaptureClearResult, AdminError> {
        input.validate()?;
        self.store()?
            .clear(input, context)
            .await
            .map_err(|e| map_store_error(e, "request capture"))
    }
    async fn read(
        &self,
        id: &str,
        offset: u64,
        context: &MutationContext,
    ) -> Result<CapturePage, AdminError> {
        validate_id(id)?;
        self.store()?
            .read(id, offset, context)
            .await
            .map_err(|e| map_store_error(e, "request capture"))
    }
    async fn export(
        &self,
        id: &str,
        context: &MutationContext,
    ) -> Result<CaptureExport, AdminError> {
        validate_id(id)?;
        self.store()?
            .export(id, context)
            .await
            .map_err(|e| map_store_error(e, "request capture"))
    }

    async fn export_task(
        &self,
        id: &str,
        context: &MutationContext,
    ) -> Result<CaptureExport, AdminError> {
        validate_id(id)?;
        self.store()?
            .export_task(id, context)
            .await
            .map_err(|e| map_store_error(e, "request capture"))
    }
}
