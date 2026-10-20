//! Export business data and global usage from the same repeatable-read snapshot.

use futures::TryStreamExt as _;
use gateway_admin::model::backup::BackupError;
use sqlx::{PgPool, Postgres, Transaction};
use tokio::io::{AsyncWrite, AsyncWriteExt as _};

use super::pg_dump::{pg_dump_failed, write_failed};

/// Keep all business ledgers and their deduplication entries; omit diagnostic history.
const EXCLUDED_DATA: &[&str] = &[
    "model_requests",
    "ops_events",
    "admin_audit_events",
    "request_capture_records",
    "request_capture_tasks",
    "backup_usage_hours",
];

pub(super) struct BusinessSnapshot {
    transaction: Transaction<'static, Postgres>,
    snapshot_id: String,
    schema: String,
}

impl BusinessSnapshot {
    pub async fn begin(pool: &PgPool) -> Result<Self, BackupError> {
        let mut transaction = pool
            .begin()
            .await
            .map_err(|_| pg_dump_failed("无法开始备份快照"))?;
        sqlx::query("set transaction isolation level repeatable read, read only")
            .execute(&mut *transaction)
            .await
            .map_err(|_| pg_dump_failed("无法隔离备份快照"))?;
        sqlx::query("set local statement_timeout = '30s'")
            .execute(&mut *transaction)
            .await
            .map_err(|_| pg_dump_failed("无法设置备份查询期限"))?;
        let (snapshot_id, schema): (String, String) =
            sqlx::query_as("select pg_export_snapshot(), current_schema()")
                .fetch_one(&mut *transaction)
                .await
                .map_err(|_| pg_dump_failed("无法导出一致性快照"))?;
        Ok(Self {
            transaction,
            snapshot_id,
            schema: format!("\"{}\"", schema.replace('"', "\"\"")),
        })
    }

    pub fn configure(&self, command: &mut tokio::process::Command) {
        command
            .arg("--snapshot")
            .arg(&self.snapshot_id)
            .arg("--schema")
            .arg(&self.schema);
        for table in EXCLUDED_DATA {
            command
                .arg("--exclude-table-data")
                .arg(format!("{}.\"{table}\"", self.schema));
        }
    }

    pub fn restore_preamble(&self) -> String {
        // 新库自带 public；只替换空 schema，含已有对象时由 PostgreSQL 拒绝删除
        format!("DROP SCHEMA IF EXISTS {};\n", self.schema)
    }

    pub async fn append(
        mut self,
        writer: &mut (impl AsyncWrite + Unpin),
    ) -> Result<(), BackupError> {
        let header = format!(
            "\n-- CPR business backup: global hourly usage\nCOPY {}.backup_usage_hours (bucket_start,counts,costs) FROM stdin;\n",
            self.schema
        );
        writer
            .write_all(header.as_bytes())
            .await
            .map_err(write_failed)?;
        let sql = crate::postgres::backup_usage_export_sql();
        let mut stream = self
            .transaction
            .copy_out_raw(&sql)
            .await
            .map_err(|_| pg_dump_failed("无法导出用量汇总"))?;
        while let Some(bytes) = stream
            .try_next()
            .await
            .map_err(|_| pg_dump_failed("读取用量汇总失败"))?
        {
            writer.write_all(&bytes).await.map_err(write_failed)?;
        }
        drop(stream);
        let footer = "\\.\n";
        writer
            .write_all(footer.as_bytes())
            .await
            .map_err(write_failed)?;
        self.transaction
            .commit()
            .await
            .map_err(|_| pg_dump_failed("关闭备份快照失败"))
    }
}
