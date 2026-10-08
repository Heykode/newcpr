//! Two abnormal rounds with explicit overload evidence, scoped to the unchanged account.

use super::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub(super) struct OverloadStreak {
    count: u8,
    overloaded: bool,
    scope: serde_json::Value,
    identity: (Option<String>, Option<String>),
    model: String,
    mode: QualityDetectionMode,
}

/// Called only after the run's lease/revision fence, in the result transaction.
pub(super) async fn advance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
    status: &str,
    overloaded: bool,
) -> AdminStoreResult<bool> {
    if status == "correct" {
        sqlx::query("update quality_rules set recovery=recovery-'overload_streak' where id=$1")
            .bind(&claim.rule.id)
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
        return Ok(false);
    }
    if status != "incorrect" && status != "overloaded" {
        return Ok(false);
    }
    if action_scope(tx, &claim.rule.config.account_id).await? != claim.action_scope {
        return Ok(false);
    }
    let value: Option<serde_json::Value> =
        sqlx::query_scalar("select recovery->'overload_streak' from quality_rules where id=$1")
            .bind(&claim.rule.id)
            .fetch_one(&mut **tx)
            .await
            .map_err(unavailable)?;
    let previous = value
        .map(serde_json::from_value::<OverloadStreak>)
        .transpose()
        .map_err(unavailable)?
        .filter(|previous| {
            previous.scope == claim.action_scope
                && previous.identity == claim.account_identity
                && previous.model == claim.rule.config.model
                && previous.mode == claim.rule.config.detection_mode
        });
    let count = previous
        .as_ref()
        .map_or(0, |previous| previous.count)
        .saturating_add(1)
        .min(2);
    let state = OverloadStreak {
        count,
        overloaded: overloaded || previous.is_some_and(|previous| previous.overloaded),
        scope: claim.action_scope.clone(),
        identity: claim.account_identity.clone(),
        model: claim.rule.config.model.clone(),
        mode: claim.rule.config.detection_mode,
    };
    let reached = state.count == 2 && state.overloaded;
    // Overload is abnormal evidence, so it cannot bridge healthy recovery rounds.
    sqlx::query("update quality_rules set recovery=jsonb_set(
        case when recovery ? 'excel_pass_streak' then jsonb_set(recovery,'{excel_pass_streak}','0'::jsonb)
        else recovery end,'{overload_streak}',$2) where id=$1")
        .bind(&claim.rule.id)
        .bind(serde_json::to_value(state).map_err(unavailable)?)
        .execute(&mut **tx).await.map_err(unavailable)?;
    Ok(reached)
}
