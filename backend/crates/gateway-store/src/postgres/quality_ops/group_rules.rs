use super::*;

fn group(row: &sqlx::postgres::PgRow) -> AdminStoreResult<QualityGroupRule> {
    Ok(QualityGroupRule {
        id: row.try_get("id").map_err(unavailable)?,
        revision: row.try_get("revision").map_err(unavailable)?,
        name: row.try_get("name").map_err(unavailable)?,
        filter: serde_json::from_value(row.try_get("filter").map_err(unavailable)?)
            .map_err(unavailable)?,
        config: serde_json::from_value(row.try_get("config").map_err(unavailable)?)
            .map_err(unavailable)?,
        rule_count: row.try_get("rule_count").map_err(unavailable)?,
        excluded_count: row.try_get("excluded_count").map_err(unavailable)?,
        last_synced_at: row.try_get("last_synced_at").map_err(unavailable)?,
    })
}

pub(super) async fn check_application(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    expected: &QualityGroupRule,
    rule_id: Option<&str>,
    account: &str,
) -> AdminStoreResult<()> {
    let current: Option<(i64, serde_json::Value, serde_json::Value)> = sqlx::query_as(
        "select revision,config,filter from quality_group_rules where id=$1 for share",
    )
    .bind(&expected.id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(unavailable)?;
    if !current.is_some_and(|(revision, config, filter)| {
        revision == expected.revision
            && serde_json::to_value(&expected.config).is_ok_and(|expected| config == expected)
            && serde_json::to_value(&expected.filter).is_ok_and(|expected| filter == expected)
    }) {
        return Err(conflict());
    }
    let membership: Option<(Option<String>, i64)> = sqlx::query_as(
        "select rule_id,applied_revision from quality_group_members where group_rule_id=$1 and account_id=$2")
        .bind(&expected.id).bind(account).fetch_optional(&mut **tx).await.map_err(unavailable)?;
    match (rule_id, membership) {
        (Some(id), Some((Some(owned), applied))) if id == owned && applied < expected.revision => {
            Ok(())
        }
        (None, None) if expected.config.enabled => {
            // Recheck membership under the same configuration fence as account/group edits.
            let eligible: bool = sqlx::query_scalar(
                "select exists(select 1 from provider_accounts a where a.id=$1
                 and provider_kind='openai' and authentication_kind='oauth'
                 and ($2='' or ($2='ungrouped' and not exists(select 1 from account_group_accounts m where m.provider_account_id=a.id))
                 or exists(select 1 from account_group_accounts m where m.provider_account_id=a.id and m.account_group_id=$2)))")
                .bind(account).bind(&expected.filter.group).fetch_one(&mut **tx).await.map_err(unavailable)?;
            if eligible { Ok(()) } else { Err(conflict()) }
        }
        _ => Err(conflict()),
    }
}

impl PgQualityOpsStore {
    pub(super) async fn list_group_rules(&self) -> AdminStoreResult<Vec<QualityGroupRule>> {
        sqlx::query("select g.*,
            (select count(*) from quality_group_members m where m.group_rule_id=g.id and rule_id is not null) as rule_count,
            (select count(*) from quality_group_members m where m.group_rule_id=g.id and rule_id is null) as excluded_count
            from quality_group_rules g order by created_at,id")
            .fetch_all(&self.pool).await.map_err(unavailable)?.iter().map(group).collect()
    }

    pub(super) async fn save_group(
        &self,
        id: Option<&str>,
        revision: Option<i64>,
        name: String,
        filter: QualityGroupFilter,
        mut config: QualityRuleConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<QualityGroupRule> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        lock_configuration(&mut tx).await?;
        let old_config: Option<(serde_json::Value, serde_json::Value)> = sqlx::query_as(
            "select config,filter from quality_group_rules where id=$1 and revision=$2",
        )
        .bind(id)
        .bind(revision)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        let pause_only = !config.enabled
            && old_config.is_some_and(|(mut old, old_filter)| {
                old["enabled"] = serde_json::Value::Bool(false);
                serde_json::to_value(&config).is_ok_and(|new| new == old)
                    && serde_json::to_value(&filter).is_ok_and(|new| new == old_filter)
            });
        if !pause_only && !filter.group.is_empty() && filter.group != "ungrouped" {
            let exists: bool =
                sqlx::query_scalar("select exists(select 1 from account_groups where id=$1)")
                    .bind(&filter.group)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(unavailable)?;
            if !exists {
                return Err(conflict());
            }
        }
        if let Some(selected) = config.failure_template.as_ref().filter(|_| !pause_only) {
            config.failure_template = Some(
                template_action::resolve(&mut tx, selected)
                    .await?
                    .ok_or_else(conflict)?,
            );
        }
        let filter = serde_json::to_value(filter).map_err(unavailable)?;
        let config = serde_json::to_value(config).map_err(unavailable)?;
        let row = if let Some(id) = id {
            sqlx::query("update quality_group_rules set revision=revision+1,name=$3,filter=$4,config=$5,updated_at=now()
                where id=$1 and revision=$2 returning *,0::bigint as rule_count,0::bigint as excluded_count")
                .bind(id).bind(revision).bind(name).bind(filter).bind(config)
                .fetch_optional(&mut *tx).await.map_err(unavailable)?.ok_or_else(conflict)?
        } else {
            sqlx::query(
                "insert into quality_group_rules(id,name,filter,config) values($1,$2,$3,$4)
                returning *,0::bigint as rule_count,0::bigint as excluded_count",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(name)
            .bind(filter)
            .bind(config)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?
        };
        let saved = group(&row)?;
        // Pause atomically, even if a subsequent per-account validation cannot be completed.
        if !saved.config.enabled {
            sqlx::query("update quality_rules q set enabled=false,config=jsonb_set(q.config,'{enabled}','false'),
                pending=false,lease_token=null,lease_until=null,revision=q.revision+1,updated_at=now()
                from quality_group_members m where m.rule_id=q.id and m.group_rule_id=$1")
                .bind(&saved.id).execute(&mut *tx).await.map_err(unavailable)?;
            sqlx::query("update quality_runs set status='cancelled',finished_at=now() where status='running'
                and rule_id in (select rule_id from quality_group_members where group_rule_id=$1)")
                .bind(&saved.id).execute(&mut *tx).await.map_err(unavailable)?;
        }
        audit(&mut tx, context, "quality_group.save", &saved.id).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(saved)
    }

    pub(super) async fn group_application_superseded(
        &self,
        group: &QualityGroupRule,
        target: &QualityTemplateTarget,
    ) -> AdminStoreResult<bool> {
        // Expected races are no-ops; failed validation of a still-pending target is not success.
        sqlx::query_scalar(
            "select not exists(select 1 from quality_group_rules g where g.id=$1 and g.revision=$2
             and exists(select 1 from provider_accounts where id=$3)
             and (($4::text is null and (g.config->>'enabled')::boolean
               and exists(select 1 from provider_accounts a where a.id=$3
                 and provider_kind='openai' and authentication_kind='oauth'
                 and (g.filter->>'group'='' or (g.filter->>'group'='ungrouped'
                   and not exists(select 1 from account_group_accounts m where m.provider_account_id=a.id))
                   or exists(select 1 from account_group_accounts m where m.provider_account_id=a.id and m.account_group_id=g.filter->>'group')))
               and not exists(select 1 from quality_rules where account_id=$3)
               and not exists(select 1 from quality_group_members where group_rule_id=g.id and account_id=$3))
             or ($4::text is not null and exists(select 1 from quality_group_members m
               where m.group_rule_id=g.id and m.account_id=$3 and m.rule_id=$4 and m.applied_revision<g.revision))))",
        )
        .bind(&group.id)
        .bind(group.revision)
        .bind(&target.account_id)
        .bind(&target.rule_id)
        .fetch_one(&self.pool)
        .await
        .map_err(unavailable)
    }

    pub(super) async fn delete_group(
        &self,
        id: &str,
        revision: i64,
        delete_rules: bool,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        lock_configuration(&mut tx).await?;
        let found: Option<String> = sqlx::query_scalar(
            "select id from quality_group_rules where id=$1 and revision=$2 for update",
        )
        .bind(id)
        .bind(revision)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if found.is_none() {
            return Err(conflict());
        }
        if delete_rules {
            sqlx::query("delete from quality_rules where id in (select rule_id from quality_group_members where group_rule_id=$1)")
                .bind(id).execute(&mut *tx).await.map_err(unavailable)?;
        }
        sqlx::query("delete from quality_group_rules where id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        audit(&mut tx, context, "quality_group.delete", id).await?;
        tx.commit().await.map_err(unavailable)
    }

    pub(super) async fn list_group_targets(
        &self,
        id: &str,
        after: &str,
    ) -> AdminStoreResult<Vec<QualityTemplateTarget>> {
        let rows = sqlx::query("select m.account_id,q.id,q.revision from quality_group_members m
            join quality_rules q on q.id=m.rule_id join quality_group_rules g on g.id=m.group_rule_id
            where m.group_rule_id=$1 and m.account_id>$2 and m.applied_revision<g.revision order by m.account_id limit 100")
            .bind(id).bind(after).fetch_all(&self.pool).await.map_err(unavailable)?;
        rows.iter()
            .map(|row| {
                Ok(QualityTemplateTarget {
                    account_id: row.try_get("account_id").map_err(unavailable)?,
                    rule_id: row.try_get("id").map_err(unavailable)?,
                    revision: row.try_get("revision").map_err(unavailable)?,
                })
            })
            .collect()
    }
}
