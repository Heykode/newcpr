//! Dedicated login library, fenced independently from managed credentials.

use async_trait::async_trait;
use gateway_admin::model::relogin_templates::{ReloginTemplate, ReloginTemplateConfig};
use gateway_admin::{
    model::relogin::{MAX_ENTRIES, ReloginEntry, ReloginSettings},
    ports::{
        relogin::ReloginStore,
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
    },
};
use sqlx::{PgConnection, PgPool, Row};

pub struct PgReloginStore {
    pool: PgPool,
}
impl PgReloginStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn unavailable(_: impl std::fmt::Display) -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "relogin",
        "storage unavailable",
    )
}
fn conflict() -> AdminStoreError {
    AdminStoreError::new(AdminStoreErrorKind::Conflict, "relogin", "entry changed")
}

// All account creation paths hold the account-egress fence. Settle the saved
// schedule before the pool transaction commits, even if the account is removed
// before the next worker tick. Do not invalidate an in-flight operation's CAS.
pub(super) async fn settle_pool_enrollment(
    connection: &mut PgConnection,
    account_ids: &[String],
) -> crate::StoreResult<()> {
    sqlx::query(
        "with matched as (
           select r.id, r.revision + case when r.material->>'status'='pushing'
               or (r.material->>'enrollment' is null and (r.material->>'status'='running'
               or r.material#>>'{maintenance,view,state}'='running')) then 0 else 1 end as rev,
               r.material->>'enrollment' is not null and r.material->>'status' not in ('pushing','uncertain') as stop_job
           from account_relogin_entries r
           where lower(r.email) in (select lower(email) from provider_accounts where id=any($1))
           and (r.material#>>'{enrollment_plan,state}' = 'scheduled' or
                (r.material#>>'{enrollment_plan,state}'='running' and r.material->>'status'<>'pushing'))
           and exists (select 1 from provider_accounts a where a.id = any($1)
              and a.provider_kind='openai' and a.authentication_kind='oauth'
              and lower(a.email)=lower(r.email)
              and coalesce(r.material#>>'{target,workspace_id}',r.material->>'selected_workspace_id',
                   r.material->>'preferred_workspace_id',a.upstream_account_id) is not distinct from a.upstream_account_id
              and (r.material#>>'{target,user_id}' is null or r.material#>>'{target,user_id}'=a.upstream_user_id)
           ) for update of r
         ) update account_relogin_entries r set revision=m.rev,
           material=jsonb_set(jsonb_set(jsonb_set(r.material,'{revision}',to_jsonb(m.rev)),
              '{enrollment_plan,state}','\"completed_early\"'),'{enrollment_plan,when}','null')
              || case when m.stop_job then '{\"enrollment\":null,\"next_attempt_at\":null,\"status\":\"pending\",\"message\":\"入池安排已完成\"}'::jsonb else '{}'::jsonb end
           from matched m where r.id=m.id",
    )
    .bind(account_ids)
    .execute(connection)
    .await
    .map_err(|err| crate::postgres_unavailable("settle enrollment schedule").with_source(err))?;
    Ok(())
}

async fn save_row(
    connection: &mut PgConnection,
    entry: &ReloginEntry,
    expected: Option<u64>,
    explicit_plan: bool,
) -> AdminStoreResult<()> {
    let material = serde_json::to_value(entry).map_err(unavailable)?;
    let revision = i64::try_from(entry.revision).map_err(unavailable)?;
    let result = if let Some(expected) = expected {
        sqlx::query("update account_relogin_entries set revision=$2, material=case
            when not $6 and material#>>'{enrollment_plan,state}'='completed_early'
            then jsonb_set($3,'{enrollment_plan}',material->'enrollment_plan')
            else $3 end where id=$1 and revision=$4 and email=$5")
            .bind(&entry.id).bind(revision).bind(material)
            .bind(i64::try_from(expected).map_err(unavailable)?).bind(&entry.email).bind(explicit_plan)
            .execute(&mut *connection).await
    } else {
        sqlx::query("insert into account_relogin_entries (id,email,revision,material) values ($1,$2,$3,$4) on conflict do nothing")
            .bind(&entry.id).bind(&entry.email).bind(revision).bind(material)
            .execute(&mut *connection).await
    }.map_err(unavailable)?;
    if result.rows_affected() != 1 {
        return Err(conflict());
    }
    for attempt in entry
        .last_attempt
        .iter()
        .chain(entry.maintenance.last_activity.iter())
    {
        sqlx::query(
            "insert into account_relogin_history (entry_id,id,detail) values ($1,$2,$3)
            on conflict (entry_id,id) do update set detail=excluded.detail",
        )
        .bind(&entry.id)
        .bind(&attempt.id)
        .bind(serde_json::to_value(attempt).map_err(unavailable)?)
        .execute(&mut *connection)
        .await
        .map_err(unavailable)?;
    }
    Ok(())
}

#[async_trait]
impl ReloginStore for PgReloginStore {
    async fn save_enrollment_plan(
        &self,
        entry: &ReloginEntry,
        expected: u64,
        context: &gateway_admin::model::MutationContext,
    ) -> AdminStoreResult<()> {
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        // Serialize the absence check with pool insert/import. A UI snapshot
        // taken before the pool commit must not install a new stale schedule.
        sqlx::query("select id from provider_egress_settings where id=1 for share")
            .execute(&mut *transaction)
            .await
            .map_err(unavailable)?;
        let present: bool = sqlx::query_scalar("select exists(select 1 from provider_accounts
            where provider_kind='openai' and authentication_kind='oauth' and lower(email)=lower($1))")
            .bind(&entry.email).fetch_one(&mut *transaction).await.map_err(unavailable)?;
        if present {
            return Err(conflict());
        }
        save_row(&mut transaction, entry, Some(expected), true).await?;
        super::admin_security_audit::insert_admin_audit_event(
            &mut transaction,
            crate::mutation_audit(
                context,
                "arrange_relogin_enrollment",
                "relogin_entry",
                &entry.id,
                vec!["enrollment_plan".to_owned()],
            ),
        )
        .await
        .map_err(unavailable)?;
        transaction.commit().await.map_err(unavailable)
    }

    async fn record_maintenance(
        &self,
        id: &str,
        action: Option<gateway_admin::model::relogin_maintenance::MaintenanceAction>,
        context: &gateway_admin::model::MutationContext,
    ) -> AdminStoreResult<()> {
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        super::admin_security_audit::insert_admin_audit_event(
            &mut transaction,
            crate::mutation_audit(
                context,
                if action.is_some() {
                    "maintain_relogin_account"
                } else {
                    "update_relogin_material"
                },
                "relogin_entry",
                id,
                action
                    .map(|action| vec![action.label().to_owned()])
                    .unwrap_or_else(|| vec!["password".to_owned(), "mfa_secret".to_owned()]),
            ),
        )
        .await
        .map_err(unavailable)?;
        transaction.commit().await.map_err(unavailable)
    }

    async fn save_attempt(
        &self,
        id: &str,
        revision: u64,
        attempt: &gateway_admin::model::relogin::ReloginAttempt,
    ) -> AdminStoreResult<()> {
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        let detail = serde_json::to_value(attempt).map_err(unavailable)?;
        let result = sqlx::query("update account_relogin_entries set material=jsonb_set(material,'{last_attempt}',$3) where id=$1 and revision=$2 and material->'last_attempt'->>'id'=$4")
            .bind(id).bind(i64::try_from(revision).map_err(unavailable)?).bind(&detail).bind(&attempt.id)
            .execute(&mut *transaction).await.map_err(unavailable)?;
        if result.rows_affected() != 1 {
            return Err(conflict());
        }
        sqlx::query("update account_relogin_history set detail=$3 where entry_id=$1 and id=$2")
            .bind(id)
            .bind(&attempt.id)
            .bind(detail)
            .execute(&mut *transaction)
            .await
            .map_err(unavailable)?;
        transaction.commit().await.map_err(unavailable)
    }

    async fn history(
        &self,
        id: &str,
        before: Option<&str>,
        limit: usize,
    ) -> AdminStoreResult<Vec<gateway_admin::model::relogin::ReloginAttempt>> {
        let rows = sqlx::query("select detail from account_relogin_history where entry_id=$1 and ($2::text is null or id<$2) order by id desc limit $3")
            .bind(id).bind(before).bind(limit.min(51) as i64).fetch_all(&self.pool).await.map_err(unavailable)?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_value(row.try_get("detail").map_err(unavailable)?)
                    .map_err(unavailable)
            })
            .collect()
    }
    async fn record_totp_access(
        &self,
        id: &str,
        context: &gateway_admin::model::MutationContext,
    ) -> AdminStoreResult<()> {
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        super::admin_security_audit::insert_admin_audit_event(
            &mut transaction,
            crate::mutation_audit(context, "view_relogin_2fa", "relogin_entry", id, Vec::new()),
        )
        .await
        .map_err(unavailable)?;
        transaction.commit().await.map_err(unavailable)
    }

    async fn record_export(
        &self,
        ids: &[String],
        format: gateway_admin::model::relogin::ReloginExportFormat,
        context: &gateway_admin::model::MutationContext,
    ) -> AdminStoreResult<()> {
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        let action = match format {
            gateway_admin::model::relogin::ReloginExportFormat::Json => "export_relogin_json",
            gateway_admin::model::relogin::ReloginExportFormat::TwoFa => "export_relogin_2fa",
        };
        for id in ids {
            super::admin_security_audit::insert_admin_audit_event(
                &mut transaction,
                crate::mutation_audit(context, action, "relogin_entry", id, Vec::new()),
            )
            .await
            .map_err(unavailable)?;
        }
        transaction.commit().await.map_err(unavailable)
    }
    async fn templates(&self) -> AdminStoreResult<Vec<ReloginTemplate>> {
        let rows = sqlx::query(
            "select id,revision,config from account_relogin_templates order by lower(name),id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)?;
        rows.into_iter()
            .map(|row| {
                Ok(ReloginTemplate {
                    id: row.try_get("id").map_err(unavailable)?,
                    revision: row.try_get::<i64, _>("revision").map_err(unavailable)? as u64,
                    config: serde_json::from_value(row.try_get("config").map_err(unavailable)?)
                        .map_err(unavailable)?,
                })
            })
            .collect()
    }
    async fn save_template(
        &self,
        template: &ReloginTemplate,
        expected: Option<u64>,
    ) -> AdminStoreResult<()> {
        template.config.settings().map_err(unavailable)?;
        self.validate_template_references(&template.config).await?;
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        // Serialize the bounded catalog check with insert/update/delete.
        sqlx::query("lock table account_relogin_templates in exclusive mode")
            .execute(&mut *transaction)
            .await
            .map_err(unavailable)?;
        if expected.is_none() {
            let count: i64 = sqlx::query_scalar("select count(*) from account_relogin_templates")
                .fetch_one(&mut *transaction)
                .await
                .map_err(unavailable)?;
            if count >= 100 {
                return Err(AdminStoreError::new(
                    AdminStoreErrorKind::Invalid,
                    "relogin templates",
                    "最多保存 100 个模板",
                ));
            }
        }
        let config = serde_json::to_value(&template.config).map_err(unavailable)?;
        let revision = i64::try_from(template.revision).map_err(unavailable)?;
        let result = if let Some(expected) = expected {
            sqlx::query("update account_relogin_templates set revision=$2,name=$3,config=$4 where id=$1 and revision=$5")
                .bind(&template.id).bind(revision).bind(&template.config.name).bind(config)
                .bind(i64::try_from(expected).map_err(unavailable)?)
                .execute(&mut *transaction).await
        } else {
            sqlx::query("insert into account_relogin_templates (id,revision,name,config) values ($1,$2,$3,$4)")
                .bind(&template.id).bind(revision).bind(&template.config.name).bind(config)
                .execute(&mut *transaction).await
        }.map_err(|error| {
            if error.as_database_error().is_some_and(|error| error.is_unique_violation()) {
                conflict()
            } else { unavailable(error) }
        })?;
        if result.rows_affected() != 1 {
            return Err(conflict());
        }
        transaction.commit().await.map_err(unavailable)
    }
    async fn delete_template(&self, id: &str, expected: u64) -> AdminStoreResult<()> {
        let result =
            sqlx::query("delete from account_relogin_templates where id=$1 and revision=$2")
                .bind(id)
                .bind(i64::try_from(expected).map_err(unavailable)?)
                .execute(&self.pool)
                .await
                .map_err(unavailable)?;
        if result.rows_affected() != 1 {
            return Err(conflict());
        }
        Ok(())
    }
    async fn validate_template_references(
        &self,
        config: &ReloginTemplateConfig,
    ) -> AdminStoreResult<()> {
        let count: i64 = sqlx::query_scalar("select count(*) from account_groups where id=any($1)")
            .bind(&config.group_ids)
            .fetch_one(&self.pool)
            .await
            .map_err(unavailable)?;
        if count as usize != config.group_ids.len() {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                "relogin templates",
                "模板中的分组已不存在，请编辑模板",
            ));
        }
        if let Some(Some(id)) = &config.credential_proxy_id {
            let valid: bool = sqlx::query_scalar("select exists(select 1 from outbound_proxies where id=$1 and enabled and last_test_success=true)")
                .bind(id).fetch_one(&self.pool).await.map_err(unavailable)?;
            if !valid {
                return Err(AdminStoreError::new(
                    AdminStoreErrorKind::Invalid,
                    "relogin templates",
                    "凭据维护出口不存在、已停用或未通过测试",
                ));
            }
        }
        if let Some(id) = &config.outbound_proxy_id {
            let valid: bool = sqlx::query_scalar("select exists(select 1 from outbound_proxies where id=$1 and last_test_success=true)")
                .bind(id).fetch_one(&self.pool).await.map_err(unavailable)?;
            if !valid {
                return Err(AdminStoreError::new(
                    AdminStoreErrorKind::Invalid,
                    "relogin templates",
                    "模板中的代理不存在或未通过测试，请编辑模板",
                ));
            }
        }
        Ok(())
    }
    async fn entries(&self) -> AdminStoreResult<Vec<ReloginEntry>> {
        let rows =
            sqlx::query("select material from account_relogin_entries order by email limit $1")
                .bind((MAX_ENTRIES + 1) as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(unavailable)?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_value(row.try_get("material").map_err(unavailable)?)
                    .map_err(unavailable)
            })
            .collect()
    }
    async fn save(&self, entry: &ReloginEntry, expected: Option<u64>) -> AdminStoreResult<()> {
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        save_row(&mut transaction, entry, expected, false).await?;
        transaction.commit().await.map_err(unavailable)
    }
    async fn save_batch(&self, entries: &[(ReloginEntry, Option<u64>)]) -> AdminStoreResult<()> {
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        for (entry, expected) in entries {
            save_row(&mut transaction, entry, *expected, false).await?;
        }
        transaction.commit().await.map_err(unavailable)
    }
    async fn delete(&self, id: &str, expected: u64) -> AdminStoreResult<()> {
        let result = sqlx::query("delete from account_relogin_entries where id=$1 and revision=$2")
            .bind(id)
            .bind(expected as i64)
            .execute(&self.pool)
            .await
            .map_err(unavailable)?;
        if result.rows_affected() != 1 {
            return Err(conflict());
        }
        Ok(())
    }
    async fn settings(&self) -> AdminStoreResult<ReloginSettings> {
        let row =
            sqlx::query("select concurrency,paused,max_retries,retry_interval_seconds from account_relogin_settings where singleton")
                .fetch_one(&self.pool)
                .await
                .map_err(unavailable)?;
        Ok(ReloginSettings {
            concurrency: row.try_get::<i32, _>("concurrency").map_err(unavailable)? as usize,
            paused: row.try_get("paused").map_err(unavailable)?,
            max_retries: row.try_get::<i32, _>("max_retries").map_err(unavailable)? as u32,
            retry_interval_seconds: row
                .try_get::<i32, _>("retry_interval_seconds")
                .map_err(unavailable)? as u32,
        })
    }
    async fn save_settings(&self, settings: &ReloginSettings) -> AdminStoreResult<()> {
        settings.validate().map_err(unavailable)?;
        sqlx::query("update account_relogin_settings set concurrency=$1,paused=$2,max_retries=$3,retry_interval_seconds=$4 where singleton")
            .bind(settings.concurrency as i32)
            .bind(settings.paused)
            .bind(settings.max_retries as i32)
            .bind(settings.retry_interval_seconds as i32)
            .execute(&self.pool)
            .await
            .map_err(unavailable)?;
        Ok(())
    }
}
