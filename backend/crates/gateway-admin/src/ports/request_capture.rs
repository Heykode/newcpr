use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

use super::store::AdminStoreResult;
use crate::model::{MutationContext, request_capture::*};

pub type CaptureExport = Pin<Box<dyn Stream<Item = AdminStoreResult<Vec<u8>>> + Send>>;

#[async_trait]
pub trait RequestCaptureStore: Send + Sync {
    async fn status(&self) -> AdminStoreResult<RequestCaptureStatus>;
    async fn configure(
        &self,
        config: RequestCaptureConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<()>;
    async fn create(
        &self,
        task: CreateCaptureTask,
        context: &MutationContext,
    ) -> AdminStoreResult<CaptureTask>;
    async fn stop(&self, id: &str, context: &MutationContext) -> AdminStoreResult<()>;
    async fn delete(&self, id: &str, context: &MutationContext) -> AdminStoreResult<()>;
    async fn read(
        &self,
        id: &str,
        offset: u64,
        context: &MutationContext,
    ) -> AdminStoreResult<CapturePage>;
    async fn export(&self, id: &str, context: &MutationContext) -> AdminStoreResult<CaptureExport>;
    async fn export_task(
        &self,
        id: &str,
        context: &MutationContext,
    ) -> AdminStoreResult<CaptureExport>;
}
