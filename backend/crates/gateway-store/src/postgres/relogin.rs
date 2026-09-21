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

async fn save_row(
    connection: &mut PgConnection,
    entry: &ReloginEntry,
    expected: Option<u64>,
) -> AdminStoreResult<()> {
    let material = serde_json::to_value(entry).map_err(unavailable)?;
    let revision = i64::try_from(entry.revision).map_err(unavailable)?;
    let result = if let Some(expected) = expected {
        sqlx::query("update account_relogin_entries set revision=$2, material=$3 where id=$1 and revision=$4 and email=$5")
            .bind(&entry.id).bind(revision).bind(material)
            .bind(i64::try_from(expected).map_err(unavailable)?).bind(&entry.email)
            .execute(connection).await
    } else {
        sqlx::query("insert into account_relogin_entries (id,email,revision,material) values ($1,$2,$3,$4) on conflict do nothing")
            .bind(&entry.id).bind(&entry.email).bind(revision).bind(material)
            .execute(connection).await
    }.map_err(unavailable)?;
    if result.rows_affected() != 1 {
        return Err(conflict());
    }
    Ok(())
}

#[async_trait]
impl ReloginStore for PgReloginStore {
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
        let mut connection = self.pool.acquire().await.map_err(unavailable)?;
        save_row(&mut connection, entry, expected).await
    }
    async fn save_batch(&self, entries: &[(ReloginEntry, Option<u64>)]) -> AdminStoreResult<()> {
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        for (entry, expected) in entries {
            save_row(&mut transaction, entry, *expected).await?;
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
            sqlx::query("select concurrency,paused,max_retries,retry_interval_minutes from account_relogin_settings where singleton")
                .fetch_one(&self.pool)
                .await
                .map_err(unavailable)?;
        Ok(ReloginSettings {
            concurrency: row.try_get::<i32, _>("concurrency").map_err(unavailable)? as usize,
            paused: row.try_get("paused").map_err(unavailable)?,
            max_retries: row.try_get::<i32, _>("max_retries").map_err(unavailable)? as u32,
            retry_interval_minutes: row
                .try_get::<i32, _>("retry_interval_minutes")
                .map_err(unavailable)? as u32,
        })
    }
    async fn save_settings(&self, settings: &ReloginSettings) -> AdminStoreResult<()> {
        settings.validate().map_err(unavailable)?;
        sqlx::query("update account_relogin_settings set concurrency=$1,paused=$2,max_retries=$3,retry_interval_minutes=$4 where singleton")
            .bind(settings.concurrency as i32)
            .bind(settings.paused)
            .bind(settings.max_retries as i32)
            .bind(settings.retry_interval_minutes as i32)
            .execute(&self.pool)
            .await
            .map_err(unavailable)?;
        Ok(())
    }
}
