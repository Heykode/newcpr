use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::store::AdminStoreResult;
use crate::model::{MutationContext, quality_ops::*};

#[async_trait]
pub trait QualityOpsStore: Send + Sync {
    async fn templates(&self) -> AdminStoreResult<Vec<QualityRuleTemplate>>;
    async fn template(&self, id: &str) -> AdminStoreResult<Option<QualityRuleTemplate>>;
    async fn save_template(
        &self,
        id: Option<&str>,
        revision: Option<i64>,
        name: String,
        config: QualityRuleConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<QualityRuleTemplate>;
    async fn delete_template(
        &self,
        id: &str,
        revision: i64,
        context: &MutationContext,
    ) -> AdminStoreResult<()>;
    async fn apply_template(
        &self,
        template: &QualityRuleTemplate,
        target: &QualityTemplateTarget,
        next: DateTime<Utc>,
        context: &MutationContext,
    ) -> AdminStoreResult<QualityRule>;
    async fn monitoring(
        &self,
        account_ids: &[String],
    ) -> AdminStoreResult<std::collections::BTreeMap<String, QualityMonitoring>>;
    async fn rules(&self) -> AdminStoreResult<Vec<QualityRule>>;
    async fn save(
        &self,
        id: Option<&str>,
        revision: Option<i64>,
        config: QualityRuleConfig,
        next: DateTime<Utc>,
        context: &MutationContext,
    ) -> AdminStoreResult<QualityRule>;
    async fn delete(
        &self,
        id: &str,
        revision: i64,
        context: &MutationContext,
    ) -> AdminStoreResult<()>;
    async fn enqueue(
        &self,
        id: &str,
        revision: i64,
        context: &MutationContext,
    ) -> AdminStoreResult<()>;
    async fn claim(&self) -> AdminStoreResult<Option<QualityClaim>>;
    async fn current(&self, claim: &QualityClaim) -> AdminStoreResult<bool>;
    async fn finish(
        &self,
        claim: &QualityClaim,
        next: DateTime<Utc>,
        answers: Vec<QualityAnswer>,
    ) -> AdminStoreResult<()>;
    async fn runs(&self, rule_id: &str) -> AdminStoreResult<Vec<QualityRun>>;
    async fn detail(&self, run_id: &str) -> AdminStoreResult<Option<QualityRun>>;
    async fn cleanup(&self) -> AdminStoreResult<()>;
}
