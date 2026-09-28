use super::*;
use std::collections::BTreeMap;

fn template(row: &sqlx::postgres::PgRow) -> AdminStoreResult<QualityRuleTemplate> {
    Ok(QualityRuleTemplate {
        id: row.try_get("id").map_err(unavailable)?,
        revision: row.try_get("revision").map_err(unavailable)?,
        name: row.try_get("name").map_err(unavailable)?,
        config: serde_json::from_value(row.try_get("config").map_err(unavailable)?)
            .map_err(unavailable)?,
    })
}

pub(super) async fn load_locked(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: &str,
) -> AdminStoreResult<Option<QualityRuleTemplate>> {
    sqlx::query("select * from quality_rule_templates where id=$1 for share")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(template)
        .transpose()
}

async fn template_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    context: &MutationContext,
    action: &'static str,
    id: &str,
) -> AdminStoreResult<()> {
    let revision = bump_config_revision_in_transaction(tx)
        .await
        .map_err(unavailable)?;
    append_admin_audit_event_in_transaction(
        tx,
        mutation_audit(
            context,
            action,
            "quality_rule_template",
            id,
            vec!["quality_rule_template".into()],
        ),
        revision,
    )
    .await
    .map_err(unavailable)
}

impl PgQualityOpsStore {
    pub(super) async fn list_rule_templates(&self) -> AdminStoreResult<Vec<QualityRuleTemplate>> {
        sqlx::query("select * from quality_rule_templates order by updated_at desc,id")
            .fetch_all(&self.pool)
            .await
            .map_err(unavailable)?
            .iter()
            .map(template)
            .collect()
    }

    pub(super) async fn load_rule_template(
        &self,
        id: &str,
    ) -> AdminStoreResult<Option<QualityRuleTemplate>> {
        sqlx::query("select * from quality_rule_templates where id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(unavailable)?
            .as_ref()
            .map(template)
            .transpose()
    }

    pub(super) async fn save_rule_template(
        &self,
        id: Option<&str>,
        revision: Option<i64>,
        name: String,
        mut config: QualityRuleConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<QualityRuleTemplate> {
        if !config.account_id.is_empty() {
            return Err(conflict());
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        lock_configuration(&mut tx).await?;
        if config.failure_action == QualityFailureAction::ApplyAccountTemplate {
            config.failure_template = Some(
                template_action::resolve(
                    &mut tx,
                    config.failure_template.as_ref().ok_or_else(conflict)?,
                )
                .await?
                .ok_or_else(conflict)?,
            );
        } else {
            config.failure_template = None;
        }
        let config = serde_json::to_value(config).map_err(unavailable)?;
        let row = if let Some(id) = id {
            sqlx::query("update quality_rule_templates set name=$3,config=$4,revision=revision+1,updated_at=now() where id=$1 and revision=$2 returning *")
                .bind(id).bind(revision).bind(name).bind(config).fetch_optional(&mut *tx).await.map_err(unavailable)?.ok_or_else(conflict)?
        } else {
            sqlx::query(
                "insert into quality_rule_templates(id,name,config) values($1,$2,$3) returning *",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(name)
            .bind(config)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?
        };
        let result = template(&row)?;
        template_audit(&mut tx, context, "quality_rule_template.save", &result.id).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(result)
    }

    pub(super) async fn delete_rule_template(
        &self,
        id: &str,
        revision: i64,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        lock_configuration(&mut tx).await?;
        let changed = sqlx::query("delete from quality_rule_templates where id=$1 and revision=$2")
            .bind(id)
            .bind(revision)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?
            .rows_affected();
        if changed != 1 {
            return Err(conflict());
        }
        template_audit(&mut tx, context, "quality_rule_template.delete", id).await?;
        tx.commit().await.map_err(unavailable)
    }

    pub(crate) async fn account_monitoring(
        &self,
        account_ids: &[String],
    ) -> AdminStoreResult<BTreeMap<String, QualityMonitoring>> {
        if account_ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let rows = sqlx::query("select account_id,id,revision,enabled,pending,next_run_at,last_status,last_run_at,last_action,source_template,coalesce(lease_until>now(),false) as running from quality_rules where account_id=any($1::text[])")
            .bind(account_ids).fetch_all(&self.pool).await.map_err(unavailable)?;
        rows.iter()
            .map(|row| {
                Ok((
                    row.try_get("account_id").map_err(unavailable)?,
                    QualityMonitoring {
                        rule_id: row.try_get("id").map_err(unavailable)?,
                        revision: row.try_get("revision").map_err(unavailable)?,
                        enabled: row.try_get("enabled").map_err(unavailable)?,
                        running: row.try_get("running").map_err(unavailable)?,
                        pending: row.try_get("pending").map_err(unavailable)?,
                        next_run_at: row.try_get("next_run_at").map_err(unavailable)?,
                        last_status: row.try_get("last_status").map_err(unavailable)?,
                        last_run_at: row.try_get("last_run_at").map_err(unavailable)?,
                        last_action: row.try_get("last_action").map_err(unavailable)?,
                        source_template: row
                            .try_get::<Option<serde_json::Value>, _>("source_template")
                            .map_err(unavailable)?
                            .map(serde_json::from_value)
                            .transpose()
                            .map_err(unavailable)?,
                    },
                ))
            })
            .collect()
    }
}
