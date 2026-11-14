//! Excel answer checks own only the Excel suspension, never shared account settings.

use super::*;
use gateway_admin::model::MutationActor;

pub(super) async fn apply(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
    status: &str,
    overload_trigger: bool,
) -> AdminStoreResult<Option<&'static str>> {
    let config = &claim.rule.config;
    if config.failure_action == QualityFailureAction::EnableExcel {
        return Ok(Some("route_switch_requires_confirmation"));
    }
    if matches!(
        config.failure_action,
        QualityFailureAction::RemoveGroups | QualityFailureAction::ApplyAccountTemplate
    ) {
        return Ok(Some("excel_shared_action_blocked"));
    }
    if action_scope(tx, &config.account_id).await? != claim.action_scope {
        return Ok(Some("excel_blocked_configuration_changed"));
    }
    if config.failure_action != QualityFailureAction::DisableScheduling {
        return Ok(None);
    }
    let value: serde_json::Value =
        sqlx::query_scalar("select recovery from quality_rules where id=$1")
            .bind(&claim.rule.id)
            .fetch_one(&mut **tx)
            .await
            .map_err(unavailable)?;
    let changed = if status == "incorrect" {
        let count = super::policy::update_failure_streak(tx, claim, status, &value).await?;
        if !overload_trigger && count < config.failure_threshold.unwrap_or(1) {
            return Ok(Some("excel_threshold_pending"));
        }
        sqlx::query("update openai_excel_credentials e set suspended=true,quality_pause_owner=$2,
            updated_at=greatest(now(),e.updated_at) from provider_accounts a
            where e.account_id=$1 and a.id=e.account_id and a.enabled and not e.suspended
            and a.responses_upstream='excel'
            and e.upstream_user_id is not distinct from $3 and e.upstream_account_id is not distinct from $4")
            .bind(&config.account_id).bind(&claim.rule.id)
            .bind(&claim.account_identity.0).bind(&claim.account_identity.1)
            .execute(&mut **tx).await.map_err(unavailable)?.rows_affected() == 1
    } else {
        super::policy::update_failure_streak(tx, claim, status, &value).await?;
        if !config.auto_restore {
            return Ok(Some("excel_streak_reset"));
        }
        sqlx::query("update openai_excel_credentials e set suspended=false,quality_pause_owner=null,
            updated_at=greatest(now(),e.updated_at) from provider_accounts a
            where e.account_id=$1 and a.id=e.account_id and a.enabled and a.responses_upstream='excel'
            and e.quality_pause_owner=$2 and e.credential_state='ready'
            and (e.access_token_expires_at is null or e.access_token_expires_at>now())
            and e.upstream_user_id is not distinct from $3 and e.upstream_account_id is not distinct from $4")
            .bind(&config.account_id).bind(&claim.rule.id)
            .bind(&claim.account_identity.0).bind(&claim.account_identity.1)
            .execute(&mut **tx).await.map_err(unavailable)?.rows_affected() == 1
    };
    if !changed {
        return Ok(Some("excel_quality_no_change"));
    }
    let action = if status == "incorrect" {
        "excel_scheduling_paused"
    } else {
        "excel_scheduling_restored"
    };
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
            "quality_rule.excel_action",
            "provider_account",
            &config.account_id,
            vec![action.into()],
        ),
        revision,
    )
    .await
    .map_err(unavailable)?;
    Ok(Some(action))
}
