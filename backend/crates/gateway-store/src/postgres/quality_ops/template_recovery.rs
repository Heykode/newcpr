//! Restore only the configuration owned by one successful template application.

use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, Serialize)]
struct Ownership {
    before: Value,
    after: Value,
    scope: Value,
    identity: (Option<String>, Option<String>),
    model: String,
}

pub(super) async fn snapshot(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account: &str,
) -> AdminStoreResult<Value> {
    // Explicit allowlist: never retain provider tokens, quota observations or leases.
    sqlx::query_scalar(
        "select jsonb_build_object('account',jsonb_build_object(
            'enabled',a.enabled,'concurrency_limit',a.concurrency_limit,'weight',a.weight,
            'turn_state_injection_enabled',a.turn_state_injection_enabled,
            'responses_upstream',a.responses_upstream,'excel_models',a.excel_models,
            'excel_models_follow_global',a.excel_models_follow_global,
            'excel_cache_creation_as_input',a.excel_cache_creation_as_input,
            'excel_ignore_encrypted_content',a.excel_ignore_encrypted_content,
            'request_proxy_source',a.request_proxy_source,
            'excel_auto_disable_on_403',a.excel_auto_disable_on_403,
            'excel_403_action',a.excel_403_action,'model_access_json',a.model_access_json,
            'outbound_proxy_id',a.outbound_proxy_id,'outbound_proxy_url',a.outbound_proxy_url),
         'groups',(select coalesce(jsonb_agg(jsonb_build_object('id',g.account_group_id,
            'created_at',g.created_at) order by g.account_group_id),'[]')
            from account_group_accounts g where g.provider_account_id=a.id),
         'egress',(select o.mode from provider_egress_account_overrides o where o.provider_account_id=a.id),
         'excel_recovery',(select jsonb_build_object('enabled',r.enabled,
            'interval_minutes',r.interval_minutes)
            from account_excel_recovery r where r.account_id=a.id))
         from provider_accounts a where a.id=$1",
    )
    .bind(account)
    .fetch_one(&mut **tx)
    .await
    .map_err(unavailable)
}

pub(super) async fn record(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
    before: Value,
) -> AdminStoreResult<()> {
    let owner = Ownership {
        before,
        after: snapshot(tx, &claim.rule.config.account_id).await?,
        scope: action_scope(tx, &claim.rule.config.account_id).await?,
        identity: claim.account_identity.clone(),
        model: claim.rule.config.model.clone(),
    };
    sqlx::query("update quality_rules set recovery=jsonb_build_object('template_owner',$2::jsonb) where id=$1")
        .bind(&claim.rule.id).bind(serde_json::to_value(owner).map_err(unavailable)?)
        .execute(&mut **tx).await.map_err(unavailable)?;
    Ok(())
}

pub(super) async fn apply(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
    status: &str,
    recovery: &Value,
) -> AdminStoreResult<Option<&'static str>> {
    let Some(value) = recovery.get("template_owner") else {
        // Historical Excel-only ownership cannot reconstruct a full template.
        return Ok(None);
    };
    let owner: Ownership = serde_json::from_value(value.clone()).map_err(unavailable)?;
    sqlx::query("select id from provider_egress_settings where id=1 for update")
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    let safe: bool = sqlx::query_scalar(
        "select credential_state='ready' and quota_access_state<>'exhausted'
         and (access_token_expires_at is null or access_token_expires_at>now())
         and excel_mode_disabled_at is null and excel_auto_disabled_at is null
         and quality_pause_owner is null from provider_accounts where id=$1 for update",
    )
    .bind(&claim.rule.config.account_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(unavailable)?;
    if owner.scope != claim.action_scope
        || owner.identity != claim.account_identity
        || owner.model != claim.rule.config.model
        || owner.after != snapshot(tx, &claim.rule.config.account_id).await?
    {
        release(tx, &claim.rule.id).await?;
        return Ok(Some("template_recovery_released"));
    }
    if status != "correct" || !claim.rule.config.auto_restore {
        return Ok(Some("already_applied"));
    }
    if !safe || !references_exist(tx, &owner.before).await? {
        return Ok(Some("template_restore_blocked"));
    }
    // Restore and egress reconciliation are atomic. Invalidated egress settings
    // must preserve the probe result without a partially restored account.
    sqlx::query("savepoint quality_template_restore")
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    restore(
        tx,
        &claim.rule.config.account_id,
        &owner.before,
        &owner.after,
    )
    .await?;
    let result = super::super::synchronize_account_egress_in_transaction(
        tx,
        std::slice::from_ref(&claim.rule.config.account_id),
    )
    .await;
    if let Err(error) = result {
        sqlx::query("rollback to savepoint quality_template_restore")
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("release savepoint quality_template_restore")
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
        return match error {
            crate::StoreError::Unavailable { .. } => Err(unavailable(error)),
            _ => Ok(Some("template_restore_blocked")),
        };
    }
    sqlx::query("release savepoint quality_template_restore")
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    release(tx, &claim.rule.id).await?;
    let revision = bump_config_revision_in_transaction(tx)
        .await
        .map_err(unavailable)?;
    append_admin_audit_event_in_transaction(
        tx,
        mutation_audit(
            &MutationContext {
                actor: gateway_admin::model::MutationActor::System,
                request_id: claim.run_id.clone(),
            },
            "quality_rule.restore_account_template",
            "provider_account",
            &claim.rule.config.account_id,
            vec!["account_template".into()],
        ),
        revision,
    )
    .await
    .map_err(unavailable)?;
    Ok(Some("template_restored"))
}

async fn references_exist(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    before: &Value,
) -> AdminStoreResult<bool> {
    sqlx::query_scalar(
        "select not exists(select 1 from jsonb_array_elements($1->'groups') g
           where not exists(select 1 from account_groups where id=g->>'id'))
         and ($1->'account'->>'outbound_proxy_id' is null or exists(
           select 1 from outbound_proxies where id=$1->'account'->>'outbound_proxy_id'
           and last_test_success and proxy_url=$1->'account'->>'outbound_proxy_url'))",
    )
    .bind(before)
    .fetch_one(&mut **tx)
    .await
    .map_err(unavailable)
}

async fn restore(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account: &str,
    before: &Value,
    after: &Value,
) -> AdminStoreResult<()> {
    sqlx::query(
        "update provider_accounts a set enabled=b.enabled,concurrency_limit=b.concurrency_limit,
         weight=b.weight,turn_state_injection_enabled=b.turn_state_injection_enabled,
         responses_upstream=b.responses_upstream,excel_models=b.excel_models,
         excel_models_follow_global=b.excel_models_follow_global,
         excel_cache_creation_as_input=b.excel_cache_creation_as_input,
         excel_ignore_encrypted_content=b.excel_ignore_encrypted_content,
         request_proxy_source=b.request_proxy_source,excel_auto_disable_on_403=b.excel_auto_disable_on_403,
         excel_403_action=b.excel_403_action,model_access_json=b.model_access_json,
         outbound_proxy_id=b.outbound_proxy_id,outbound_proxy_url=b.outbound_proxy_url,
         updated_at=greatest(now(),a.updated_at)
         from jsonb_populate_record(null::provider_accounts,$2->'account') b where a.id=$1",
    ).bind(account).bind(before).execute(&mut **tx).await.map_err(unavailable)?;
    sqlx::query("delete from account_group_accounts where provider_account_id=$1")
        .bind(account)
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    sqlx::query("insert into account_group_accounts(provider_account_id,account_group_id,created_at)
        select $1,g->>'id',(g->>'created_at')::timestamptz from jsonb_array_elements($2->'groups') g")
        .bind(account).bind(before).execute(&mut **tx).await.map_err(unavailable)?;
    sqlx::query("delete from provider_egress_account_overrides where provider_account_id=$1")
        .bind(account)
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    sqlx::query(
        "insert into provider_egress_account_overrides(provider_account_id,mode)
        select $1,$2->>'egress' where $2->>'egress' is not null",
    )
    .bind(account)
    .bind(before)
    .execute(&mut **tx)
    .await
    .map_err(unavailable)?;
    if before["excel_recovery"] != after["excel_recovery"] {
        if before["excel_recovery"].is_null() {
            sqlx::query("delete from account_excel_recovery where account_id=$1")
                .bind(account)
                .execute(&mut **tx)
                .await
                .map_err(unavailable)?;
        } else {
            // Advance the generation to invalidate any in-flight recovery probe.
            sqlx::query("update account_excel_recovery set enabled=($2->'excel_recovery'->>'enabled')::boolean,
                interval_minutes=($2->'excel_recovery'->>'interval_minutes')::int,
                generation=generation+1,next_probe_at=now()+make_interval(mins=>($2->'excel_recovery'->>'interval_minutes')::int)
                where account_id=$1")
                .bind(account).bind(before).execute(&mut **tx).await.map_err(unavailable)?;
        }
    }
    Ok(())
}

async fn release(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &str,
) -> AdminStoreResult<()> {
    sqlx::query("update quality_rules set recovery=recovery-'template_owner'-'excel_owner'-'excel_streak'-'excel_pass_streak' where id=$1")
        .bind(rule).execute(&mut **tx).await.map_err(unavailable)?;
    Ok(())
}
