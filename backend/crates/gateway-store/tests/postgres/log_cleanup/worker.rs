use super::*;
use async_trait::async_trait;
use gateway_admin::ports::{log_cleanup::LogFileMaintenance, store::AdminStoreResult};
use gateway_core::{lifecycle::CancellationToken, task::DaemonTask};
use gateway_store::request_capture::CaptureManager;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

async fn seed_requests(db: &TestDatabase, count: i32) {
    sqlx::query(
        "insert into model_requests (
           id,client_api_key_ref,config_revision,protocol,operation,endpoint,client_transport,
           outcome,client_status_code,started_at,deadline_at,completed_at,routing_scope
         ) select 'continuous-'||n,'fixture-key',1,'openai','responses','/v1/responses','http',
           'succeeded',200,now()-interval '100 days',now()-interval '99 days',
           now()-interval '100 days'+interval '1 second','all' from generate_series(1,$1) n",
    )
    .bind(count)
    .execute(&db.pool)
    .await
    .unwrap();
}

fn spawn_worker(
    store: &Arc<PgLogCleanupStore>,
) -> (
    CancellationToken,
    tokio::task::JoinHandle<Result<(), gateway_core::task::WorkerTaskError>>,
) {
    let cancellation = CancellationToken::new();
    let worker_cancel = cancellation.clone();
    let store = store.clone();
    let handle = tokio::spawn(async move { store.run(worker_cancel).await });
    (cancellation, handle)
}

async fn wait_for_completion(store: &PgLogCleanupStore) -> CleanupJob {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(job) = store.state().await.unwrap().job
                && job.status != CleanupJobStatus::Running
            {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("continuous cleanup completes")
}

#[tokio::test]
async fn manual_and_scheduled_workers_drain_multiple_batches() {
    let Some(db) = TestDatabase::create("cleanup_worker_drain").await else {
        return;
    };
    let store = Arc::new(PgLogCleanupStore::new(db.pool.clone(), None, None));
    request_only(&store).await;
    let (stop, handle) = spawn_worker(&store);

    for automatic in [false, true] {
        seed_requests(&db, 2_501).await;
        let state = store.state().await.unwrap();
        let mut config = state.config;
        config.enabled = automatic;
        store
            .configure(
                CleanupCommand {
                    revision: state.revision,
                    config,
                },
                &context(),
            )
            .await
            .unwrap();
        if automatic {
            // Simulate the due time arriving after configuration, without a local wakeup.
            sqlx::query(
                "update log_cleanup_control set next_run_at=now()-interval '1 second',job=null",
            )
            .execute(&db.pool)
            .await
            .unwrap();
        } else {
            let state = store.state().await.unwrap();
            store
                .start(
                    CleanupPreview {
                        revision: state.revision,
                        config: state.config,
                        cutoff_at: Utc::now(),
                    },
                    &context(),
                )
                .await
                .unwrap();
        }
        let finished = wait_for_completion(&store).await;
        assert_eq!(finished.status, CleanupJobStatus::Succeeded);
        assert_eq!(finished.automatic, automatic);
        assert_eq!(finished.removed, 2_501);
        let remaining: i64 = sqlx::query_scalar("select count(*) from model_requests")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
    }
    stop.cancel();
    handle.await.unwrap().unwrap();
    db.close().await;
}

#[tokio::test]
async fn locked_requests_are_retried_not_reported_complete() {
    let Some(db) = TestDatabase::create("cleanup_locked_requests").await else {
        return;
    };
    let store = Arc::new(PgLogCleanupStore::new(db.pool.clone(), None, None));
    seed_requests(&db, 3).await;
    let preview = request_only(&store).await;
    store.start(preview, &context()).await.unwrap();
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("select 1 from model_requests where id='continuous-1' for update")
        .execute(&mut *lock)
        .await
        .unwrap();
    for _ in 0..2 {
        store.run_batch().await.unwrap();
        let job = store.state().await.unwrap().job.unwrap();
        assert_eq!(job.category_index, 0);
        assert_eq!(job.removed, 2);
        assert!(job.errors.is_empty());
    }
    lock.rollback().await.unwrap();
    let (stop, handle) = spawn_worker(&store);
    let finished = wait_for_completion(&store).await;
    assert_eq!(finished.removed, 3);
    assert_eq!(finished.status, CleanupJobStatus::Succeeded);
    stop.cancel();
    handle.await.unwrap().unwrap();
    db.close().await;
}

#[tokio::test]
async fn locked_events_and_audit_rows_are_not_skipped_permanently() {
    let Some(db) = TestDatabase::create("cleanup_locked_events").await else {
        return;
    };
    let store = PgLogCleanupStore::new(db.pool.clone(), None, None);
    sqlx::query("insert into ops_events (id,level,component,operation,failure_kind,message,created_at)
        values ('locked-event','error','fixture','fixture','fixture','fixture',now()-interval '100 days')")
        .execute(&db.pool).await.unwrap();
    sqlx::query("insert into admin_audit_events (id,actor_kind,actor_ref,action,entity_kind,entity_ref,created_at)
        values ('locked-audit','system','system','fixture','fixture','fixture',now()-interval '100 days')")
        .execute(&db.pool).await.unwrap();
    let preview = request_only(&store).await;
    let mut config = preview.config;
    config.audit.selected = true;
    store
        .configure(
            CleanupCommand {
                revision: preview.revision,
                config,
            },
            &context(),
        )
        .await
        .unwrap();
    let state = store.state().await.unwrap();
    let preview = CleanupPreview {
        revision: state.revision,
        config: state.config,
        cutoff_at: Utc::now(),
    };
    store.start(preview, &context()).await.unwrap();

    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("select 1 from ops_events where id='locked-event' for update")
        .execute(&mut *lock)
        .await
        .unwrap();
    store.run_batch().await.unwrap();
    assert_eq!(store.state().await.unwrap().job.unwrap().category_index, 0);
    lock.rollback().await.unwrap();

    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("select 1 from admin_audit_events where id='locked-audit' for update")
        .execute(&mut *lock)
        .await
        .unwrap();
    for _ in 0..4 {
        store.run_batch().await.unwrap();
    }
    let job = store.state().await.unwrap().job.unwrap();
    assert_eq!(job.status, CleanupJobStatus::Running);
    assert_eq!(job.category_index, 3);
    assert_eq!(job.removed, 1);
    lock.rollback().await.unwrap();
    let finished = finish(&store).await;
    assert_eq!(finished.removed, 2);
    assert_eq!(finished.status, CleanupJobStatus::Succeeded);
    db.close().await;
}

#[tokio::test]
async fn cascade_lock_timeout_preserves_category_for_retry() {
    let Some(db) = TestDatabase::create("cleanup_cascade_lock").await else {
        return;
    };
    let store = PgLogCleanupStore::new(db.pool.clone(), None, None);
    seed_requests(&db, 1).await;
    sqlx::query("insert into ops_events (id,model_request_id,attempt_index,level,component,operation,failure_kind,message,created_at)
        values ('cascade-event','continuous-1',1,'error','fixture','fixture','fixture','fixture',now()-interval '100 days')")
        .execute(&db.pool).await.unwrap();
    let preview = request_only(&store).await;
    store.start(preview, &context()).await.unwrap();
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("select 1 from ops_events where id='cascade-event' for update")
        .execute(&mut *lock)
        .await
        .unwrap();
    store.run_batch().await.unwrap();
    let job = store.state().await.unwrap().job.unwrap();
    assert_eq!(job.category_index, 0);
    assert_eq!(job.removed, 0);
    assert!(job.errors.is_empty());
    lock.rollback().await.unwrap();
    let finished = finish(&store).await;
    assert_eq!(finished.status, CleanupJobStatus::Succeeded);
    assert_eq!(finished.removed, 1);
    db.close().await;
}

#[tokio::test]
async fn waiting_worker_remains_cancellable_and_other_instance_does_not_take_over() {
    let Some(db) = TestDatabase::create("cleanup_worker_cancel").await else {
        return;
    };
    let store = Arc::new(PgLogCleanupStore::new(db.pool.clone(), None, None));
    seed_requests(&db, 1).await;
    let preview = request_only(&store).await;
    let job = store.start(preview, &context()).await.unwrap();
    let other = PgLogCleanupStore::new(db.pool.clone(), None, None);
    other.run_batch().await.unwrap();
    assert_eq!(store.state().await.unwrap().job.unwrap().removed, 0);
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("select 1 from model_requests for update")
        .execute(&mut *lock)
        .await
        .unwrap();
    let (stop, handle) = spawn_worker(&store);
    tokio::task::yield_now().await;
    store.cancel(&job.id, &context()).await.unwrap();
    lock.rollback().await.unwrap();
    let finished = wait_for_completion(&store).await;
    assert_eq!(finished.status, CleanupJobStatus::Cancelled);
    stop.cancel();
    handle.await.unwrap().unwrap();
    let remaining: i64 = sqlx::query_scalar("select count(*) from model_requests")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(remaining, 1);
    db.close().await;
}

#[derive(Default)]
struct PausedFiles {
    entered: Notify,
    release: Notify,
    calls: AtomicUsize,
}

#[async_trait]
impl LogFileMaintenance for PausedFiles {
    async fn bytes(&self) -> AdminStoreResult<u64> {
        Ok(0)
    }

    async fn clean(
        &self,
        _cutoff: chrono::DateTime<Utc>,
        limit: usize,
    ) -> AdminStoreResult<CleanupBatch> {
        assert_eq!(limit, 100);
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(CleanupBatch {
            removed: if call == 0 { 100 } else { 1 },
            complete: call > 0,
        })
    }
}

#[tokio::test]
async fn shutdown_commits_file_progress_and_restart_continues_same_job() {
    let Some(db) = TestDatabase::create("cleanup_worker_restart").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (capture, _) = CaptureManager::open(db.pool.clone(), directory.path().into())
        .await
        .unwrap();
    let files = Arc::new(PausedFiles::default());
    let store = Arc::new(PgLogCleanupStore::new(
        db.pool.clone(),
        Some(files.clone()),
        Some(capture.clone()),
    ));
    let state = store.state().await.unwrap();
    let mut config = state.config;
    config.requests.selected = false;
    config.files.selected = true;
    config.captures.selected = false;
    config.audit.selected = false;
    store
        .configure(
            CleanupCommand {
                revision: state.revision,
                config,
            },
            &context(),
        )
        .await
        .unwrap();
    let state = store.state().await.unwrap();
    let job = store
        .start(
            CleanupPreview {
                revision: state.revision,
                config: state.config,
                cutoff_at: Utc::now(),
            },
            &context(),
        )
        .await
        .unwrap();
    let (stop, handle) = spawn_worker(&store);
    tokio::time::timeout(std::time::Duration::from_secs(5), files.entered.notified())
        .await
        .unwrap();
    stop.cancel();
    files.release.notify_one();
    handle.await.unwrap().unwrap();
    let progress = store.state().await.unwrap().job.unwrap();
    assert_eq!(progress.status, CleanupJobStatus::Running);
    assert_eq!(progress.removed, 100);
    assert_eq!(files.calls.load(Ordering::SeqCst), 1);
    drop(store);
    drop(capture);

    let (capture, _) = CaptureManager::open(db.pool.clone(), directory.path().into())
        .await
        .unwrap();
    let resumed = Arc::new(PgLogCleanupStore::new(
        db.pool.clone(),
        Some(files),
        Some(capture),
    ));
    let (stop, handle) = spawn_worker(&resumed);
    let finished = wait_for_completion(&resumed).await;
    assert_eq!(finished.id, job.id);
    assert_eq!(finished.cutoff_at, job.cutoff_at);
    assert_eq!(finished.config, job.config);
    assert_eq!(finished.status, CleanupJobStatus::Succeeded);
    assert_eq!(finished.removed, 101);
    stop.cancel();
    handle.await.unwrap().unwrap();
    db.close().await;
}
