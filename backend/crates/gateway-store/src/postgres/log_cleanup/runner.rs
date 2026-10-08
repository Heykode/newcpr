use super::*;
use chrono::Duration;
use sqlx::Acquire as _;

const BATCH: i64 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BatchOutcome {
    Continue,
    Idle,
    Retry,
}

impl PgLogCleanupStore {
    pub async fn run_batch(&self) -> AdminStoreResult<()> {
        self.run_next_batch().await.map(|_| ())
    }

    pub(super) async fn run_next_batch(&self) -> AdminStoreResult<BatchOutcome> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("set local statement_timeout='5s'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("set local lock_timeout='500ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let row: Option<StateRow> = sqlx::query_as(
            "select revision,config,next_run_at,job from log_cleanup_control where singleton for update skip locked"
        ).fetch_optional(&mut *tx).await.map_err(unavailable)?;
        let Some(row) = row else {
            return Ok(BatchOutcome::Retry);
        };
        let running = row
            .3
            .as_ref()
            .is_some_and(|job| job.get("status").and_then(Value::as_str) == Some("running"));
        if !running && !row.2.is_some_and(|at| at <= Utc::now()) {
            return Ok(BatchOutcome::Idle);
        }
        let state = self.decode(row, &mut tx).await?;
        let now = Utc::now();
        let mut job = match state.job {
            Some(job) if job.status == CleanupJobStatus::Running => job,
            _ if state.config.enabled && state.next_run_at.is_some_and(|t| t <= now) => {
                state.config.validate().map_err(invalid)?;
                let job = self.new_job(state.config.clone(), now, true);
                Self::audit(&mut tx, &system_context(), "log_cleanup.scheduled", &job.id).await?;
                sqlx::query("update log_cleanup_control set next_run_at=$1 where singleton")
                    .bind(state.config.next_after(now).map_err(invalid)?)
                    .execute(&mut *tx)
                    .await
                    .map_err(unavailable)?;
                job
            }
            _ => return Ok(BatchOutcome::Idle),
        };
        let mut outcome = BatchOutcome::Continue;
        if job.automatic && !state.config.enabled {
            job.status = CleanupJobStatus::Cancelled;
        } else {
            // Files are node-local. Only the accepting instance processes this job;
            // its persisted capture-directory identity survives ordinary restarts.
            if job.instance_id != self.instance_id {
                return Ok(BatchOutcome::Idle);
            }
            if let Some(category) = CleanupCategory::ALL.get(job.category_index).copied() {
                let selection = job.config.selection(category);
                if !selection.selected {
                    job.category_index += 1;
                } else {
                    let cutoff =
                        job.cutoff_at - Duration::days(i64::from(selection.retention_days));
                    // Failed SQL rolls back to this savepoint before recording the error.
                    let mut batch_tx = tx.begin().await.map_err(unavailable)?;
                    let result = if job.capture_only && category == CleanupCategory::Captures {
                        match &self.captures {
                            Some(captures) => captures
                                .clear_stored(
                                    gateway_admin::model::request_capture::ClearCaptures {
                                        confirmed: true,
                                        cutoff_at: Some(cutoff),
                                    },
                                    &system_context(),
                                )
                                .await
                                .map(|result| CleanupBatch {
                                    removed: result.removed_records,
                                    complete: result.complete,
                                }),
                            None => Err(unavailable("captures")),
                        }
                    } else {
                        self.clean_batch(&mut batch_tx, category, cutoff).await
                    };
                    match result {
                        Ok(batch) => {
                            batch_tx.commit().await.map_err(unavailable)?;
                            job.removed = job.removed.saturating_add(batch.removed);
                            if batch.complete {
                                job.category_index += 1;
                            } else if batch.removed == 0 {
                                outcome = BatchOutcome::Retry;
                            }
                        }
                        Err(error) => {
                            batch_tx.rollback().await.map_err(unavailable)?;
                            if error.kind() == AdminStoreErrorKind::Conflict {
                                outcome = BatchOutcome::Retry;
                            } else {
                                tracing::warn!(category = ?category, "Log cleanup category failed");
                                job.errors.push(category);
                                job.category_index += 1;
                            }
                        }
                    }
                }
            }
            if job.category_index >= CleanupCategory::ALL.len() {
                job.status = if job.errors.is_empty() {
                    CleanupJobStatus::Succeeded
                } else {
                    CleanupJobStatus::Failed
                };
            }
        }
        if job.status != CleanupJobStatus::Running {
            outcome = BatchOutcome::Idle;
            Self::audit(
                &mut tx,
                &system_context(),
                match job.status {
                    CleanupJobStatus::Succeeded => "log_cleanup.completed",
                    CleanupJobStatus::Failed => "log_cleanup.failed",
                    _ => "log_cleanup.cancelled",
                },
                &job.id,
            )
            .await?;
        }
        Self::save_job(&mut tx, &job).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(outcome)
    }

    async fn clean_batch(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        category: CleanupCategory,
        cutoff: DateTime<Utc>,
    ) -> AdminStoreResult<CleanupBatch> {
        match category {
            CleanupCategory::Requests => {
                let requests = sqlx::query(
                    "delete from model_requests where ctid in (
                       select ctid from model_requests where outcome <> 'running' and completed_at < $1
                       order by completed_at limit $2 for update skip locked)"
                ).bind(cutoff).bind(BATCH).execute(&mut **tx).await.map_err(cleanup_query_error)?.rows_affected();
                let events = sqlx::query(
                    "delete from ops_events where ctid in (
                       select ctid from ops_events where model_request_id is null and created_at < $1
                       order by created_at limit $2 for update skip locked)"
                ).bind(cutoff).bind(BATCH).execute(&mut **tx).await.map_err(cleanup_query_error)?.rows_affected();
                // SKIP LOCKED can produce a short batch with eligible rows still present.
                let complete = if requests < BATCH as u64 && events < BATCH as u64 {
                    !sqlx::query_scalar::<_, bool>(
                        "select exists(select 1 from model_requests where outcome <> 'running' and completed_at < $1)
                         or exists(select 1 from ops_events where model_request_id is null and created_at < $1)",
                    )
                    .bind(cutoff)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(cleanup_query_error)?
                } else {
                    false
                };
                Ok(CleanupBatch {
                    removed: requests + events,
                    complete,
                })
            }
            CleanupCategory::Audit => {
                let removed = sqlx::query(
                    "delete from admin_audit_events where ctid in (
                       select ctid from admin_audit_events where created_at < $1
                       order by created_at limit $2 for update skip locked)",
                )
                .bind(cutoff)
                .bind(BATCH)
                .execute(&mut **tx)
                .await
                .map_err(cleanup_query_error)?
                .rows_affected();
                let complete = if removed < BATCH as u64 {
                    !sqlx::query_scalar::<_, bool>(
                        "select exists(select 1 from admin_audit_events where created_at < $1)",
                    )
                    .bind(cutoff)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(cleanup_query_error)?
                } else {
                    false
                };
                Ok(CleanupBatch { removed, complete })
            }
            CleanupCategory::Files => {
                self.files
                    .as_ref()
                    .ok_or_else(|| unavailable("files"))?
                    .clean(cutoff, 100)
                    .await
            }
            CleanupCategory::Captures => {
                self.captures
                    .as_ref()
                    .ok_or_else(|| unavailable("captures"))?
                    .clean_expired(cutoff, 100)
                    .await
            }
        }
    }
}

fn cleanup_query_error(error: sqlx::Error) -> AdminStoreError {
    if error
        .as_database_error()
        .and_then(|error| error.code())
        .as_deref()
        == Some("55P03")
    {
        conflict()
    } else {
        unavailable(error)
    }
}

fn system_context() -> MutationContext {
    MutationContext {
        actor: gateway_admin::model::MutationActor::System,
        request_id: uuid::Uuid::new_v4().to_string(),
    }
}
