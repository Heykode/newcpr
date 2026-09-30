//! Durable, opt-in paused-account recovery. No raw model response is retained.

use super::*;
use gateway_admin::model::excel_recovery::*;
use std::collections::BTreeMap;

pub(super) async fn update(
    tx: &mut Transaction<'_, Postgres>,
    ids: &[String],
    config: ExcelRecoveryConfig,
) -> StoreResult<()> {
    config.validate().map_err(invalid)?;
    let invalid_target: bool = sqlx::query_scalar("select exists(select 1 from provider_accounts where id=any($1::text[]) and (provider_kind<>'openai' or authentication_kind<>'oauth'))")
        .bind(ids).fetch_one(&mut **tx).await.map_err(|_| postgres_unavailable("validate Excel recovery"))?;
    if invalid_target {
        return Err(invalid("Excel recovery requires OpenAI OAuth accounts"));
    }
    sqlx::query("insert into account_excel_recovery(account_id,enabled,interval_minutes,next_probe_at)
        select id,$2,$3,now()+make_interval(mins=>$3) from provider_accounts where id=any($1::text[])
        on conflict(account_id) do update set enabled=excluded.enabled,interval_minutes=excluded.interval_minutes,
        generation=account_excel_recovery.generation+1,next_probe_at=excluded.next_probe_at")
        .bind(ids).bind(config.enabled).bind(i32::try_from(config.interval_minutes).map_err(|_| invalid("invalid interval"))?)
        .execute(&mut **tx).await.map_err(|_| postgres_unavailable("update Excel recovery"))?;
    Ok(())
}

pub(super) async fn load(
    pool: &PgPool,
    ids: &[String],
) -> StoreResult<BTreeMap<String, ExcelRecoveryView>> {
    let rows = sqlx::query("select r.*,case when last_result='probing' and (lease_until is null or lease_until<=now()) then 'interrupted' else last_result end as display_last_result from account_excel_recovery r where account_id=any($1::text[])")
        .bind(ids)
        .fetch_all(pool)
        .await
        .map_err(|_| postgres_unavailable("load Excel recovery"))?;
    rows.into_iter()
        .map(|row| {
            Ok((
                get(&row, "account_id")?,
                ExcelRecoveryView {
                    config: ExcelRecoveryConfig {
                        enabled: get(&row, "enabled")?,
                        interval_minutes: u32::try_from(get::<i32>(&row, "interval_minutes")?)
                            .map_err(|_| invalid("invalid interval"))?,
                    },
                    next_probe_at: get(&row, "next_probe_at")?,
                    last_probe_at: get(&row, "last_probe_at")?,
                    last_result: get(&row, "display_last_result")?,
                    last_model: get(&row, "last_model")?,
                    recovered_at: get(&row, "recovered_at")?,
                },
            ))
        })
        .collect()
}

async fn scope(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> StoreResult<Option<serde_json::Value>> {
    sqlx::query_scalar("select jsonb_build_array(e.revision,a.credential_revision,a.enabled,a.responses_upstream,
        a.upstream_user_id,a.upstream_account_id,a.excel_403_action,a.excel_auto_disabled_at,
        a.excel_models_follow_global,case when a.excel_models_follow_global then s.excel_default_models else a.excel_models end,
        a.request_proxy_source,a.outbound_proxy_id,p.revision,a.model_access_json,u.mode,u.custom_user_agent,u.updated_at,
        a.excel_cache_creation_as_input,a.excel_ignore_encrypted_content,a.quality_pause_owner,
        (select max(config_revision) from admin_audit_events where entity_kind='provider_account' and entity_ref=a.id),
        case when a.outbound_proxy_id is null then coalesce(o.mode,e.default_mode) else null end)
        from provider_accounts a cross join runtime_settings s cross join provider_egress_settings e
        left join outbound_proxies p on p.id=a.outbound_proxy_id
        left join provider_outbound_user_agents u on u.provider_kind=a.provider_kind
        left join provider_egress_account_overrides o on o.provider_account_id=a.id
        where a.id=$1 and s.id=1 and e.id=1")
        .bind(id).fetch_optional(&mut **tx).await.map_err(|_| postgres_unavailable("read Excel recovery scope"))
}

async fn lock_configuration(tx: &mut Transaction<'_, Postgres>) -> StoreResult<()> {
    sqlx::query("select config_revision from runtime_settings where id=1 for update")
        .execute(&mut **tx)
        .await
        .map_err(|_| postgres_unavailable("lock Excel recovery"))?;
    Ok(())
}

pub(super) async fn claim(pool: &PgPool) -> StoreResult<Option<ExcelRecoveryClaim>> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| postgres_unavailable("begin Excel recovery"))?;
    lock_configuration(&mut tx).await?;
    let slot: Option<i32> = sqlx::query_scalar("select id from excel_recovery_slots where lease_until is null or lease_until<=now() order by id limit 1 for update")
        .fetch_optional(&mut *tx).await.map_err(|_| postgres_unavailable("claim recovery slot"))?;
    let Some(slot) = slot else {
        return Ok(None);
    };
    let row=sqlx::query("select r.account_id,r.generation,a.credential_revision,s.config_revision,
        coalesce(r.failed_model,(case when a.excel_models_follow_global then s.excel_default_models else a.excel_models end)[1]) as model,
        case when a.excel_models_follow_global then s.excel_default_models else a.excel_models end as models
        from account_excel_recovery r join provider_accounts a on a.id=r.account_id cross join runtime_settings s
        where s.id=1 and r.enabled and not a.enabled and a.responses_upstream='excel'
        and a.provider_kind='openai' and a.authentication_kind='oauth' and a.quality_pause_owner is null
        and a.credential_state='ready' and (a.access_token_expires_at is null or a.access_token_expires_at>now())
        and r.next_probe_at<=now() and (r.lease_until is null or r.lease_until<=now())
        order by r.next_probe_at,r.account_id limit 1 for update of a,r skip locked")
        .fetch_optional(&mut *tx).await.map_err(|_| postgres_unavailable("claim Excel recovery"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let id: String = get(&row, "account_id")?;
    let model: Option<String> = get(&row, "model")?;
    let models: Vec<String> = get(&row, "models")?;
    let Some(model) = model.filter(|m| !m.trim().is_empty() && models.contains(m)) else {
        sqlx::query("update account_excel_recovery set last_result='model_unavailable',next_probe_at=now()+make_interval(mins=>interval_minutes) where account_id=$1")
            .bind(&id).execute(&mut *tx).await.map_err(|_| postgres_unavailable("defer recovery model"))?;
        tx.commit()
            .await
            .map_err(|_| postgres_unavailable("commit recovery deferral"))?;
        return Ok(None);
    };
    let claim = ExcelRecoveryClaim {
        account_id: id.clone(),
        generation: get(&row, "generation")?,
        credential_revision: u64::try_from(get::<i64>(&row, "credential_revision")?)
            .map_err(|_| invalid("invalid revision"))?,
        config_revision: u64::try_from(get::<i64>(&row, "config_revision")?)
            .map_err(|_| invalid("invalid revision"))?,
        lease_id: uuid::Uuid::new_v4().to_string(),
        model,
        scope: scope(&mut tx, &id)
            .await?
            .ok_or_else(|| invalid("missing recovery account"))?,
    };
    sqlx::query("update account_excel_recovery set lease_id=$2,lease_until=now()+interval '2 minutes',
        last_probe_at=now(),last_result='probing',last_model=$3,next_probe_at=now()+make_interval(mins=>interval_minutes) where account_id=$1")
        .bind(&id).bind(&claim.lease_id).bind(&claim.model).execute(&mut *tx).await.map_err(|_| postgres_unavailable("persist recovery claim"))?;
    sqlx::query("update excel_recovery_slots set lease_id=$2,lease_until=now()+interval '2 minutes' where id=$1")
        .bind(slot).bind(&claim.lease_id).execute(&mut *tx).await.map_err(|_| postgres_unavailable("reserve recovery slot"))?;
    tx.commit()
        .await
        .map_err(|_| postgres_unavailable("commit recovery claim"))?;
    Ok(Some(claim))
}

async fn matches(
    tx: &mut Transaction<'_, Postgres>,
    claim: &ExcelRecoveryClaim,
) -> StoreResult<bool> {
    let current:bool=sqlx::query_scalar("select exists(select 1 from account_excel_recovery r join provider_accounts a on a.id=r.account_id
        where r.account_id=$1 and r.generation=$2 and r.lease_id=$3 and r.lease_until>now() and r.enabled
        and not a.enabled and a.responses_upstream='excel' and a.quality_pause_owner is null
        and a.credential_state='ready' and (a.access_token_expires_at is null or a.access_token_expires_at>now()))")
        .bind(&claim.account_id).bind(claim.generation).bind(&claim.lease_id).fetch_one(&mut **tx).await.map_err(|_| postgres_unavailable("validate recovery claim"))?;
    Ok(current && scope(tx, &claim.account_id).await?.as_ref() == Some(&claim.scope))
}

pub(super) async fn current(pool: &PgPool, claim: &ExcelRecoveryClaim) -> StoreResult<bool> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| postgres_unavailable("read recovery claim"))?;
    matches(&mut tx, claim).await
}

pub(super) async fn finish(
    pool: &PgPool,
    claim: &ExcelRecoveryClaim,
    outcome: ExcelRecoveryOutcome,
) -> StoreResult<bool> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| postgres_unavailable("finish recovery"))?;
    lock_configuration(&mut tx).await?;
    sqlx::query("select id from provider_accounts where id=$1 for update")
        .bind(&claim.account_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| postgres_unavailable("lock recovery account"))?;
    let valid = matches(&mut tx, claim).await?;
    let restored = valid && outcome == ExcelRecoveryOutcome::Recovered;
    if restored {
        sqlx::query("update provider_accounts set enabled=true,excel_auto_disabled_at=null,updated_at=greatest(now(),updated_at)
            where id=$1").bind(&claim.account_id).execute(&mut *tx).await.map_err(|_| postgres_unavailable("restore Excel scheduling"))?;
        let revision = bump_config_revision_in_transaction(&mut tx).await?;
        append_admin_audit_event_in_transaction(
            &mut tx,
            mutation_audit(
                &MutationContext {
                    actor: gateway_admin::model::MutationActor::System,
                    request_id: claim.lease_id.clone(),
                },
                "account.excel_recovery",
                "provider_account",
                &claim.account_id,
                vec!["enabled".into(), "excel_auto_disabled_at".into()],
            ),
            revision,
        )
        .await?;
    }
    sqlx::query(
        "update account_excel_recovery set lease_id=null,lease_until=null,
        last_result=case when $3 then $4 else 'superseded' end,
        recovered_at=case when $5 then now() else recovered_at end,
        next_probe_at=now()+make_interval(mins=>interval_minutes)
        where account_id=$1 and lease_id=$2",
    )
    .bind(&claim.account_id)
    .bind(&claim.lease_id)
    .bind(valid)
    .bind(outcome.as_str())
    .bind(restored)
    .execute(&mut *tx)
    .await
    .map_err(|_| postgres_unavailable("finish recovery state"))?;
    sqlx::query("update excel_recovery_slots set lease_id=null,lease_until=null where lease_id=$1")
        .bind(&claim.lease_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| postgres_unavailable("release recovery slot"))?;
    tx.commit()
        .await
        .map_err(|_| postgres_unavailable("commit recovery result"))?;
    Ok(restored)
}
