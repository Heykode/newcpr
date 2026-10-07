//! Leased quality jobs. No credentials or opaque upstream headers are persisted here.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gateway_admin::{
    model::{MutationContext, quality_ops::*},
    ports::{
        quality_ops::QualityOpsStore,
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
    },
};
use sqlx::{PgPool, Row as _};

mod group_rules;
mod native_recovery;
mod policy;
mod rule_templates;
mod template_action;

use crate::{
    mutation_audit,
    postgres::{append_admin_audit_event_in_transaction, bump_config_revision_in_transaction},
};

pub struct PgQualityOpsStore {
    pool: PgPool,
}

enum RuleSource<'a> {
    Independent,
    Template(&'a QualityRuleTemplate),
    Group(&'a QualityGroupRule),
}

impl PgQualityOpsStore {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn unavailable(_: impl std::fmt::Display) -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "quality operations",
        "storage unavailable",
    )
}

fn conflict() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Conflict,
        "quality rule",
        "规则已变化、正在执行或账号已有规则，请刷新",
    )
}

fn rule(row: &sqlx::postgres::PgRow) -> AdminStoreResult<QualityRule> {
    let value: serde_json::Value = row.try_get("config").map_err(unavailable)?;
    Ok(QualityRule {
        id: row.try_get("id").map_err(unavailable)?,
        revision: row.try_get("revision").map_err(unavailable)?,
        config: serde_json::from_value(value).map_err(unavailable)?,
        next_run_at: row.try_get("next_run_at").map_err(unavailable)?,
        running: row
            .try_get::<Option<DateTime<Utc>>, _>("lease_until")
            .map_err(unavailable)?
            .is_some_and(|until| until > Utc::now()),
        pending: row.try_get("pending").map_err(unavailable)?,
        last_status: row.try_get("last_status").map_err(unavailable)?,
        last_run_at: row.try_get("last_run_at").map_err(unavailable)?,
        last_action: row.try_get("last_action").map_err(unavailable)?,
        excel_failure_streak: policy::excel_failure_streak(
            row.try_get::<serde_json::Value, _>("recovery")
                .map_err(unavailable)?,
        )?,
        source_template: row
            .try_get::<Option<serde_json::Value>, _>("source_template")
            .map_err(unavailable)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(unavailable)?,
    })
}

fn run(row: &sqlx::postgres::PgRow, detail: bool) -> AdminStoreResult<QualityRun> {
    let mode: Option<String> = row.try_get("detection_mode").map_err(unavailable)?;
    Ok(QualityRun {
        detection_mode: match mode.as_deref() {
            Some("state_probe") => QualityDetectionMode::StateProbe,
            _ => QualityDetectionMode::Answer,
        },
        config: if detail {
            Some(
                serde_json::from_value(row.try_get("config").map_err(unavailable)?)
                    .map_err(unavailable)?,
            )
        } else {
            None
        },
        id: row.try_get("id").map_err(unavailable)?,
        rule_id: row.try_get("rule_id").map_err(unavailable)?,
        account_id: row.try_get("account_id").map_err(unavailable)?,
        model: row.try_get("model").map_err(unavailable)?,
        status: row.try_get("status").map_err(unavailable)?,
        action: row.try_get("action").map_err(unavailable)?,
        started_at: row.try_get("started_at").map_err(unavailable)?,
        finished_at: row.try_get("finished_at").map_err(unavailable)?,
        correct: u32::try_from(row.try_get::<i32, _>("correct").map_err(unavailable)?)
            .map_err(unavailable)?,
        incorrect: u32::try_from(row.try_get::<i32, _>("incorrect").map_err(unavailable)?)
            .map_err(unavailable)?,
        unknown: u32::try_from(row.try_get::<i32, _>("unknown").map_err(unavailable)?)
            .map_err(unavailable)?,
        request_errors: u32::try_from(
            row.try_get::<i32, _>("request_errors")
                .map_err(unavailable)?,
        )
        .map_err(unavailable)?,
        answers: if detail {
            serde_json::from_value(row.try_get("answers").map_err(unavailable)?)
                .map_err(unavailable)?
        } else {
            Vec::new()
        },
    })
}

async fn audit(
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
            "quality_rule",
            id,
            vec!["quality_rule".to_owned()],
        ),
        revision,
    )
    .await
    .map_err(unavailable)
}

async fn lock_configuration(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> AdminStoreResult<()> {
    sqlx::query("select id from runtime_settings where id=1 for update")
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    Ok(())
}

async fn action_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account: &str,
) -> AdminStoreResult<serde_json::Value> {
    // Fence the account's policy, not unrelated concurrent rule actions.
    sqlx::query_scalar(
        "select jsonb_build_array(a.enabled,a.responses_upstream,a.excel_mode_disabled_at,
         a.upstream_user_id,a.upstream_account_id,
         a.excel_models_follow_global,
         case when a.excel_models_follow_global then s.excel_default_models else a.excel_models end,
         a.outbound_proxy_id,p.revision,a.model_access_json,u.mode,u.custom_user_agent,
         (select max(config_revision) from admin_audit_events
          where entity_kind='provider_account' and entity_ref=a.id),
         case when a.outbound_proxy_id is null then coalesce(o.mode,e.default_mode) else null end)
         from provider_accounts a cross join runtime_settings s
         left join outbound_proxies p on p.id=a.outbound_proxy_id
         left join provider_outbound_user_agents u on u.provider_kind=a.provider_kind
         left join provider_egress_account_overrides o on o.provider_account_id=a.id
         cross join provider_egress_settings e where a.id=$1 and s.id=1 and e.id=1",
    )
    .bind(account)
    .fetch_optional(&mut **tx)
    .await
    .map_err(unavailable)
    .map(|scope| scope.unwrap_or(serde_json::Value::Null))
}

impl PgQualityOpsStore {
    async fn save_checked(
        &self,
        id: Option<&str>,
        revision: Option<i64>,
        mut config: QualityRuleConfig,
        next: DateTime<Utc>,
        context: &MutationContext,
        source: RuleSource<'_>,
    ) -> AdminStoreResult<QualityRule> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        lock_configuration(&mut tx).await?;
        let group = if let RuleSource::Group(expected) = &source {
            group_rules::check_application(&mut tx, expected, id, &config.account_id).await?;
            let account_id = config.account_id;
            config = expected.config.clone();
            config.account_id = account_id;
            Some((expected.id.clone(), expected.revision))
        } else {
            None
        };
        let source = if let RuleSource::Template(expected) = source {
            let current = rule_templates::load_locked(&mut tx, &expected.id)
                .await?
                .ok_or_else(conflict)?;
            if current.revision != expected.revision {
                return Err(conflict());
            }
            let account_id = config.account_id;
            config = current.config;
            config.account_id = account_id;
            Some(
                serde_json::to_value(QualityRuleTemplateRef {
                    id: current.id,
                    revision: current.revision,
                    name: current.name,
                })
                .map_err(unavailable)?,
            )
        } else {
            None
        };
        if config.failure_action == QualityFailureAction::ApplyAccountTemplate {
            let selected = config.failure_template.as_ref().ok_or_else(conflict)?;
            let template = template_action::resolve(&mut tx, selected)
                .await?
                .ok_or_else(|| {
                    AdminStoreError::new(
                        AdminStoreErrorKind::Conflict,
                        "quality template",
                        "模板已删除或版本变化，请重新选择",
                    )
                })?;
            template_action::validate_for_save(&mut tx, &config, &template).await?;
            // The catalog owns the snapshot; never persist client-supplied template fields.
            config.failure_template = Some(template);
        } else {
            config.failure_template = None;
        }
        if config.detection_mode == QualityDetectionMode::StateProbe
            || config.failure_action == QualityFailureAction::EnableExcel
        {
            let valid: bool = sqlx::query_scalar(
                "select exists(select 1 from provider_accounts a where id=$1
                 and provider_kind='openai' and authentication_kind='oauth'
                 and (not $2 or $3=any(case when excel_models_follow_global then
                 (select excel_default_models from runtime_settings where id=1) else excel_models end)))")
                .bind(&config.account_id)
                .bind(config.failure_action == QualityFailureAction::EnableExcel)
                .bind(&config.model).fetch_one(&mut *tx).await.map_err(unavailable)?;
            if !valid {
                return Err(conflict());
            }
        }
        let value = serde_json::to_value(&config).map_err(unavailable)?;
        if config.failure_action == QualityFailureAction::RemoveGroups {
            let count: i64 =
                sqlx::query_scalar("select count(*) from account_groups where id=any($1::text[])")
                    .bind(&config.failure_group_ids)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(unavailable)?;
            if usize::try_from(count).ok() != Some(config.failure_group_ids.len()) {
                return Err(conflict());
            }
        }
        let row = if let Some(id) = id {
            let row = sqlx::query(
                "update quality_rules set config=$3,enabled=$4,
                 next_run_at=case when $4 and not enabled then now() else $5 end,
                 source_template=coalesce($7,source_template),
                 recovery=case when config->>'model' is distinct from $3->>'model'
                    or config->>'detectionMode' is distinct from $3->>'detectionMode'
                    or config->>'failureAction' is distinct from $3->>'failureAction'
                    or config->'failureTemplate' is distinct from $3->'failureTemplate'
                    then recovery-'excel_streak'-'excel_owner'-'excel_pass_streak' else recovery-'excel_streak'-'excel_pass_streak' end,
                 last_action=case when last_action in ('excel_threshold_pending','excel_streak_reset','excel_recovery_counted')
                    then null else last_action end,
                 last_status=case when coalesce(config->>'detectionMode','answer')<>
                    coalesce($3->>'detectionMode','answer') then null else last_status end,
                 last_run_at=case when coalesce(config->>'detectionMode','answer')<>
                    coalesce($3->>'detectionMode','answer') then null else last_run_at end,
                 revision=revision+1,pending=false,lease_token=null,lease_until=null,updated_at=now()
                 where id=$1 and revision=$2 and account_id=$6 returning *")
                .bind(id).bind(revision).bind(value).bind(config.enabled).bind(next)
                .bind(&config.account_id).bind(&source).fetch_optional(&mut *tx).await.map_err(unavailable)?
                .ok_or_else(conflict)?;
            sqlx::query(
                "update quality_runs set status='cancelled',finished_at=now()
                where rule_id=$1 and status='running'",
            )
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            row
        } else {
            sqlx::query(
                "insert into quality_rules(id,account_id,config,enabled,next_run_at,source_template)
                 values($1,$2,$3,$4,case when $4 then now() else $5 end,$6)
                 on conflict(account_id) do nothing returning *",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(&config.account_id)
            .bind(value)
            .bind(config.enabled)
            .bind(next)
            .bind(&source)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
            .ok_or_else(conflict)?
        };
        let result = rule(&row)?;
        if let Some((group_id, revision)) = group {
            sqlx::query("insert into quality_group_members(group_rule_id,account_id,rule_id,applied_revision)
                values($1,$2,$3,$4) on conflict(group_rule_id,account_id)
                do update set applied_revision=excluded.applied_revision")
                .bind(group_id).bind(&result.config.account_id).bind(&result.id).bind(revision)
                .execute(&mut *tx).await.map_err(unavailable)?;
        }
        audit(&mut tx, context, "quality_rule.save", &result.id).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(result)
    }
}

#[async_trait]
impl QualityOpsStore for PgQualityOpsStore {
    async fn group_rules(&self) -> AdminStoreResult<Vec<QualityGroupRule>> {
        self.list_group_rules().await
    }
    async fn save_group_rule(
        &self,
        id: Option<&str>,
        revision: Option<i64>,
        name: String,
        filter: QualityGroupFilter,
        config: QualityRuleConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<QualityGroupRule> {
        self.save_group(id, revision, name, filter, config, context)
            .await
    }
    async fn delete_group_rule(
        &self,
        id: &str,
        revision: i64,
        delete_rules: bool,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        self.delete_group(id, revision, delete_rules, context).await
    }
    async fn group_targets(
        &self,
        id: &str,
        after: &str,
    ) -> AdminStoreResult<Vec<QualityTemplateTarget>> {
        self.list_group_targets(id, after).await
    }
    async fn apply_group_rule(
        &self,
        group: &QualityGroupRule,
        target: &QualityTemplateTarget,
        next: DateTime<Utc>,
        context: &MutationContext,
    ) -> AdminStoreResult<bool> {
        let mut config = group.config.clone();
        config.account_id.clone_from(&target.account_id);
        match self
            .save_checked(
                target.rule_id.as_deref(),
                target.revision,
                config,
                next,
                context,
                RuleSource::Group(group),
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(error)
                if error.kind() == AdminStoreErrorKind::Conflict
                    && self.group_application_superseded(group, target).await? =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
    async fn mark_group_synced(&self, id: &str, revision: i64) -> AdminStoreResult<()> {
        sqlx::query(
            "update quality_group_rules set last_synced_at=now() where id=$1 and revision=$2",
        )
        .bind(id)
        .bind(revision)
        .execute(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(())
    }
    async fn templates(&self) -> AdminStoreResult<Vec<QualityRuleTemplate>> {
        self.list_rule_templates().await
    }
    async fn template(&self, id: &str) -> AdminStoreResult<Option<QualityRuleTemplate>> {
        self.load_rule_template(id).await
    }
    async fn save_template(
        &self,
        id: Option<&str>,
        revision: Option<i64>,
        name: String,
        config: QualityRuleConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<QualityRuleTemplate> {
        self.save_rule_template(id, revision, name, config, context)
            .await
    }
    async fn delete_template(
        &self,
        id: &str,
        revision: i64,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        self.delete_rule_template(id, revision, context).await
    }
    async fn apply_template(
        &self,
        template: &QualityRuleTemplate,
        target: &QualityTemplateTarget,
        next: DateTime<Utc>,
        context: &MutationContext,
    ) -> AdminStoreResult<QualityRule> {
        let mut config = template.config.clone();
        config.account_id.clone_from(&target.account_id);
        self.save_checked(
            target.rule_id.as_deref(),
            target.revision,
            config,
            next,
            context,
            RuleSource::Template(template),
        )
        .await
    }
    async fn monitoring(
        &self,
        account_ids: &[String],
    ) -> AdminStoreResult<std::collections::BTreeMap<String, QualityMonitoring>> {
        self.account_monitoring(account_ids).await
    }
    async fn rules(&self) -> AdminStoreResult<Vec<QualityRule>> {
        sqlx::query("select * from quality_rules order by updated_at desc")
            .fetch_all(&self.pool)
            .await
            .map_err(unavailable)?
            .iter()
            .map(rule)
            .collect()
    }
    async fn save(
        &self,
        id: Option<&str>,
        revision: Option<i64>,
        config: QualityRuleConfig,
        next: DateTime<Utc>,
        context: &MutationContext,
    ) -> AdminStoreResult<QualityRule> {
        self.save_checked(id, revision, config, next, context, RuleSource::Independent)
            .await
    }

    async fn delete(
        &self,
        id: &str,
        revision: i64,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        lock_configuration(&mut tx).await?;
        let result = sqlx::query("delete from quality_rules where id=$1 and revision=$2")
            .bind(id)
            .bind(revision)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        if result.rows_affected() != 1 {
            return Err(conflict());
        }
        audit(&mut tx, context, "quality_rule.delete", id).await?;
        tx.commit().await.map_err(unavailable)
    }

    async fn enqueue(
        &self,
        id: &str,
        revision: i64,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        lock_configuration(&mut tx).await?;
        let result = sqlx::query(
            "update quality_rules set pending=true where id=$1 and revision=$2
             and not pending and (lease_until is null or lease_until<now())",
        )
        .bind(id)
        .bind(revision)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        if result.rows_affected() != 1 {
            return Err(conflict());
        }
        audit(&mut tx, context, "quality_rule.enqueue", id).await?;
        tx.commit().await.map_err(unavailable)
    }

    async fn claim(&self) -> AdminStoreResult<Option<QualityClaim>> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        lock_configuration(&mut tx).await?;
        // Serialize only the short admission transaction, not model requests.
        sqlx::query("select pg_advisory_xact_lock(71632046)")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query(
            "update quality_runs r set status='interrupted',finished_at=now()
            where r.status='running' and exists(select 1 from quality_rules q
              where q.id=r.rule_id and (q.lease_until is null or q.lease_until<now()))",
        )
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        let active: i64 =
            sqlx::query_scalar("select count(*) from quality_rules where lease_until>now()")
                .fetch_one(&mut *tx)
                .await
                .map_err(unavailable)?;
        if active >= QUALITY_MAX_WORKERS {
            tx.commit().await.map_err(unavailable)?;
            return Ok(None);
        }
        let row = sqlx::query(
            "select * from quality_rules where (pending or (enabled and next_run_at<=now()))
             and (lease_until is null or lease_until<now())
             order by pending desc,next_run_at,id limit 1 for update skip locked",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        let Some(row) = row else {
            tx.commit().await.map_err(unavailable)?;
            return Ok(None);
        };
        let rule = rule(&row)?;
        let account_identity = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "select upstream_user_id,upstream_account_id from provider_accounts where id=$1",
        )
        .bind(&rule.config.account_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        let action_scope = action_scope(&mut tx, &rule.config.account_id).await?;
        sqlx::query(
            "update quality_runs set status='interrupted',finished_at=now()
            where rule_id=$1 and status='running'",
        )
        .bind(&rule.id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        let lease_token = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7().to_string();
        sqlx::query("update quality_rules set pending=false,lease_token=$2,lease_until=now()+interval '5 minutes'
            where id=$1")
            .bind(&rule.id).bind(&lease_token).execute(&mut *tx).await.map_err(unavailable)?;
        sqlx::query("insert into quality_runs(id,rule_id,rule_revision,account_id,model,config) values($1,$2,$3,$4,$5,$6)")
            .bind(&run_id).bind(&rule.id).bind(rule.revision).bind(&rule.config.account_id)
            .bind(&rule.config.model)
            .bind(serde_json::to_value(&rule.config).map_err(unavailable)?)
            .execute(&mut *tx).await.map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(Some(QualityClaim {
            action_scope,
            rule,
            run_id,
            lease_token,
            account_identity,
        }))
    }

    async fn current(&self, claim: &QualityClaim) -> AdminStoreResult<bool> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        lock_configuration(&mut tx).await?;
        let result = sqlx::query(
            "update quality_rules set lease_until=now()+interval '5 minutes'
            where id=$1 and revision=$2 and lease_token=$3 and lease_until>now()",
        )
        .bind(&claim.rule.id)
        .bind(claim.rule.revision)
        .bind(&claim.lease_token)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(result.rows_affected() == 1)
    }

    async fn finish(
        &self,
        claim: &QualityClaim,
        next: DateTime<Utc>,
        answers: Vec<QualityAnswer>,
    ) -> AdminStoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        lock_configuration(&mut tx).await?;
        let mut counts = [0_i32; 4];
        for answer in &answers {
            counts[match answer.verdict {
                QualityVerdict::Correct => 0,
                QualityVerdict::Incorrect => 1,
                QualityVerdict::Unknown => 2,
                QualityVerdict::RequestError => 3,
            }] += 1;
        }
        let status = if counts[1] > 0 {
            "incorrect"
        } else if counts[3] > 0 {
            "request_error"
        } else if counts[2] > 0 || answers.len() != usize::from(claim.rule.config.repetitions) {
            "unknown"
        } else {
            "correct"
        };
        let affected = sqlx::query(
            "update quality_rules set lease_token=null,lease_until=null,next_run_at=$4,last_status=$5,last_run_at=now()
             where id=$1 and revision=$2 and lease_token=$3 and lease_until>now()")
            .bind(&claim.rule.id).bind(claim.rule.revision).bind(&claim.lease_token).bind(next).bind(status)
            .execute(&mut *tx).await.map_err(unavailable)?;
        if affected.rows_affected() == 1 {
            let action = policy::apply(&mut tx, claim, status).await?;
            sqlx::query("update quality_rules set last_action=$2 where id=$1")
                .bind(&claim.rule.id)
                .bind(action)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
            sqlx::query(
                "update quality_runs set status=$2,finished_at=now(),correct=$3,incorrect=$4,
                unknown=$5,request_errors=$6,answers=$7,action=$8 where id=$1 and status='running'",
            )
            .bind(&claim.run_id)
            .bind(status)
            .bind(counts[0])
            .bind(counts[1])
            .bind(counts[2])
            .bind(counts[3])
            .bind(serde_json::to_value(answers).map_err(unavailable)?)
            .bind(action)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        }
        tx.commit().await.map_err(unavailable)
    }

    async fn runs(&self, rule_id: &str) -> AdminStoreResult<Vec<QualityRun>> {
        sqlx::query("select id,rule_id,account_id,model,status,started_at,finished_at,correct,incorrect,
            config->>'detectionMode' as detection_mode,
            unknown,request_errors,action from quality_runs where rule_id=$1 order by started_at desc limit 100")
            .bind(rule_id).fetch_all(&self.pool).await.map_err(unavailable)?
            .iter().map(|row| run(row, false)).collect()
    }

    async fn detail(&self, run_id: &str) -> AdminStoreResult<Option<QualityRun>> {
        sqlx::query(
            "select *,config->>'detectionMode' as detection_mode from quality_runs where id=$1",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(|row| run(row, true))
        .transpose()
    }

    async fn cleanup(&self) -> AdminStoreResult<()> {
        sqlx::query("delete from quality_runs where status<>'running' and
            (started_at<now()-interval '7 days' or id in
             (select id from (select id,row_number() over(partition by rule_id order by started_at desc) n
              from quality_runs) ranked where n>200))")
            .execute(&self.pool).await.map_err(unavailable)?;
        Ok(())
    }
}
