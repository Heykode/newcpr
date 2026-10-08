//! Quality-owned mutations; never clear credentials, quota or independent scheduling intent.

use super::*;
use gateway_admin::model::MutationActor;
use serde::{Deserialize, Serialize};

pub(super) use super::native_recovery::record as record_excel_ownership;

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct Recovery {
    paused: bool,
    groups: Vec<Membership>,
    remaining: Vec<Membership>,
    user: Option<String>,
    workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    overload_streak: Option<super::overload_streak::OverloadStreak>,
}

#[derive(Deserialize, Serialize, PartialEq, Eq)]
struct Membership {
    group_id: String,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize, Serialize)]
struct ExcelStreak {
    count: u8,
    scope: serde_json::Value,
    identity: (Option<String>, Option<String>),
}

pub(super) fn excel_failure_streak(recovery: serde_json::Value) -> AdminStoreResult<u8> {
    recovery
        .get("excel_streak")
        .map(|value| serde_json::from_value::<ExcelStreak>(value.clone()).map(|state| state.count))
        .transpose()
        .map(|count| count.unwrap_or(0))
        .map_err(unavailable)
}

async fn memberships(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account: &str,
) -> AdminStoreResult<Vec<Membership>> {
    sqlx::query(
        "select account_group_id,created_at from account_group_accounts
        where provider_account_id=$1 order by account_group_id",
    )
    .bind(account)
    .fetch_all(&mut **tx)
    .await
    .map_err(unavailable)?
    .iter()
    .map(|row| {
        Ok(Membership {
            group_id: row.try_get("account_group_id").map_err(unavailable)?,
            created_at: row.try_get("created_at").map_err(unavailable)?,
        })
    })
    .collect()
}

pub(super) async fn apply(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
    status: &str,
    overload_trigger: bool,
) -> AdminStoreResult<Option<&'static str>> {
    if status != "correct" && status != "incorrect" {
        return Ok(None);
    }
    let config = &claim.rule.config;
    if matches!(
        config.failure_action,
        QualityFailureAction::EnableExcel | QualityFailureAction::ApplyAccountTemplate
    ) {
        return apply_excel_threshold(tx, claim, status, overload_trigger).await;
    }
    let value: serde_json::Value =
        sqlx::query_scalar("select recovery from quality_rules where id=$1")
            .bind(&claim.rule.id)
            .fetch_one(&mut **tx)
            .await
            .map_err(unavailable)?;
    let mut recovery: Recovery = serde_json::from_value(value.clone()).map_err(unavailable)?;
    let owned = recovery.paused || !recovery.groups.is_empty();
    let cleared_streak = status == "correct" && value.get("excel_streak").is_some();
    if cleared_streak {
        update_failure_streak(tx, claim, status, &value).await?;
    }
    if status == "incorrect" && !owned && config.failure_action == QualityFailureAction::None {
        return Ok(None);
    }
    if status == "correct" && (!owned || !config.auto_restore) {
        return Ok(cleared_streak.then_some("excel_streak_reset"));
    }
    let account = &config.account_id;
    let row = sqlx::query(
        "select enabled,quality_pause_owner,upstream_user_id,upstream_account_id,
        credential_state,access_token_expires_at from provider_accounts where id=$1 for update",
    )
    .bind(account)
    .fetch_optional(&mut **tx)
    .await
    .map_err(unavailable)?;
    let Some(row) = row else {
        return Ok(Some("restore_blocked"));
    };
    let user: Option<String> = row.try_get("upstream_user_id").map_err(unavailable)?;
    let workspace: Option<String> = row.try_get("upstream_account_id").map_err(unavailable)?;
    if (user.as_ref(), workspace.as_ref())
        != (
            claim.account_identity.0.as_ref(),
            claim.account_identity.1.as_ref(),
        )
    {
        return Ok(Some("identity_changed"));
    }
    if status == "incorrect" && owned {
        if recovery.paused {
            let owner: Option<String> = row.try_get("quality_pause_owner").map_err(unavailable)?;
            if owner.as_deref() == Some(&claim.rule.id) {
                return Ok(Some("already_applied"));
            }
            recovery.paused = false;
        }
        if !recovery.groups.is_empty() {
            return Ok(Some("already_applied"));
        }
    }
    let mut changed = false;
    let action;
    if status == "incorrect" {
        if matches!(
            config.failure_action,
            QualityFailureAction::DisableScheduling | QualityFailureAction::RemoveGroups
        ) {
            if action_scope(tx, account).await? != claim.action_scope {
                return Ok(Some("failure_configuration_changed"));
            }
            let count = update_failure_streak(tx, claim, status, &value).await?;
            if !overload_trigger && count < config.failure_threshold.unwrap_or(1) {
                return Ok(Some("excel_threshold_pending"));
            }
        }
        recovery.user = user;
        recovery.workspace = workspace;
        match config.failure_action {
            QualityFailureAction::None => {
                action = "ownership_released";
            }
            QualityFailureAction::DisableScheduling => {
                let result = sqlx::query("update provider_accounts set enabled=false,
                    quality_pause_owner=$2,updated_at=greatest(now(),updated_at) where id=$1 and enabled")
                    .bind(account).bind(&claim.rule.id).execute(&mut **tx).await.map_err(unavailable)?;
                recovery.paused = result.rows_affected() == 1;
                changed = recovery.paused;
                action = if changed {
                    "scheduling_paused"
                } else {
                    "no_change"
                };
            }
            QualityFailureAction::RemoveGroups => {
                let before = memberships(tx, account).await?;
                for membership in before {
                    if config.failure_group_ids.contains(&membership.group_id) {
                        sqlx::query("delete from account_group_accounts where provider_account_id=$1 and account_group_id=$2")
                            .bind(account).bind(&membership.group_id).execute(&mut **tx).await.map_err(unavailable)?;
                        recovery.groups.push(membership);
                    }
                }
                recovery.remaining = memberships(tx, account).await?;
                changed = !recovery.groups.is_empty();
                action = if changed {
                    "groups_removed"
                } else {
                    "no_change"
                };
            }
            QualityFailureAction::EnableExcel | QualityFailureAction::ApplyAccountTemplate => {
                unreachable!("handled before recovery")
            }
        }
    } else {
        let credential: String = row.try_get("credential_state").map_err(unavailable)?;
        let expires: Option<DateTime<Utc>> = row
            .try_get("access_token_expires_at")
            .map_err(unavailable)?;
        if recovery.user != user
            || recovery.workspace != workspace
            || credential != "ready"
            || expires.is_some_and(|at| at <= Utc::now())
        {
            return Ok(Some("restore_blocked"));
        }
        if recovery.paused {
            let owner: Option<String> = row.try_get("quality_pause_owner").map_err(unavailable)?;
            if owner.as_deref() != Some(&claim.rule.id) {
                // A manual scheduling change supersedes us, even if it kept enabled=false.
                recovery.paused = false;
            } else {
                sqlx::query(
                    "update provider_accounts set enabled=true,quality_pause_owner=null,
                    updated_at=greatest(now(),updated_at) where id=$1",
                )
                .bind(account)
                .execute(&mut **tx)
                .await
                .map_err(unavailable)?;
                recovery.paused = false;
                changed = true;
            }
        }
        if !recovery.groups.is_empty() {
            let current = memberships(tx, account).await?;
            // Full account edit forms may rewrite unchanged membership timestamps.
            if !current
                .iter()
                .map(|member| &member.group_id)
                .eq(recovery.remaining.iter().map(|member| &member.group_id))
            {
                return Ok(Some("restore_blocked"));
            }
            let ids = recovery
                .groups
                .iter()
                .map(|member| member.group_id.clone())
                .collect::<Vec<_>>();
            let count: i64 =
                sqlx::query_scalar("select count(*) from account_groups where id=any($1::text[])")
                    .bind(&ids)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(unavailable)?;
            if usize::try_from(count).ok() != Some(ids.len()) {
                return Ok(Some("restore_blocked"));
            }
            for membership in &recovery.groups {
                sqlx::query("insert into account_group_accounts(account_group_id,provider_account_id,created_at)
                    values($1,$2,$3)")
                    .bind(&membership.group_id).bind(account).bind(membership.created_at)
                    .execute(&mut **tx).await.map_err(unavailable)?;
            }
            recovery.groups.clear();
            recovery.remaining.clear();
            changed = true;
        }
        action = if changed {
            "restored"
        } else {
            "ownership_released"
        };
    }
    sqlx::query("update quality_rules set recovery=$2 where id=$1")
        .bind(&claim.rule.id)
        .bind(serde_json::to_value(recovery).map_err(unavailable)?)
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    if changed {
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
                "quality_rule.account_action",
                "provider_account",
                account,
                vec![action.to_owned()],
            ),
            revision,
        )
        .await
        .map_err(unavailable)?;
    }
    Ok(Some(action))
}

async fn apply_excel_threshold(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
    status: &str,
    overload_trigger: bool,
) -> AdminStoreResult<Option<&'static str>> {
    if action_scope(tx, &claim.rule.config.account_id).await? != claim.action_scope {
        return Ok(Some("excel_blocked_configuration_changed"));
    }
    let value: serde_json::Value =
        sqlx::query_scalar("select recovery from quality_rules where id=$1")
            .bind(&claim.rule.id)
            .fetch_one(&mut **tx)
            .await
            .map_err(unavailable)?;
    if claim.rule.config.failure_action == QualityFailureAction::ApplyAccountTemplate {
        if let Some(action) = super::template_recovery::apply(tx, claim, status, &value).await? {
            return Ok(Some(action));
        }
    } else if let Some(action) = native_recovery::apply(tx, claim, status, &value).await? {
        return Ok(Some(action));
    }
    if status == "correct" {
        if value.get("excel_streak").is_none() {
            return Ok(None);
        }
        update_failure_streak(tx, claim, status, &value).await?;
        return Ok(Some("excel_streak_reset"));
    }
    let count = update_failure_streak(tx, claim, status, &value).await?;
    if !overload_trigger && count < claim.rule.config.excel_failure_threshold {
        return Ok(Some("excel_threshold_pending"));
    }
    if claim.rule.config.failure_action == QualityFailureAction::ApplyAccountTemplate {
        super::template_action::apply(tx, claim).await.map(Some)
    } else {
        enable_excel(tx, claim).await.map(Some)
    }
}

async fn update_failure_streak(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
    status: &str,
    value: &serde_json::Value,
) -> AdminStoreResult<u8> {
    if status == "correct" {
        sqlx::query("update quality_rules set recovery=recovery-'excel_streak' where id=$1")
            .bind(&claim.rule.id)
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
        return Ok(0);
    }
    let previous = value
        .get("excel_streak")
        .map(|value| serde_json::from_value::<ExcelStreak>(value.clone()))
        .transpose()
        .map_err(unavailable)?;
    // A changed account identity or outbound policy starts a new evidence sequence.
    let count = previous
        .filter(|state| {
            state.scope == claim.action_scope && state.identity == claim.account_identity
        })
        .map_or(0, |state| state.count)
        .saturating_add(1)
        .min(100);
    let state = ExcelStreak {
        count,
        scope: claim.action_scope.clone(),
        identity: claim.account_identity.clone(),
    };
    sqlx::query(
        "update quality_rules set recovery=jsonb_set(recovery,'{excel_streak}',$2) where id=$1",
    )
    .bind(&claim.rule.id)
    .bind(serde_json::to_value(state).map_err(unavailable)?)
    .execute(&mut **tx)
    .await
    .map_err(unavailable)?;
    Ok(count)
}

async fn enable_excel(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
) -> AdminStoreResult<&'static str> {
    if action_scope(tx, &claim.rule.config.account_id).await? != claim.action_scope {
        return Ok("excel_blocked_configuration_changed");
    }
    let account = &claim.rule.config.account_id;
    let row = sqlx::query(
        "select provider_kind,authentication_kind,enabled,credential_state,quota_access_state,
         access_token_expires_at,upstream_user_id,upstream_account_id,responses_upstream,
         excel_mode_disabled_at,
         case when excel_models_follow_global then
           (select excel_default_models from runtime_settings where id=1) else excel_models end as models
         from provider_accounts where id=$1 for update")
        .bind(account).fetch_optional(&mut **tx).await.map_err(unavailable)?;
    let Some(row) = row else {
        return Ok("identity_changed");
    };
    let identity = (
        row.try_get::<Option<String>, _>("upstream_user_id")
            .map_err(unavailable)?,
        row.try_get::<Option<String>, _>("upstream_account_id")
            .map_err(unavailable)?,
    );
    if identity != claim.account_identity {
        return Ok("identity_changed");
    }
    if row
        .try_get::<String, _>("responses_upstream")
        .map_err(unavailable)?
        == "excel"
    {
        return Ok("excel_already_enabled");
    }
    if row
        .try_get::<Option<DateTime<Utc>>, _>("excel_mode_disabled_at")
        .map_err(unavailable)?
        .is_some()
    {
        return Ok("excel_blocked_403");
    }
    let models: Vec<String> = row.try_get("models").map_err(unavailable)?;
    if !models.contains(&claim.rule.config.model) {
        return Ok("excel_blocked_model");
    }
    if row
        .try_get::<String, _>("provider_kind")
        .map_err(unavailable)?
        != "openai"
        || row
            .try_get::<String, _>("authentication_kind")
            .map_err(unavailable)?
            != "oauth"
        || !row.try_get::<bool, _>("enabled").map_err(unavailable)?
        || row
            .try_get::<String, _>("credential_state")
            .map_err(unavailable)?
            != "ready"
        || row
            .try_get::<String, _>("quota_access_state")
            .map_err(unavailable)?
            == "exhausted"
        || row
            .try_get::<Option<DateTime<Utc>>, _>("access_token_expires_at")
            .map_err(unavailable)?
            .is_some_and(|expiry| expiry <= Utc::now())
    {
        return Ok("excel_blocked_account");
    }
    sqlx::query(
        "update provider_accounts set responses_upstream='excel',
        updated_at=greatest(now(),updated_at) where id=$1",
    )
    .bind(account)
    .execute(&mut **tx)
    .await
    .map_err(unavailable)?;
    let action = "excel_enabled";
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
            "quality_rule.account_action",
            "provider_account",
            account,
            vec![action.into()],
        ),
        revision,
    )
    .await
    .map_err(unavailable)?;
    record_excel_ownership(tx, claim).await?;
    Ok(action)
}
