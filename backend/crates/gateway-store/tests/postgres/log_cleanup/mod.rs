use super::TestDatabase;
use chrono::{Duration, Utc};
use gateway_admin::{
    model::{MutationActor, MutationContext, log_cleanup::*},
    ports::{log_cleanup::LogCleanupStore, store::AdminStoreErrorKind},
};
use gateway_store::postgres::PgLogCleanupStore;

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "cleanup-fixture".into(),
    }
}

async fn request_only(store: &PgLogCleanupStore) -> CleanupPreview {
    let state = store.state().await.unwrap();
    let mut config = state.config;
    config.enabled = false;
    config.files.selected = false;
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
    CleanupPreview {
        revision: state.revision,
        config: state.config,
        cutoff_at: Utc::now(),
    }
}

async fn finish(store: &PgLogCleanupStore) -> CleanupJob {
    for _ in 0..30 {
        store.run_batch().await.unwrap();
        let job = store.state().await.unwrap().job.unwrap();
        if job.status != CleanupJobStatus::Running {
            return job;
        }
    }
    panic!("cleanup failed to finish");
}

#[tokio::test]
async fn log_cleanup_is_bounded_and_preserves_accounts_and_billing() {
    let Some(db) = TestDatabase::create("cleanup_bounds").await else {
        return;
    };
    let store = PgLogCleanupStore::new(db.pool.clone(), None, None);
    let cutoff = Utc::now() - Duration::days(100);
    sqlx::query("insert into provider_accounts (
        id,provider_kind,name,upstream_user_id,authentication_kind,provider_credentials_json,
        has_refresh_token,credential_observed_at,created_at,updated_at
    ) values ('cleanup-account','openai','Fixture','fixture-user','oauth','{}',false,now(),now(),now())")
        .execute(&db.pool).await.unwrap();
    sqlx::query("insert into model_requests (
        id,client_api_key_ref,config_revision,protocol,operation,endpoint,client_transport,
        provider_account_ref,provider_kind,outcome,client_status_code,started_at,deadline_at,completed_at,
        downstream_committed_at,routing_scope,cost_amount,cost_currency,cost_source,diagnostic_trace_json
    ) select 'cleanup-request-'||n,'fixture-key',1,'openai','responses','/v1/responses','http',
        'cleanup-account','openai','succeeded',200,$1,$1+interval '1 minute',$1+interval '1 second',
        $1,'all',0.25,'USD','calculated','{\"fixture\":true}'::jsonb from generate_series(1,505) n")
        .bind(cutoff).execute(&db.pool).await.unwrap();
    sqlx::query(
        "update model_requests set started_at=now(),deadline_at=now()+interval '1 minute',
        completed_at=now(),downstream_committed_at=now() where id='cleanup-request-505'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("insert into model_requests (
        id,client_api_key_ref,config_revision,protocol,operation,endpoint,client_transport,started_at,deadline_at,routing_scope
    ) values ('cleanup-running','fixture-key',1,'openai','responses','/v1/responses','http',$1,$1+interval '1 minute','all')")
        .bind(cutoff).execute(&db.pool).await.unwrap();
    let preview = request_only(&store).await;
    store.run_batch().await.unwrap();
    assert!(
        store.state().await.unwrap().job.is_none(),
        "automatic cleanup starts disabled"
    );
    let job = store.start(preview.clone(), &context()).await.unwrap();
    assert_eq!(
        store.start(preview, &context()).await.unwrap_err().kind(),
        AdminStoreErrorKind::Conflict
    );
    store.run_batch().await.unwrap();
    assert_eq!(store.state().await.unwrap().job.unwrap().removed, 500);
    let finished = finish(&store).await;
    assert_eq!(finished.id, job.id);
    assert_eq!(finished.status, CleanupJobStatus::Succeeded);
    assert_eq!(finished.removed, 504);
    let remaining: Vec<String> = sqlx::query_scalar("select id from model_requests order by id")
        .fetch_all(&db.pool)
        .await
        .unwrap();
    assert_eq!(remaining, ["cleanup-request-505", "cleanup-running"]);
    let total: String = sqlx::query_scalar("select amount::text from account_cumulative_costs where provider_account_ref='cleanup-account'")
        .fetch_one(&db.pool).await.unwrap();
    assert_eq!(total, "126.2500000000");
    let ledger: i64 = sqlx::query_scalar("select count(*) from account_cumulative_cost_entries")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(ledger, 505);
    let account: serde_json::Value = sqlx::query_scalar(
        "select provider_credentials_json from provider_accounts where id='cleanup-account'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(account, serde_json::json!({}));
    let usage = store.footprint().await.unwrap();
    assert!(usage.items[0].bytes.unwrap() > 0);
    assert!(
        usage.items[1].bytes.is_none(),
        "missing file capability is unknown, not zero"
    );
    db.close().await;
}

#[tokio::test]
async fn log_cleanup_rejects_stale_preview_and_serializes_workers() {
    let Some(db) = TestDatabase::create("cleanup_concurrency").await else {
        return;
    };
    let store = PgLogCleanupStore::new(db.pool.clone(), None, None);
    let stale = request_only(&store).await;
    let fresh = request_only(&store).await;
    assert!(store.start(stale, &context()).await.is_err());
    let ctx = context();
    let (a, b) = tokio::join!(store.start(fresh.clone(), &ctx), store.start(fresh, &ctx));
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let mut locked = db.pool.begin().await.unwrap();
    sqlx::query("select 1 from log_cleanup_control for update")
        .execute(&mut *locked)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), store.run_batch())
        .await
        .unwrap()
        .unwrap();
    locked.rollback().await.unwrap();
    let job = store.state().await.unwrap().job.unwrap();
    assert_eq!(job.removed, 0);
    store.cancel(&job.id, &context()).await.unwrap();
    store.run_batch().await.unwrap();
    assert_eq!(
        store.state().await.unwrap().job.unwrap().status,
        CleanupJobStatus::Cancelled
    );
    let mut invalid = request_only(&store).await;
    invalid.config.requests.retention_days = 1;
    assert!(store.start(invalid, &context()).await.is_err());
    db.close().await;
}

#[tokio::test]
async fn log_cleanup_schedule_and_partial_failure_are_observable() {
    let Some(db) = TestDatabase::create("cleanup_schedule").await else {
        return;
    };
    let store = PgLogCleanupStore::new(db.pool.clone(), None, None);
    let state = store.state().await.unwrap();
    let mut config = state.config;
    config.enabled = true;
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
    store.run_batch().await.unwrap();
    assert!(store.state().await.unwrap().job.is_none());
    sqlx::query("update log_cleanup_control set next_run_at=now()-interval '1 minute'")
        .execute(&db.pool)
        .await
        .unwrap();
    let job = finish(&store).await;
    assert!(job.automatic);
    assert_eq!(job.status, CleanupJobStatus::Failed);
    assert_eq!(job.errors, [CleanupCategory::Files]);
    assert!(store.state().await.unwrap().next_run_at.unwrap() > Utc::now());
    let state = store.state().await.unwrap();
    let mut config = state.config;
    config.enabled = false;
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
    assert!(store.state().await.unwrap().next_run_at.is_none());
    db.close().await;
}

#[tokio::test]
async fn log_cleanup_resumes_on_owning_instance_and_auto_off_cancels_only_auto() {
    use gateway_store::request_capture::CaptureManager;
    let Some(db) = TestDatabase::create("cleanup_restart").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (capture, _) = CaptureManager::open(db.pool.clone(), directory.path().into())
        .await
        .unwrap();
    let store = PgLogCleanupStore::new(db.pool.clone(), None, Some(capture.clone()));
    let preview = request_only(&store).await;
    store.start(preview, &context()).await.unwrap();
    drop(store);
    drop(capture);
    let (capture, _) = CaptureManager::open(db.pool.clone(), directory.path().into())
        .await
        .unwrap();
    let resumed = PgLogCleanupStore::new(db.pool.clone(), None, Some(capture));
    assert_eq!(finish(&resumed).await.status, CleanupJobStatus::Succeeded);

    let state = resumed.state().await.unwrap();
    let mut config = state.config;
    config.enabled = true;
    resumed
        .configure(
            CleanupCommand {
                revision: state.revision,
                config,
            },
            &context(),
        )
        .await
        .unwrap();
    sqlx::query("update log_cleanup_control set next_run_at=now()-interval '1 minute'")
        .execute(&db.pool)
        .await
        .unwrap();
    resumed.run_batch().await.unwrap();
    let state = resumed.state().await.unwrap();
    assert_eq!(state.job.unwrap().status, CleanupJobStatus::Running);
    let mut config = state.config;
    config.enabled = false;
    resumed
        .configure(
            CleanupCommand {
                revision: state.revision,
                config,
            },
            &context(),
        )
        .await
        .unwrap();
    resumed.run_batch().await.unwrap();
    assert_eq!(
        resumed.state().await.unwrap().job.unwrap().status,
        CleanupJobStatus::Cancelled
    );
    db.close().await;
}
