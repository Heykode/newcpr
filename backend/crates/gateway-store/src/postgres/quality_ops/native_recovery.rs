//! Only reverse an Excel route change owned by this rule, never an account template.

use super::*;
use gateway_admin::model::MutationActor;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct ExcelOwnership {
    scope: serde_json::Value,
    identity: (Option<String>, Option<String>),
    model: String,
}

pub(super) async fn record(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
) -> AdminStoreResult<()> {
    if claim.rule.config.detection_mode != QualityDetectionMode::StateProbe {
        return Ok(());
    }
    // Read after the account audit was appended, so even a later same-value
    // manual edit supersedes this ownership. Credential refresh is not reverted.
    let owner = ExcelOwnership {
        scope: action_scope(tx, &claim.rule.config.account_id).await?,
        identity: claim.account_identity.clone(),
        model: claim.rule.config.model.clone(),
    };
    sqlx::query("update quality_rules set recovery=jsonb_set(recovery-'excel_streak','{excel_owner}',$2) where id=$1")
        .bind(&claim.rule.id)
        .bind(serde_json::to_value(owner).map_err(unavailable)?)
        .execute(&mut **tx).await.map_err(unavailable)?;
    Ok(())
}

pub(super) async fn apply(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
    status: &str,
    recovery: &serde_json::Value,
) -> AdminStoreResult<Option<&'static str>> {
    let Some(value) = recovery.get("excel_owner") else {
        return Ok(None);
    };
    let owner: ExcelOwnership = serde_json::from_value(value.clone()).map_err(unavailable)?;
    if owner.scope != claim.action_scope
        || owner.identity != claim.account_identity
        || owner.model != claim.rule.config.model
        || claim.rule.config.detection_mode != QualityDetectionMode::StateProbe
    {
        release(tx, &claim.rule.id).await?;
        return Ok(Some("excel_recovery_released"));
    }
    if status != "correct" || !claim.rule.config.disable_excel_on_native_recovery {
        return Ok(Some("excel_recovery_pending"));
    }
    // The finish transaction already holds the configuration and rule lease
    // fences. Still CAS the route/identity and do not clear a 403 protection flag.
    let changed = sqlx::query(
        "update provider_accounts set responses_upstream='codex',
        updated_at=greatest(now(),updated_at) where id=$1 and responses_upstream='excel'
        and upstream_user_id is not distinct from $2 and upstream_account_id is not distinct from $3
        and excel_mode_disabled_at is null and excel_auto_disabled_at is null",
    )
    .bind(&claim.rule.config.account_id)
    .bind(&owner.identity.0)
    .bind(&owner.identity.1)
    .execute(&mut **tx)
    .await
    .map_err(unavailable)?
    .rows_affected()
        == 1;
    release(tx, &claim.rule.id).await?;
    if !changed {
        return Ok(Some("excel_recovery_released"));
    }
    let revision = bump_config_revision_in_transaction(tx)
        .await
        .map_err(unavailable)?;
    append_admin_audit_event_in_transaction(
        tx,
        mutation_audit(
            &MutationContext {
                actor: MutationActor::System,
                request_id: claim.run_id.clone(),
            },
            "quality_rule.native_recovery",
            "provider_account",
            &claim.rule.config.account_id,
            vec!["responses_upstream".into()],
        ),
        revision,
    )
    .await
    .map_err(unavailable)?;
    Ok(Some("excel_disabled_native_recovered"))
}

async fn release(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &str,
) -> AdminStoreResult<()> {
    sqlx::query(
        "update quality_rules set recovery=recovery-'excel_owner'-'excel_streak' where id=$1",
    )
    .bind(rule)
    .execute(&mut **tx)
    .await
    .map_err(unavailable)?;
    Ok(())
}
