//! One durable cleanup job; row locks serialize manual and scheduled maintenance.
mod runner;
mod worker;

use crate::request_capture::CaptureManager;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gateway_admin::{
    model::{MutationContext, log_cleanup::*},
    ports::{
        log_cleanup::{LogCleanupStore, LogFileMaintenance},
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
    },
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use std::sync::Arc;
use tokio::sync::Notify;

pub struct PgLogCleanupStore {
    pool: PgPool,
    files: Option<Arc<dyn LogFileMaintenance>>,
    captures: Option<Arc<CaptureManager>>,
    instance_id: String,
    wakeup: Notify,
}

type StateRow = (i64, Value, Option<DateTime<Utc>>, Option<Value>);

fn unavailable(_: impl std::fmt::Display) -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "log cleanup",
        "日志清理存储暂不可用",
    )
}
fn conflict() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Conflict,
        "log cleanup",
        "清理任务正在运行或配置已变化，请刷新后重试",
    )
}
fn invalid(error: impl std::fmt::Display) -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Invalid,
        "log cleanup",
        error.to_string(),
    )
}

impl PgLogCleanupStore {
    pub fn new(
        pool: PgPool,
        files: Option<Arc<dyn LogFileMaintenance>>,
        captures: Option<Arc<CaptureManager>>,
    ) -> Self {
        let instance_id = captures.as_ref().map_or_else(
            || uuid::Uuid::new_v4().to_string(),
            |c| c.instance_id().to_owned(),
        );
        Self {
            pool,
            files,
            captures,
            instance_id,
            wakeup: Notify::new(),
        }
    }

    async fn decode(
        &self,
        row: StateRow,
        tx: &mut Transaction<'_, Postgres>,
    ) -> AdminStoreResult<CleanupState> {
        let mut config: CleanupConfig = serde_json::from_value(row.1).map_err(unavailable)?;
        // These existing settings remain authoritative for compatibility with old clients.
        let days: (i64, i64) = sqlx::query_as(
            "select usage_retention_days, audit_retention_days from runtime_settings where id=1",
        )
        .fetch_one(&mut **tx)
        .await
        .map_err(unavailable)?;
        // The legacy usage window cannot represent cleanup retention below 31
        // days. Keep those values (including clear-all) owned by cleanup only.
        if config.requests.retention_days >= 31 {
            config.requests.retention_days = u32::try_from(days.0).map_err(unavailable)?;
        }
        if config.audit.retention_days > 0 {
            config.audit.retention_days = u32::try_from(days.1).map_err(unavailable)?;
        }
        let capture_days: i32 = sqlx::query_scalar(
            "select (config->>'retentionDays')::int from request_capture_config where singleton",
        )
        .fetch_one(&mut **tx)
        .await
        .map_err(unavailable)?;
        if config.captures.retention_days > 0 {
            config.captures.retention_days = u32::try_from(capture_days).map_err(unavailable)?;
        }
        Ok(CleanupState {
            revision: row.0,
            config,
            next_run_at: row.2,
            job: row
                .3
                .map(serde_json::from_value)
                .transpose()
                .map_err(unavailable)?,
        })
    }

    async fn locked(&self, tx: &mut Transaction<'_, Postgres>) -> AdminStoreResult<CleanupState> {
        sqlx::query("set local lock_timeout = '1s'")
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
        let row = sqlx::query_as("select revision,config,next_run_at,job from log_cleanup_control where singleton for update")
            .fetch_one(&mut **tx).await.map_err(unavailable)?;
        self.decode(row, tx).await
    }

    async fn save_job(
        tx: &mut Transaction<'_, Postgres>,
        job: &CleanupJob,
    ) -> AdminStoreResult<()> {
        sqlx::query("update log_cleanup_control set job=$1 where singleton")
            .bind(serde_json::to_value(job).map_err(unavailable)?)
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    async fn audit(
        tx: &mut Transaction<'_, Postgres>,
        context: &MutationContext,
        action: &str,
        id: &str,
    ) -> AdminStoreResult<()> {
        super::insert_admin_audit_event(
            tx,
            crate::mutation_audit(context, action, "log_cleanup", id, vec![]),
        )
        .await
        .map_err(unavailable)
    }

    fn new_job(
        &self,
        config: CleanupConfig,
        cutoff_at: DateTime<Utc>,
        automatic: bool,
    ) -> CleanupJob {
        CleanupJob {
            id: uuid::Uuid::new_v4().to_string(),
            instance_id: self.instance_id.clone(),
            automatic,
            config,
            cutoff_at,
            status: CleanupJobStatus::Running,
            category_index: 0,
            removed: 0,
            errors: vec![],
        }
    }

    async fn table_bytes(&self, table: &'static str) -> Option<u64> {
        sqlx::query_scalar::<_, i64>("select pg_total_relation_size($1::regclass)")
            .bind(table)
            .fetch_one(&self.pool)
            .await
            .ok()
            .and_then(|n| u64::try_from(n).ok())
    }
}

#[async_trait]
impl LogCleanupStore for PgLogCleanupStore {
    async fn state(&self) -> AdminStoreResult<CleanupState> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let row = sqlx::query_as(
            "select revision,config,next_run_at,job from log_cleanup_control where singleton",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        let state = self.decode(row, &mut tx).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(state)
    }

    async fn footprint(&self) -> AdminStoreResult<CleanupFootprint> {
        let requests = self.table_bytes("model_requests").await;
        let events = self.table_bytes("ops_events").await;
        let requests = requests.zip(events).map(|(a, b)| a.saturating_add(b));
        let audit = self.table_bytes("admin_audit_events").await;
        let files = match &self.files {
            Some(files) => files.bytes().await.ok(),
            None => None,
        };
        let capture_tables = self
            .table_bytes("request_capture_records")
            .await
            .zip(self.table_bytes("request_capture_tasks").await);
        let capture_files = match &self.captures {
            Some(c) => c.storage_bytes().await.ok(),
            None => None,
        };
        let captures = capture_tables
            .zip(capture_files)
            .map(|((a, b), c)| a.saturating_add(b).saturating_add(c));
        Ok(CleanupFootprint {
            measured_at: Utc::now(),
            items: vec![
                CleanupUsage {
                    category: CleanupCategory::Requests,
                    bytes: requests,
                },
                CleanupUsage {
                    category: CleanupCategory::Files,
                    bytes: files,
                },
                CleanupUsage {
                    category: CleanupCategory::Captures,
                    bytes: captures,
                },
                CleanupUsage {
                    category: CleanupCategory::Audit,
                    bytes: audit,
                },
            ],
        })
    }

    async fn configure(
        &self,
        command: CleanupCommand,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        command.config.validate().map_err(invalid)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let current = self.locked(&mut tx).await?;
        if current.revision != command.revision {
            return Err(conflict());
        }
        // A running job keeps its confirmed snapshot; only disabling auto may cancel it.
        if current
            .job
            .as_ref()
            .is_some_and(|j| j.status == CleanupJobStatus::Running)
            && !(current.config.enabled && !command.config.enabled && {
                let mut unchanged = command.config.clone();
                unchanged.enabled = true;
                unchanged == current.config
            })
        {
            return Err(conflict());
        }
        sqlx::query("update log_cleanup_control set revision=revision+1, config=$1, next_run_at=$2 where singleton")
            .bind(serde_json::to_value(&command.config).map_err(unavailable)?)
            .bind(command.config.next_after(Utc::now()).map_err(invalid)?)
            .execute(&mut *tx).await.map_err(unavailable)?;
        sqlx::query("update runtime_settings
            set usage_retention_days = case when $1 >= 31 then $1 else usage_retention_days end,
                ops_event_retention_days = case when $1 >= 31 then $1 else ops_event_retention_days end,
                audit_retention_days = case when $2 > 0 then $2 else audit_retention_days end
            where id=1")
            .bind(i64::from(command.config.requests.retention_days))
            .bind(i64::from(command.config.audit.retention_days))
            .execute(&mut *tx).await.map_err(unavailable)?;
        super::bump_config_revision_in_transaction(&mut tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("update request_capture_config set config=jsonb_set(config,'{retentionDays}',to_jsonb($1::int)) where singleton")
            .bind(command.config.captures.retention_days as i32).execute(&mut *tx).await.map_err(unavailable)?;
        Self::audit(&mut tx, context, "log_cleanup.configure", "settings").await?;
        tx.commit().await.map_err(unavailable)?;
        self.wakeup.notify_one();
        Ok(())
    }

    async fn start(
        &self,
        preview: CleanupPreview,
        context: &MutationContext,
    ) -> AdminStoreResult<CleanupJob> {
        preview.validate().map_err(invalid)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let current = self.locked(&mut tx).await?;
        if current.revision != preview.revision
            || current.config != preview.config
            || current
                .job
                .as_ref()
                .is_some_and(|j| j.status == CleanupJobStatus::Running)
        {
            return Err(conflict());
        }
        let job = self.new_job(preview.config, preview.cutoff_at, false);
        Self::save_job(&mut tx, &job).await?;
        Self::audit(&mut tx, context, "log_cleanup.start", &job.id).await?;
        tx.commit().await.map_err(unavailable)?;
        self.wakeup.notify_one();
        Ok(job)
    }

    async fn cancel(&self, id: &str, context: &MutationContext) -> AdminStoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let state = self.locked(&mut tx).await?;
        let mut job = state
            .job
            .filter(|j| j.id == id && j.status == CleanupJobStatus::Running)
            .ok_or_else(conflict)?;
        job.status = CleanupJobStatus::Cancelled;
        Self::save_job(&mut tx, &job).await?;
        Self::audit(&mut tx, context, "log_cleanup.cancel", id).await?;
        tx.commit().await.map_err(unavailable)?;
        self.wakeup.notify_one();
        Ok(())
    }
}
