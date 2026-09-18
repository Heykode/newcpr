//! Storage operations do not interpret the provider-owned credential document.

use super::store::AdminStoreResult;
use crate::model::relogin::{ReloginEntry, ReloginSettings};
use crate::model::relogin_templates::{ReloginTemplate, ReloginTemplateConfig};
use async_trait::async_trait;

#[async_trait]
pub trait ReloginStore: Send + Sync {
    async fn entries(&self) -> AdminStoreResult<Vec<ReloginEntry>>;
    /// None creates a row; Some requires exactly that revision.
    async fn save(&self, entry: &ReloginEntry, expected: Option<u64>) -> AdminStoreResult<()>;
    /// All rows must commit together or leave the library unchanged.
    async fn save_batch(&self, entries: &[(ReloginEntry, Option<u64>)]) -> AdminStoreResult<()>;
    async fn delete(&self, id: &str, expected: u64) -> AdminStoreResult<()>;
    async fn settings(&self) -> AdminStoreResult<ReloginSettings>;
    async fn save_settings(&self, settings: &ReloginSettings) -> AdminStoreResult<()>;
    async fn templates(&self) -> AdminStoreResult<Vec<ReloginTemplate>>;
    async fn save_template(
        &self,
        template: &ReloginTemplate,
        expected: Option<u64>,
    ) -> AdminStoreResult<()>;
    async fn delete_template(&self, id: &str, expected: u64) -> AdminStoreResult<()>;
    /// Read-only preflight; import transactions remain authoritative for references.
    async fn validate_template_references(
        &self,
        config: &ReloginTemplateConfig,
    ) -> AdminStoreResult<()>;
}
