//! Capture-only capacity and deletion, serialized by the existing IO lock.

use super::*;
use gateway_admin::model::MutationContext;

const DELETE_BATCH: i64 = 256;

impl CaptureManager {
    pub(super) async fn stored_totals(&self) -> AdminStoreResult<(u64, u64)> {
        let (bytes, count): (i64, i64) = sqlx::query_as(
            "select coalesce(sum((r.record->>'bytes')::bigint),0)::bigint, count(*)::bigint
             from request_capture_records r join request_capture_tasks t on t.id=r.task_id
             where t.instance_id::text=$1",
        )
        .bind(&self.instance_id)
        .fetch_one(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok((
            u64::try_from(bytes).map_err(unavailable)?,
            u64::try_from(count).map_err(unavailable)?,
        ))
    }

    async fn oldest_records(
        &self,
        cutoff: chrono::DateTime<Utc>,
    ) -> AdminStoreResult<Vec<(String, i64)>> {
        sqlx::query_as(
            "select r.id::text, (r.record->>'bytes')::bigint
             from request_capture_records r join request_capture_tasks t on t.id=r.task_id
             where t.instance_id::text=$1 and r.created_at <= $2
             order by r.created_at, r.id limit $3",
        )
        .bind(&self.instance_id)
        .bind(cutoff)
        .bind(DELETE_BATCH)
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)
    }

    // Callers own io. Check every path before changing the index; never scan or
    // recursively delete directories, task metadata, usage, or unrelated files.
    async fn remove_records(
        &self,
        records: &[(String, i64)],
        context: Option<&MutationContext>,
    ) -> AdminStoreResult<u64> {
        let mut removed_bytes = 0_u64;
        for (id, bytes) in records {
            removed_bytes =
                removed_bytes.saturating_add(u64::try_from(*bytes).map_err(unavailable)?);
            match tokio::fs::symlink_metadata(self.file(id)?).await {
                Ok(meta) if meta.file_type().is_file() => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(unavailable("unexpected capture file")),
            }
        }
        let ids: Vec<_> = records.iter().map(|(id, _)| id.clone()).collect();
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("set local statement_timeout='3s'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("set local lock_timeout='500ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let removed = sqlx::query(
            "delete from request_capture_records r using request_capture_tasks t
             where r.task_id=t.id and t.instance_id::text=$1 and r.id::text=any($2)",
        )
        .bind(&self.instance_id)
        .bind(&ids)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?
        .rows_affected();
        if removed != ids.len() as u64 {
            return Err(conflict());
        }
        if let Some(context) = context {
            crate::postgres::insert_admin_audit_event(
                &mut tx,
                crate::mutation_audit(
                    context,
                    "request_capture.clear",
                    "request_capture",
                    &self.instance_id,
                    vec!["records".into()],
                ),
            )
            .await
            .map_err(unavailable)?;
        }
        tx.commit().await.map_err(unavailable)?;
        for id in ids {
            match tokio::fs::remove_file(self.file(&id)?).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    self.fault();
                    return Err(unavailable(error));
                }
            }
        }
        Ok(removed_bytes)
    }

    pub(super) async fn make_room(&self, bytes: u64, quota: u64) -> AdminStoreResult<()> {
        if bytes > quota {
            return Err(conflict());
        }
        let (mut stored, _) = self.stored_totals().await?;
        while stored.saturating_add(bytes) > quota {
            let mut records = self.oldest_records(Utc::now()).await?;
            if records.is_empty() {
                return Err(unavailable("capture capacity accounting"));
            }
            let mut freed = 0_u64;
            let excess = stored.saturating_add(bytes).saturating_sub(quota);
            let take = records
                .iter()
                .position(|(_, size)| {
                    freed = freed.saturating_add((*size).max(0) as u64);
                    freed >= excess
                })
                .map_or(records.len(), |index| index + 1);
            records.truncate(take);
            let freed = self.remove_records(&records, None).await?;
            stored = stored.saturating_sub(freed);
        }
        Ok(())
    }

    pub(crate) async fn clear_stored(
        &self,
        input: ClearCaptures,
        context: &MutationContext,
    ) -> AdminStoreResult<CaptureClearResult> {
        input.validate().map_err(unavailable)?;
        let cutoff = input.cutoff_at.unwrap_or_else(Utc::now);
        let _io = tokio::time::timeout(Duration::from_secs(5), self.io.lock())
            .await
            .map_err(unavailable)?;
        if self.shared.fault.load(Ordering::Acquire) {
            return Err(unavailable("storage"));
        }
        let records = self.oldest_records(cutoff).await?;
        let removed_bytes = self.remove_records(&records, Some(context)).await?;
        let complete = records.len() < DELETE_BATCH as usize;
        if complete {
            let (stored, _) = self.stored_totals().await?;
            let resume = {
                let control = self.shared.control.read().map_err(unavailable)?;
                control.config.enabled
                    && control.config.global_errors
                    && stored < u64::from(control.config.quota_mib) * 1024 * 1024
                    && !control
                        .tasks
                        .iter()
                        .any(|task| task.task.scope == CaptureScope::Global && task.accepts())
            };
            if resume {
                self.restore_global_capture().await?;
            }
        }
        Ok(CaptureClearResult {
            cutoff_at: cutoff,
            removed_records: records.len() as u64,
            removed_bytes,
            complete,
        })
    }
}
