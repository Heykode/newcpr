//! 价格覆盖与请求费用明细的持久化

use gateway_admin::model::pricing::{PricingChange, PricingSyncChanges, UpdatePricing};
use gateway_core::{
    metering::{ModelPriceOverride, PricingOverrides},
    routing::ProviderKind,
};
use sqlx::types::Json;

use super::{
    AdminAuditEvent, PgControlPlaneRepository, append_admin_audit_event_in_transaction,
    bump_config_revision_in_transaction,
};
use crate::{Revision, StoreError, StoreResult, postgres_unavailable};

pub(crate) fn validate_pricing(pricing: &PricingOverrides) -> StoreResult<()> {
    if pricing
        .values()
        .map(std::collections::BTreeMap::len)
        .sum::<usize>()
        > 10_000
    {
        return Err(invalid_pricing());
    }
    for (provider, models) in pricing {
        if ProviderKind::new(provider.clone()).is_err() {
            return Err(invalid_pricing());
        }
        for (model, pricing) in models {
            if model.is_empty()
                || model.len() > 128
                || model.chars().any(char::is_whitespace)
                || model.chars().any(char::is_control)
                || pricing.validate().is_err()
            {
                return Err(invalid_pricing());
            }
        }
    }
    Ok(())
}

fn invalid_pricing() -> StoreError {
    StoreError::InvalidData {
        entity: "model pricing",
        message: "invalid model pricing".to_owned(),
    }
}

impl PgControlPlaneRepository {
    pub(crate) async fn load_pricing(
        &self,
    ) -> StoreResult<gateway_admin::model::pricing::StoredPricing> {
        let (Json(overrides), Json(synced), synced_at) = sqlx::query_as::<_, (Json<PricingOverrides>, Json<PricingOverrides>, Option<chrono::DateTime<chrono::Utc>>)>(
            "select pricing_overrides_json, pricing_synced_json, pricing_synced_at from runtime_settings where id = 1")
            .fetch_one(&self.pool).await.map_err(|source| postgres_unavailable("load model pricing").with_source(source))?;
        validate_pricing(&overrides)?;
        validate_pricing(&synced)?;
        Ok(gateway_admin::model::pricing::StoredPricing {
            overrides,
            synced,
            synced_at,
        })
    }

    pub(crate) async fn sync_pricing(
        &self,
        changes: PricingSyncChanges,
        audit: AdminAuditEvent,
    ) -> StoreResult<Revision> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|source| postgres_unavailable("begin pricing sync").with_source(source))?;
        let Json(mut prices) = sqlx::query_scalar::<_, Json<PricingOverrides>>(
            "select pricing_synced_json from runtime_settings where id = 1 for update",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(|source| postgres_unavailable("lock synced model pricing").with_source(source))?;
        for (provider, models) in changes {
            let stored = prices.entry(provider).or_default();
            for (model, price) in models {
                if let Some(price) = price {
                    stored.insert(model, price);
                } else {
                    stored.remove(&model);
                }
            }
        }
        prices.retain(|_, models| !models.is_empty());
        validate_pricing(&prices)?;
        sqlx::query("update runtime_settings set pricing_synced_json = $1, pricing_synced_at = now() where id = 1")
            .bind(Json(prices)).execute(&mut *transaction).await.map_err(|source| postgres_unavailable("sync model pricing").with_source(source))?;
        let revision = bump_config_revision_in_transaction(&mut transaction).await?;
        append_admin_audit_event_in_transaction(&mut transaction, audit, revision).await?;
        transaction
            .commit()
            .await
            .map_err(|source| postgres_unavailable("commit pricing sync").with_source(source))?;
        Ok(revision)
    }

    pub(crate) async fn update_pricing(
        &self,
        command: UpdatePricing,
        audit: AdminAuditEvent,
    ) -> StoreResult<Revision> {
        let mut transaction = self.pool.begin().await.map_err(|source| {
            postgres_unavailable("begin model pricing update").with_source(source)
        })?;
        // 锁定当前配置再更新选中项，避免批量操作覆盖其他管理员已提交的模型
        let (Json(mut pricing), Json(mut synced)) = sqlx::query_as::<_, (Json<PricingOverrides>, Json<PricingOverrides>)>(
            "select pricing_overrides_json, pricing_synced_json from runtime_settings where id = 1 for update",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(|source| postgres_unavailable("lock model pricing").with_source(source))?;
        let models = pricing.entry(command.provider.clone()).or_default();
        let source = synced.entry(command.provider).or_default();
        for model in command.models {
            match &command.change {
                PricingChange::Reset => {
                    models.remove(&model);
                }
                PricingChange::Delete => {
                    models.remove(&model);
                    source.remove(&model);
                }
                PricingChange::Replace(value) => {
                    models.insert(model, value.clone());
                }
                PricingChange::Multiplier(bps) => {
                    models
                        .entry(model)
                        .or_insert_with(|| ModelPriceOverride {
                            multiplier_bps: 10_000,
                            bands: Default::default(),
                        })
                        .multiplier_bps = *bps;
                }
            }
        }
        pricing.retain(|_, models| !models.is_empty());
        synced.retain(|_, models| !models.is_empty());
        validate_pricing(&pricing)?;
        sqlx::query("update runtime_settings set pricing_overrides_json = $1, pricing_synced_json = $2 where id = 1")
            .bind(Json(pricing))
            .bind(Json(synced))
            .execute(&mut *transaction)
            .await
            .map_err(|source| postgres_unavailable("update model pricing").with_source(source))?;
        let revision = bump_config_revision_in_transaction(&mut transaction).await?;
        append_admin_audit_event_in_transaction(&mut transaction, audit, revision).await?;
        transaction
            .commit()
            .await
            .map_err(|source| postgres_unavailable("commit model pricing").with_source(source))?;
        Ok(revision)
    }
}
