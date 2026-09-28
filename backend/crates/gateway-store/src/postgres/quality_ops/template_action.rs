//! Template-backed remediation; no new request path or account mutation policy.

use super::*;
use gateway_admin::model::relogin_templates::ReloginTemplate;
use gateway_core::account::ResponsesUpstream;

pub(super) async fn resolve(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    selected: &ReloginTemplate,
) -> AdminStoreResult<Option<ReloginTemplate>> {
    let row =
        sqlx::query("select revision,config from account_relogin_templates where id=$1 for share")
            .bind(&selected.id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(unavailable)?;
    let Some(row) = row else { return Ok(None) };
    let revision = u64::try_from(row.try_get::<i64, _>("revision").map_err(unavailable)?)
        .map_err(unavailable)?;
    if revision != selected.revision {
        return Ok(None);
    }
    Ok(Some(ReloginTemplate {
        id: selected.id.clone(),
        revision,
        config: serde_json::from_value(row.try_get("config").map_err(unavailable)?)
            .map_err(unavailable)?,
    }))
}

async fn references_valid(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    template: &ReloginTemplate,
) -> AdminStoreResult<bool> {
    let groups: Vec<String> =
        sqlx::query_scalar("select id from account_groups where id=any($1::text[]) for share")
            .bind(&template.config.group_ids)
            .fetch_all(&mut **tx)
            .await
            .map_err(unavailable)?;
    if groups.len() != template.config.group_ids.len() {
        return Ok(false);
    }
    if let Some(id) = &template.config.outbound_proxy_id {
        let success: Option<Option<bool>> = sqlx::query_scalar(
            "select last_test_success from outbound_proxies where id=$1 for share",
        )
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(unavailable)?;
        if success != Some(Some(true)) {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn excel_model_allowed(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    config: &QualityRuleConfig,
    template: &ReloginTemplate,
) -> AdminStoreResult<bool> {
    sqlx::query_scalar(
        "select $2=any(case when $3 is true or
        ($3::boolean is null and $4::text[] is null and a.excel_models_follow_global)
        then s.excel_default_models else coalesce($4::text[],a.excel_models) end)
        from provider_accounts a cross join runtime_settings s where a.id=$1 and s.id=1",
    )
    .bind(&config.account_id)
    .bind(&config.model)
    .bind(template.config.excel_models_follow_global)
    .bind(
        template
            .config
            .excel_models
            .as_ref()
            .map(|models| models.as_slice()),
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(unavailable)
}

pub(super) async fn validate_for_save(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    config: &QualityRuleConfig,
    template: &ReloginTemplate,
) -> AdminStoreResult<()> {
    let invalid =
        |message| AdminStoreError::new(AdminStoreErrorKind::Invalid, "quality template", message);
    template
        .config
        .settings()
        .map_err(|_| invalid("模板配置不合法，请编辑模板"))?;
    if !references_valid(tx, template).await? {
        return Err(invalid("模板中的分组或代理已不可用，请编辑模板"));
    }
    if template.config.responses_upstream == Some(ResponsesUpstream::Excel) {
        let supported: bool = sqlx::query_scalar("select exists(select 1 from provider_accounts where id=$1 and provider_kind='openai' and authentication_kind='oauth')")
            .bind(&config.account_id).fetch_one(&mut **tx).await.map_err(unavailable)?;
        if !supported || !excel_model_allowed(tx, config, template).await? {
            return Err(invalid(
                "模板开启Excel时需要OpenAI OAuth账号，且检测模型必须在应用后的Excel模型范围内",
            ));
        }
    }
    Ok(())
}

pub(super) async fn apply(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &QualityClaim,
) -> AdminStoreResult<&'static str> {
    let Some(selected) = &claim.rule.config.failure_template else {
        return Ok("template_unavailable");
    };
    let Some(template) = resolve(tx, selected).await? else {
        return Ok("template_unavailable");
    };
    if template.config != selected.config {
        return Ok("template_unavailable");
    }
    if !references_valid(tx, &template).await? {
        return Ok("template_blocked_references");
    }
    // Match the normal account mutation order: configuration, egress, account.
    sqlx::query("select id from provider_egress_settings where id=1 for update")
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    let row = sqlx::query(
        "select enabled,credential_state,quota_access_state,access_token_expires_at,
        upstream_user_id,upstream_account_id,responses_upstream,excel_mode_disabled_at,
        excel_auto_disabled_at from provider_accounts where id=$1 for update",
    )
    .bind(&claim.rule.config.account_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(unavailable)?;
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
    if !row.try_get::<bool, _>("enabled").map_err(unavailable)?
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
        || row
            .try_get::<Option<DateTime<Utc>>, _>("excel_auto_disabled_at")
            .map_err(unavailable)?
            .is_some()
    {
        return Ok("template_blocked_account");
    }
    let excel = template.config.responses_upstream.map_or(
        row.try_get::<String, _>("responses_upstream")
            .map_err(unavailable)?
            == "excel",
        |mode| mode == ResponsesUpstream::Excel,
    );
    if excel {
        if row
            .try_get::<Option<DateTime<Utc>>, _>("excel_mode_disabled_at")
            .map_err(unavailable)?
            .is_some()
        {
            return Ok("excel_blocked_403");
        }
        if !excel_model_allowed(tx, &claim.rule.config, &template).await? {
            return Ok("excel_blocked_model");
        }
    }
    // A settings conflict must still persist the quality result, without partial mutations.
    sqlx::query("savepoint quality_template_apply")
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    let result = crate::postgres::provider_accounts::apply_account_template_in_transaction(
        tx,
        &claim.rule.config.account_id,
        &template.config,
    )
    .await;
    if let Err(error) = result {
        sqlx::query("rollback to savepoint quality_template_apply")
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("release savepoint quality_template_apply")
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
        return match error {
            crate::StoreError::Unavailable { .. } => Err(unavailable(error)),
            _ => Ok("template_blocked_settings"),
        };
    }
    sqlx::query("release savepoint quality_template_apply")
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    let pause_probe = excel && claim.rule.config.detection_mode == QualityDetectionMode::StateProbe;
    if pause_probe {
        sqlx::query("update quality_rules set enabled=false,config=jsonb_set(config,'{enabled}','false'),
            revision=revision+1,pending=false,lease_token=null,lease_until=null,updated_at=now() where id=$1")
            .bind(&claim.rule.id).execute(&mut **tx).await.map_err(unavailable)?;
    }
    sqlx::query("update quality_rules set recovery='{}'::jsonb where id=$1")
        .bind(&claim.rule.id)
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
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
            "quality_rule.apply_account_template",
            "provider_account",
            &claim.rule.config.account_id,
            vec![
                "account_template".into(),
                "scheduling".into(),
                "groups".into(),
                "proxy".into(),
                "excel".into(),
                "egress_mode".into(),
            ],
        ),
        revision,
    )
    .await
    .map_err(unavailable)?;
    Ok(if pause_probe {
        "template_applied_probe_paused"
    } else {
        "template_applied"
    })
}
