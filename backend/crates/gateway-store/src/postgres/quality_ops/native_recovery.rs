//! Legacy quality ownership is released without changing the account's chosen route.

use super::*;

pub(super) async fn apply(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
    _status: &str,
    recovery: &serde_json::Value,
) -> AdminStoreResult<Option<&'static str>> {
    if recovery.get("excel_owner").is_none() {
        return Ok(None);
    }
    sqlx::query("update quality_rules set recovery=recovery-'excel_owner'-'excel_streak'-'excel_pass_streak' where id=$1")
        .bind(&claim.rule.id).execute(&mut **tx).await.map_err(unavailable)?;
    Ok(Some("excel_recovery_released"))
}
