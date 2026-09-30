use super::*;
use chrono::DateTime;
use gateway_admin::model::log_cleanup::CleanupBatch;

impl CaptureManager {
    pub(crate) fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub(crate) async fn storage_bytes(&self) -> AdminStoreResult<u64> {
        if self.shared.fault.load(Ordering::Acquire) {
            return Err(unavailable("storage"));
        }
        let mut entries = tokio::fs::read_dir(&self.directory)
            .await
            .map_err(unavailable)?;
        let mut total = 0_u64;
        let started = std::time::Instant::now();
        while let Some(entry) = entries.next_entry().await.map_err(unavailable)? {
            if started.elapsed() >= Duration::from_secs(5) {
                return Err(unavailable("capture scan budget"));
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name
                .strip_suffix(".jsonl")
                .is_some_and(|id| Uuid::parse_str(id).is_ok())
            {
                continue;
            }
            let metadata = match tokio::fs::symlink_metadata(entry.path()).await {
                Ok(metadata) => metadata,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(unavailable(e)),
            };
            if metadata.file_type().is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    total = total.saturating_add(metadata.blocks().saturating_mul(512));
                }
                #[cfg(not(unix))]
                {
                    total = total.saturating_add(metadata.len());
                }
            }
        }
        Ok(total)
    }

    pub(crate) async fn clean_expired(
        &self,
        cutoff: DateTime<Utc>,
        limit: i64,
    ) -> AdminStoreResult<CleanupBatch> {
        if self.shared.fault.load(Ordering::Acquire) {
            return Err(unavailable("storage"));
        }
        let _io = tokio::time::timeout(Duration::from_secs(1), self.io.lock())
            .await
            .map_err(unavailable)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("set local statement_timeout='3s'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("set local lock_timeout='500ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        // Global capture is continuous: completed old records may expire without stopping it.
        let ids: Vec<String> = sqlx::query_scalar(
            "delete from request_capture_records where id in (
               select r.id from request_capture_records r join request_capture_tasks t on t.id=r.task_id
               where t.instance_id::text=$1 and r.created_at < $2
                 and (t.task->>'scope'='global' or (t.task->>'status'<>'running' and t.expires_at < $2))
               order by r.created_at limit $3
             ) returning id::text"
        ).bind(&self.instance_id).bind(cutoff).bind(limit).fetch_all(&mut *tx).await.map_err(unavailable)?;
        let removed_tasks = sqlx::query(
            "delete from request_capture_tasks where id in (
               select t.id from request_capture_tasks t
               where t.instance_id::text=$1 and t.expires_at < $2 and t.task->>'status'<>'running'
                 and not exists(select 1 from request_capture_records r where r.task_id=t.id)
               limit $3)",
        )
        .bind(&self.instance_id)
        .bind(cutoff)
        .bind(limit)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?
        .rows_affected();
        tx.commit().await.map_err(unavailable)?;
        for id in &ids {
            let path = self.file(id)?;
            match tokio::fs::symlink_metadata(&path).await {
                Ok(metadata) if metadata.file_type().is_file() => {
                    tokio::fs::remove_file(path).await.map_err(unavailable)?;
                }
                Ok(_) => return Err(unavailable("unexpected capture file")),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(unavailable(e)),
            }
        }
        Ok(CleanupBatch {
            removed: ids.len() as u64 + removed_tasks,
            complete: ids.len() < limit as usize && removed_tasks < limit as u64,
        })
    }
}
