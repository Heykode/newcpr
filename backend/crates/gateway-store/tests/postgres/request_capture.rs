use super::TestDatabase;
use futures::StreamExt;
use gateway_admin::{
    model::{MutationActor, MutationContext, request_capture::*},
    ports::request_capture::RequestCaptureStore,
};
use gateway_core::{
    diagnostics::{TraceContext, request_capture::RequestCaptureFactory},
    lifecycle::CancellationToken,
    task::DaemonTask,
};
use gateway_store::request_capture::CaptureManager;
use serde_json::json;
use std::{sync::Arc, time::Duration};

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "capture_fixture".into(),
    }
}

async fn clean_capture_records(
    database: &TestDatabase,
    manager: Arc<CaptureManager>,
    retention_days: u32,
) {
    use gateway_admin::{model::log_cleanup::*, ports::log_cleanup::LogCleanupStore};
    let store =
        gateway_store::postgres::PgLogCleanupStore::new(database.pool.clone(), None, Some(manager));
    let state = store.state().await.unwrap();
    let mut config = state.config;
    config.requests.selected = false;
    config.files.selected = false;
    config.captures.selected = true;
    config.captures.retention_days = retention_days;
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
    store
        .start(
            CleanupPreview {
                revision: state.revision,
                config: state.config,
                cutoff_at: chrono::Utc::now(),
            },
            &context(),
        )
        .await
        .unwrap();
    for _ in 0..12 {
        store.run_batch().await.unwrap();
        let job = store.state().await.unwrap().job.unwrap();
        if job.status != CleanupJobStatus::Running {
            assert_eq!(job.status, CleanupJobStatus::Succeeded);
            return;
        }
    }
    panic!("capture cleanup did not finish");
}

async fn setup(
    database: &TestDatabase,
    directory: &std::path::Path,
) -> (
    Arc<CaptureManager>,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    sqlx::query("insert into provider_accounts (
        id,provider_kind,name,upstream_user_id,authentication_kind,provider_credentials_json,
        has_refresh_token,credential_observed_at,created_at,updated_at
    ) values ('capture-owner','openai','Capture','fixture-user','oauth','{}',false,now(),now(),now())")
        .execute(&database.pool).await.unwrap();
    let (manager, writer) = CaptureManager::open(database.pool.clone(), directory.into())
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let worker = tokio::spawn(async move {
        writer.run(stop).await.unwrap();
    });
    (manager, cancel, worker)
}

async fn enable(manager: &CaptureManager) -> CaptureTask {
    manager
        .configure(
            RequestCaptureConfig {
                enabled: true,
                ..Default::default()
            },
            &context(),
        )
        .await
        .unwrap();
    manager
        .create(
            CreateCaptureTask {
                scope: CaptureScope::Account,
                target_id: "capture-owner".into(),
                minutes: 15,
                include_media: false,
            },
            &context(),
        )
        .await
        .unwrap()
}

fn trace(manager: &CaptureManager, id: &str) -> TraceContext {
    TraceContext::new(id).with_capture(manager.start(id, "fixture-key", &[]))
}

async fn wait_request(manager: &CaptureManager, id: &str) -> CaptureRecord {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(record) = manager.for_request(id).await.unwrap().into_iter().next() {
                return record;
            }
            assert!(!manager.settings().await.unwrap().storage_fault);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

fn capture_error(manager: &CaptureManager, id: &str, body: &[u8]) {
    let request = trace(manager, id);
    request.capture("client.request.body", body);
    request.record("request.finished", json!({"outcome":"Failed"}));
}

async fn seed_capture(
    database: &TestDatabase,
    directory: &std::path::Path,
    task_id: &str,
    request_id: &str,
) -> CaptureRecord {
    let record = CaptureRecord {
        id: uuid::Uuid::new_v4().to_string(),
        task_id: task_id.into(),
        request_id: request_id.into(),
        bytes: 3,
        incomplete: false,
        created_at: chrono::Utc::now(),
    };
    sqlx::query("insert into request_capture_records(id,task_id,record) values($1::text::uuid,$2::text::uuid,$3)")
        .bind(&record.id).bind(task_id).bind(serde_json::to_value(&record).unwrap())
        .execute(&database.pool).await.unwrap();
    tokio::fs::write(directory.join(format!("{}.jsonl", record.id)), b"{}\n")
        .await
        .unwrap();
    record
}

#[tokio::test]
async fn request_capture_overwrite_evicts_oldest_and_oversized_records_preserve_history() {
    let Some(database) = TestDatabase::create("capture_overwrite").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    let config = RequestCaptureConfig {
        enabled: true,
        global_errors: true,
        quota_mib: 1,
        quota_policy: CaptureQuotaPolicy::Overwrite,
        ..Default::default()
    };
    manager.configure(config, &context()).await.unwrap();
    let body = serde_json::to_vec(&json!({"input":"x".repeat(400 * 1024)})).unwrap();
    capture_error(&manager, "overwrite_oldest", &body);
    let oldest = wait_request(&manager, "overwrite_oldest").await;
    capture_error(&manager, "overwrite_middle", &body);
    let middle = wait_request(&manager, "overwrite_middle").await;
    capture_error(&manager, "overwrite_latest", &body);
    wait_request(&manager, "overwrite_latest").await;
    assert!(
        manager
            .for_request("overwrite_oldest")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !directory
            .path()
            .join(format!("{}.jsonl", oldest.id))
            .exists()
    );
    assert!(
        directory
            .path()
            .join(format!("{}.jsonl", middle.id))
            .exists()
    );
    let before = manager.settings().await.unwrap();
    assert!(before.stored_bytes <= 1024 * 1024);
    assert_eq!(before.record_count, 2);
    assert!(before.global_active);
    capture_error(
        &manager,
        "overwrite_too_large",
        &serde_json::to_vec(&json!({"input":"x".repeat(1200 * 1024)})).unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while manager.settings().await.unwrap().skipped == before.skipped {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let after = manager.settings().await.unwrap();
    assert_eq!(after.stored_bytes, before.stored_bytes);
    assert_eq!(after.record_count, 2);
    assert!(after.global_active && !after.storage_fault);
    assert!(
        manager
            .for_request("overwrite_too_large")
            .await
            .unwrap()
            .is_empty()
    );
    capture_error(&manager, "overwrite_after_large", br#"{"input":"small"}"#);
    wait_request(&manager, "overwrite_after_large").await;
    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    let (manager, writer) = CaptureManager::open(database.pool.clone(), directory.path().into())
        .await
        .unwrap();
    assert_eq!(
        manager.settings().await.unwrap().config.quota_policy,
        CaptureQuotaPolicy::Overwrite
    );
    assert_eq!(manager.settings().await.unwrap().record_count, 3);
    assert!(manager.settings().await.unwrap().global_active);
    drop(writer);
    drop(manager);
    database.close().await;
}

#[tokio::test]
async fn request_capture_clear_resumes_quota_pause_and_keeps_future_captures() {
    let Some(database) = TestDatabase::create("capture_clear_resume").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    manager
        .configure(
            RequestCaptureConfig {
                enabled: true,
                global_errors: true,
                quota_mib: 1,
                ..Default::default()
            },
            &context(),
        )
        .await
        .unwrap();
    let body = serde_json::to_vec(&json!({"input":"x".repeat(600 * 1024)})).unwrap();
    capture_error(&manager, "clear_old", &body);
    let old = wait_request(&manager, "clear_old").await;
    capture_error(&manager, "clear_overflow", &body);
    tokio::time::timeout(Duration::from_secs(10), async {
        while manager.settings().await.unwrap().global_active {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let result = manager
        .clear(
            ClearCaptures {
                confirmed: true,
                cutoff_at: None,
            },
            &context(),
        )
        .await
        .unwrap();
    assert!(result.complete);
    assert_eq!(result.removed_records, 1);
    assert_eq!(result.removed_bytes, old.bytes);
    assert!(!directory.path().join(format!("{}.jsonl", old.id)).exists());
    assert!(manager.settings().await.unwrap().global_active);
    // A request already in progress has not been stored yet and must survive a clear.
    let request = trace(&manager, "clear_inflight");
    request.capture("client.request.body", br#"{"input":"inflight"}"#);
    let boundary = manager
        .clear(
            ClearCaptures {
                confirmed: true,
                cutoff_at: None,
            },
            &context(),
        )
        .await
        .unwrap();
    request.record("request.finished", json!({"outcome":"Failed"}));
    drop(request);
    wait_request(&manager, "clear_inflight").await;
    let result = manager
        .clear(
            ClearCaptures {
                confirmed: true,
                cutoff_at: Some(boundary.cutoff_at),
            },
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(result.removed_records, 0);
    assert_eq!(
        manager.for_request("clear_inflight").await.unwrap().len(),
        1
    );
    manager
        .configure(RequestCaptureConfig::default(), &context())
        .await
        .unwrap();
    manager
        .clear(
            ClearCaptures {
                confirmed: true,
                cutoff_at: None,
            },
            &context(),
        )
        .await
        .unwrap();
    assert!(!manager.settings().await.unwrap().global_active);
    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    database.close().await;
}

#[tokio::test]
async fn request_capture_clear_is_bounded_instance_scoped_and_preserves_usage() {
    let Some(database) = TestDatabase::create("capture_clear_bounds").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let other_directory = tempfile::tempdir().unwrap();
    let (manager, writer) = CaptureManager::open(database.pool.clone(), directory.path().into())
        .await
        .unwrap();
    manager
        .configure(
            RequestCaptureConfig {
                enabled: true,
                global_errors: true,
                ..Default::default()
            },
            &context(),
        )
        .await
        .unwrap();
    let task = manager.status().await.unwrap().tasks[0].clone();
    let (other, other_writer) =
        CaptureManager::open(database.pool.clone(), other_directory.path().into())
            .await
            .unwrap();
    let other_task = other.status().await.unwrap().tasks[0].clone();
    let other_record = seed_capture(
        &database,
        other_directory.path(),
        &other_task.id,
        "other_instance",
    )
    .await;
    for index in 0..260 {
        seed_capture(
            &database,
            directory.path(),
            &task.id,
            &format!("clear_fixture_{index}"),
        )
        .await;
    }
    tokio::fs::write(directory.path().join("unrelated.log"), b"preserve")
        .await
        .unwrap();
    tokio::fs::create_dir(directory.path().join("unrelated-directory"))
        .await
        .unwrap();
    sqlx::query("insert into provider_accounts (id,provider_kind,name,upstream_user_id,authentication_kind,provider_credentials_json,has_refresh_token,credential_observed_at,created_at,updated_at)
        values ('capture-protected-account','openai','Fixture','fixture-user','oauth','{}',false,now(),now(),now())")
        .execute(&database.pool).await.unwrap();
    let account: serde_json::Value = sqlx::query_scalar(
        "select to_jsonb(a) from provider_accounts a where id='capture-protected-account'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    sqlx::query("insert into model_requests (id,client_api_key_ref,config_revision,protocol,operation,endpoint,client_transport,started_at,deadline_at,completed_at,routing_scope,outcome,client_status_code,cost_amount,cost_currency,cost_source,diagnostic_trace_json)
        values ('capture-protected','fixture-key',1,'openai','responses','/v1/responses','http',now(),now()+interval '1 minute',now(),'all','failed',400,0.25,'USD','calculated','{\"error\":\"fixture\"}'::jsonb)")
        .execute(&database.pool).await.unwrap();
    let usage: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(r) from model_requests r where id='capture-protected'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    let denied = manager
        .clear(
            ClearCaptures {
                confirmed: false,
                cutoff_at: None,
            },
            &context(),
        )
        .await;
    assert!(denied.is_err());
    assert_eq!(manager.settings().await.unwrap().record_count, 260);
    let first = manager
        .clear(
            ClearCaptures {
                confirmed: true,
                cutoff_at: None,
            },
            &context(),
        )
        .await
        .unwrap();
    assert!(!first.complete);
    assert_eq!(first.removed_records, 256);
    let new_record = seed_capture(
        &database,
        directory.path(),
        &task.id,
        "after_clear_boundary",
    )
    .await;
    let second = manager
        .clear(
            ClearCaptures {
                confirmed: true,
                cutoff_at: Some(first.cutoff_at),
            },
            &context(),
        )
        .await
        .unwrap();
    assert!(second.complete);
    assert_eq!(second.removed_records, 4);
    assert_eq!(manager.settings().await.unwrap().record_count, 1);
    assert!(
        directory
            .path()
            .join(format!("{}.jsonl", new_record.id))
            .exists()
    );
    assert!(directory.path().join("unrelated-directory").is_dir());
    assert_eq!(
        tokio::fs::read(directory.path().join("unrelated.log"))
            .await
            .unwrap(),
        b"preserve"
    );
    assert!(directory.path().join(".instance").exists());
    assert!(directory.path().join(".lock").exists());
    assert_eq!(other.settings().await.unwrap().record_count, 1);
    assert!(
        other_directory
            .path()
            .join(format!("{}.jsonl", other_record.id))
            .exists()
    );
    let after: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(r) from model_requests r where id='capture-protected'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(after, usage, "request/error/cost fields must be untouched");
    let after_account: serde_json::Value = sqlx::query_scalar(
        "select to_jsonb(a) from provider_accounts a where id='capture-protected-account'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(after_account, account);
    assert_eq!(manager.status().await.unwrap().tasks.len(), 1);
    assert!(manager.settings().await.unwrap().global_active);
    drop(writer);
    drop(manager);
    drop(other_writer);
    drop(other);
    database.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn request_capture_clear_rejects_symlinks_without_deleting_targets_or_index() {
    let Some(database) = TestDatabase::create("capture_clear_symlink").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let protected = outside.path().join("protected.log");
    tokio::fs::write(&protected, b"preserve").await.unwrap();
    let (manager, writer) = CaptureManager::open(database.pool.clone(), directory.path().into())
        .await
        .unwrap();
    manager
        .configure(
            RequestCaptureConfig {
                enabled: true,
                global_errors: true,
                ..Default::default()
            },
            &context(),
        )
        .await
        .unwrap();
    let task = manager.status().await.unwrap().tasks[0].clone();
    let record = seed_capture(&database, directory.path(), &task.id, "symlink_fixture").await;
    let path = directory.path().join(format!("{}.jsonl", record.id));
    tokio::fs::remove_file(&path).await.unwrap();
    std::os::unix::fs::symlink(&protected, &path).unwrap();
    assert!(
        manager
            .clear(
                ClearCaptures {
                    confirmed: true,
                    cutoff_at: None
                },
                &context()
            )
            .await
            .is_err()
    );
    assert_eq!(manager.settings().await.unwrap().record_count, 1);
    assert_eq!(tokio::fs::read(&protected).await.unwrap(), b"preserve");
    assert!(
        tokio::fs::symlink_metadata(&path)
            .await
            .unwrap()
            .file_type()
            .is_symlink()
    );
    drop(writer);
    drop(manager);
    database.close().await;
}
fn select(trace: &TraceContext, account: &str) {
    trace.record("account.selection", json!({"selectedAccountId":account}));
}
async fn wait_records(manager: &CaptureManager, count: usize) -> RequestCaptureStatus {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = manager.status().await.unwrap();
            if status.records.len() == count {
                return status;
            }
            assert!(!status.storage_fault);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn request_capture_only_errors_redacts_four_stages_and_preserves_retry_scope() {
    let Some(database) = TestDatabase::create("capture_errors").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    assert!(manager.start("req_before", "fixture-key", &[]).is_none());
    let task = enable(&manager).await;

    let success = trace(&manager, "req_success");
    success.capture(
        "client.request.body",
        br#"{"input":"normal must not persist"}"#,
    );
    success.attempt(1).record("upstream.completed", json!({}));
    success.record("request.finished", json!({"outcome":"succeeded"}));
    drop(success);
    assert!(manager.status().await.unwrap().records.is_empty());

    let request = trace(&manager, "req_retry");
    request.capture("client.request.body", br#"{"input":"fixture text","access_token":"never-persist","image_url":"data:image/png;base64,private-media","url":"https://user:$pass@example.invalid/a?key=private-query"}"#);
    let first = request.attempt(1);
    select(&first, "capture-owner");
    first.record(
        "excel.transport.failed",
        json!({
            "phase":"connect","cause":"connection_refused","status":200,
            "error":"private-transport-message","url":"private-transport-url"
        }),
    );
    first.record("upstream.business_result", json!({"outcome":"failed"}));
    first.record(
        "excel.transport.failed",
        json!({
            "phase":"private-phase","cause":"private-cause","status":9999
        }),
    );
    first.capture(
        "upstream.request.body",
        br#"{"input":"upstream fixture","headers":{"authorization":"never-header"}}"#,
    );
    for chunk in b"event: response.failed\ndata: {\"type\":\"response.failed\",\"error\":{\"message\":\"fixture\"}}\n\n".chunks(7) {
        first.dump("upstream.chunk", chunk);
    }
    first.record(
        "attempt.failed",
        json!({"kind":"upstream_rejected","upstreamStatus":422}),
    );
    let second = request.attempt(2);
    select(&second, "other-owner");
    second.capture(
        "upstream.request.body",
        br#"{"input":"foreign-body-must-not-persist"}"#,
    );
    second.record("upstream.completed", json!({}));
    request.dump(
        "downstream.event",
        br#"{"type":"response.output_text.delta","delta":"fixture output"}"#,
    );
    request.dump(
        "downstream.event",
        br#"{"type":"response.completed","response":{"id":"fixture"}}"#,
    );
    request.record("request.finished", json!({"outcome":"succeeded"}));
    drop((first, second, request));
    let status = wait_records(&manager, 1).await;
    assert_eq!(status.records[0].task_id, task.id);
    assert!(!status.records[0].incomplete);
    let page = manager
        .read(&status.records[0].id, 0, &context())
        .await
        .unwrap();
    for forbidden in [
        "never-persist",
        "never-header",
        "private-media",
        "private-query",
        "user:$pass",
        "foreign-body",
        "normal must not persist",
        "private-transport-message",
        "private-transport-url",
        "private-phase",
        "private-cause",
    ] {
        assert!(!page.text.contains(forbidden), "{forbidden}");
    }
    for expected in [
        "client.request.body",
        "upstream.request.body",
        "upstream.chunk",
        "downstream.event",
        "fixture output",
        "connection_refused",
        "upstream.business_result",
    ] {
        assert!(page.text.contains(expected), "{expected}");
    }
    assert!(manager.read("../outside", 0, &context()).await.is_err());
    assert!(manager.export_task(&task.id, &context()).await.is_err());
    manager.stop(&task.id, &context()).await.unwrap();
    let stopped = manager
        .status()
        .await
        .unwrap()
        .tasks
        .into_iter()
        .find(|item| item.id == task.id)
        .unwrap();
    assert!(stopped.expires_at < task.expires_at);
    let stored_end: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select expires_at from request_capture_tasks where id::text=$1")
            .bind(&task.id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(stored_end, stopped.expires_at);
    let mut export = manager.export_task(&task.id, &context()).await.unwrap();
    let second = manager
        .export(&status.records[0].id, &context())
        .await
        .unwrap();
    assert!(
        manager
            .export(&status.records[0].id, &context())
            .await
            .is_err()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = export.next().await {
        bytes.extend_from_slice(&chunk.unwrap());
    }
    let exported = String::from_utf8(bytes).unwrap();
    assert!(exported.contains("req_retry"));
    assert!(exported.contains("fixture output"));
    assert!(!exported.contains("never-persist"));
    for line in exported.lines() {
        serde_json::from_str::<serde_json::Value>(line).unwrap();
    }
    drop((export, second));
    manager.delete(&task.id, &context()).await.unwrap();
    assert!(
        manager
            .read(&status.records[0].id, 0, &context())
            .await
            .is_err()
    );
    assert!(manager.status().await.unwrap().records.is_empty());
    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    database.close().await;
}

#[tokio::test]
async fn request_capture_storage_fault_is_isolated_and_does_not_take_another_instances_lock() {
    let Some(database) = TestDatabase::create("capture_unavailable").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (owner, cancel, worker) = setup(&database, directory.path()).await;
    let task = enable(&owner).await;
    let (contender, _) = CaptureManager::open(database.pool.clone(), directory.path().into())
        .await
        .unwrap();
    assert!(contender.status().await.unwrap().storage_fault);
    assert!(
        contender
            .start("req_conflict", "fixture-key", &[])
            .is_none()
    );
    assert_eq!(owner.status().await.unwrap().tasks[0].id, task.id);
    assert_eq!(
        owner.status().await.unwrap().tasks[0].status,
        CaptureTaskStatus::Running
    );
    let bad_path = directory.path().join("not-a-directory");
    std::fs::write(&bad_path, b"fixture").unwrap();
    let (unavailable, _) = CaptureManager::open(database.pool.clone(), bad_path)
        .await
        .unwrap();
    assert!(unavailable.status().await.unwrap().storage_fault);
    assert!(
        unavailable
            .start("req_unavailable", "fixture-key", &[])
            .is_none()
    );
    cancel.cancel();
    worker.await.unwrap();
    drop((owner, contender, unavailable));
    database.close().await;
}

#[tokio::test]
async fn request_capture_large_completion_local_cancel_and_overflow_never_invent_errors() {
    let Some(database) = TestDatabase::create("capture_success").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    enable(&manager).await;
    for index in 0..4 {
        let request = trace(&manager, &format!("req_completed_{index}"));
        let attempt = request.attempt(1);
        select(&attempt, "capture-owner");
        let large = format!(
            "data: {{\"type\":\"response.completed\",\"padding\":\"{}\"}}\n\n",
            "x".repeat(32 * 1024)
        );
        for chunk in large.as_bytes().chunks(31) {
            attempt.dump("upstream.chunk", chunk);
        }
        attempt.record("upstream.completed", json!({}));
        request.record("downstream.cancelled", json!({"reason":"local_close"}));
        request.record("request.finished", json!({"outcome":"Cancelled"}));
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let status = manager.status().await.unwrap();
    assert!(status.records.is_empty());
    assert_eq!(status.active_sessions, 0);
    assert_eq!(status.buffered_bytes, 0);
    assert!(status.skipped >= 4);
    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    database.close().await;
}

#[tokio::test]
async fn request_capture_stopping_disabling_and_restarting_never_resume_tasks() {
    let Some(database) = TestDatabase::create("capture_restart").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    let task = enable(&manager).await;
    let request = trace(&manager, "req_partial");
    select(&request, "capture-owner");
    request.record("attempt.failed", json!({"kind":"upstream_rejected"}));
    request.capture(
        "upstream.error.body",
        br#"{"error":{"message":"before stop"}}"#,
    );
    manager.stop(&task.id, &context()).await.unwrap();
    request.capture(
        "upstream.error.body",
        br#"{"message":"after-stop-should-not-persist"}"#,
    );
    drop(request);
    let status = wait_records(&manager, 1).await;
    let id = &status.records[0].id;
    assert!(
        !manager
            .read(id, 0, &context())
            .await
            .unwrap()
            .text
            .contains("after-stop")
    );
    let disabled_task = enable(&manager).await;
    manager
        .configure(RequestCaptureConfig::default(), &context())
        .await
        .unwrap();
    assert!(manager.read(id, 0, &context()).await.is_ok());
    let disabled = manager
        .status()
        .await
        .unwrap()
        .tasks
        .into_iter()
        .find(|item| item.id == disabled_task.id)
        .unwrap();
    assert_eq!(disabled.status, CaptureTaskStatus::Stopped);
    assert!(disabled.expires_at < disabled_task.expires_at);
    let running = enable(&manager).await;
    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    tokio::fs::write(
        directory
            .path()
            .join("00000000-0000-0000-0000-000000000001.jsonl"),
        b"orphan",
    )
    .await
    .unwrap();
    let (manager, _) = CaptureManager::open(database.pool.clone(), directory.path().into())
        .await
        .unwrap();
    let status = manager.status().await.unwrap();
    assert_eq!(status.records.len(), 1);
    assert!(
        status
            .tasks
            .iter()
            .find(|task| task.id == running.id)
            .unwrap()
            .expires_at
            < running.expires_at
    );
    assert_eq!(
        status
            .tasks
            .iter()
            .find(|task| task.id == running.id)
            .unwrap()
            .status,
        CaptureTaskStatus::Interrupted
    );
    assert!(
        manager
            .start("req_after_restart", "fixture-key", &[])
            .is_none()
    );
    assert!(
        !directory
            .path()
            .join("00000000-0000-0000-0000-000000000001.jsonl")
            .exists()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(directory.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(directory.path().join(format!("{id}.jsonl")))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(manager);
    database.close().await;
}

#[tokio::test]
async fn request_capture_disk_quota_stops_capture_without_indexing_partial_files() {
    let Some(database) = TestDatabase::create("capture_quota").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    enable(&manager).await;
    manager
        .configure(
            RequestCaptureConfig {
                enabled: true,
                quota_mib: 1,
                retention_days: 7,
                ..Default::default()
            },
            &context(),
        )
        .await
        .unwrap();
    let request = trace(&manager, "req_quota");
    select(&request, "capture-owner");
    request.capture(
        "client.request.body",
        &serde_json::to_vec(&json!({"input":"x".repeat(2 * 1024 * 1024)})).unwrap(),
    );
    request.record("attempt.failed", json!({"kind":"upstream_rejected"}));
    drop(request);
    // The skip counter precedes the asynchronous task stop and buffer release.
    let status = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = manager.status().await.unwrap();
            if status.skipped > 0
                && status
                    .tasks
                    .iter()
                    .all(|task| task.status == CaptureTaskStatus::Stopped)
                && status.buffered_bytes == 0
            {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(status.records.is_empty());
    assert!(
        status
            .tasks
            .iter()
            .all(|task| task.status == CaptureTaskStatus::Stopped)
    );
    assert_eq!(status.buffered_bytes, 0);
    assert!(!status.storage_fault);
    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    database.close().await;
}

#[test]
fn request_capture_legacy_config_does_not_enable_global_collection() {
    let config: RequestCaptureConfig = serde_json::from_value(json!({
        "enabled": true, "quotaMib": 1024, "retentionDays": 7
    }))
    .unwrap();
    assert!(!config.global_errors);
    assert!(!config.include_media);
    assert_eq!(config.quota_policy, CaptureQuotaPolicy::Stop);
    assert_eq!(
        RequestCaptureConfig::default().quota_policy,
        CaptureQuotaPolicy::Stop
    );
    let zero_retention = RequestCaptureConfig {
        retention_days: 0,
        ..Default::default()
    };
    assert!(zero_retention.validate().is_ok());
    assert!(
        serde_json::from_value::<RequestCaptureConfig>(json!({
            "enabled":true, "quotaMib":1, "retentionDays":7, "quotaPolicy":"unknown"
        }))
        .is_err()
    );
    assert!(
        CreateCaptureTask {
            scope: CaptureScope::Global,
            target_id: "fixture".into(),
            minutes: 10,
            include_media: false,
        }
        .validate()
        .is_err()
    );
}

#[tokio::test]
async fn request_capture_global_is_opt_in_request_scoped_and_readable_after_disable() {
    let Some(database) = TestDatabase::create("capture_global").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    let old_task = enable(&manager).await;
    assert!(!manager.settings().await.unwrap().global_active);
    let config = RequestCaptureConfig {
        enabled: true,
        global_errors: true,
        quota_mib: 8,
        retention_days: 2,
        ..Default::default()
    };
    manager.configure(config.clone(), &context()).await.unwrap();
    assert!(manager.settings().await.unwrap().global_active);
    let status = manager.status().await.unwrap();
    assert_eq!(
        status
            .tasks
            .iter()
            .find(|task| task.id == old_task.id)
            .unwrap()
            .status,
        CaptureTaskStatus::Stopped
    );
    assert_eq!(
        status
            .tasks
            .iter()
            .filter(|task| task.status == CaptureTaskStatus::Running)
            .count(),
        1
    );

    // No account has been selected: a local adaptation failure still has evidence.
    let request = trace(&manager, "req_local_failure");
    request.capture(
        "client.request.body",
        br#"{"input":"local fixture","access_token":"private-fixture"}"#,
    );
    request.record("downstream.status", json!({"status":400}));
    request.record("request.finished", json!({"outcome":"Failed"}));
    drop(request);
    wait_records(&manager, 1).await;

    let request = trace(&manager, "req_other_account");
    select(&request, "another-fixture-account");
    request.capture(
        "upstream.error.body",
        br#"{"error":{"message":"another fixture"}}"#,
    );
    request.record("attempt.failed", json!({"kind":"upstream_rejected"}));
    request.record("request.finished", json!({"outcome":"Failed"}));
    drop(request);
    wait_records(&manager, 2).await;

    let success = trace(&manager, "req_success_global");
    success.capture(
        "client.request.body",
        br#"{"input":"success is not retained"}"#,
    );
    success.record("request.finished", json!({"outcome":"Succeeded"}));
    drop(success);
    assert!(
        manager
            .for_request("req_success_global")
            .await
            .unwrap()
            .is_empty()
    );
    let records = manager.for_request("req_local_failure").await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].request_id, "req_local_failure");
    assert!(manager.for_request("req_unknown").await.unwrap().is_empty());
    let id = &records[0].id;
    let body = manager.read(id, 0, &context()).await.unwrap().text;
    assert!(body.contains("local fixture"));
    assert!(!body.contains("private-fixture"));
    assert!(!body.contains("another fixture"));

    let mut disabled = config.clone();
    disabled.enabled = false;
    disabled.global_errors = false;
    manager.configure(disabled, &context()).await.unwrap();
    assert!(manager.start("req_disabled", "any-key", &[]).is_none());
    assert!(!manager.settings().await.unwrap().global_active);
    assert_eq!(manager.read(id, 0, &context()).await.unwrap().text, body);
    assert_eq!(
        manager
            .for_request("req_local_failure")
            .await
            .unwrap()
            .len(),
        1
    );
    let exported = manager
        .export(id, &context())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert!(exported.iter().all(Result::is_ok));

    manager.configure(config, &context()).await.unwrap();
    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    let (manager, _) = CaptureManager::open(database.pool.clone(), directory.path().into())
        .await
        .unwrap();
    assert!(manager.settings().await.unwrap().global_active);
    assert!(
        manager
            .start("req_restart", "different-key", &["different-group"])
            .is_some()
    );
    assert_eq!(manager.settings().await.unwrap().config.quota_mib, 8);
    assert_eq!(manager.settings().await.unwrap().config.retention_days, 2);
    drop(manager);
    database.close().await;
}

#[tokio::test]
async fn request_capture_global_retention_is_per_record_not_running_task() {
    let Some(database) = TestDatabase::create("capture_global_retention").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    manager
        .configure(
            RequestCaptureConfig {
                enabled: true,
                global_errors: true,
                retention_days: 1,
                ..Default::default()
            },
            &context(),
        )
        .await
        .unwrap();
    for id in ["req_old_global", "req_recent_global"] {
        let request = trace(&manager, id);
        request.capture("client.request.body", br#"{"input":"retention fixture"}"#);
        request.record("request.finished", json!({"outcome":"Failed"}));
        drop(request);
    }
    wait_records(&manager, 2).await;
    let old = manager
        .for_request("req_old_global")
        .await
        .unwrap()
        .remove(0);
    sqlx::query(
        "update request_capture_records set created_at=now()-interval '2 days' where id::text=$1",
    )
    .bind(&old.id)
    .execute(&database.pool)
    .await
    .unwrap();
    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    let (manager, _) = CaptureManager::open(database.pool.clone(), directory.path().into())
        .await
        .unwrap();
    assert_eq!(
        manager.for_request("req_old_global").await.unwrap().len(),
        1,
        "restart must not bypass the automatic cleanup switch"
    );
    clean_capture_records(&database, manager.clone(), 7).await;
    assert!(
        manager
            .for_request("req_old_global")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!directory.path().join(format!("{}.jsonl", old.id)).exists());
    assert_eq!(
        manager
            .for_request("req_recent_global")
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(manager.settings().await.unwrap().global_active);
    drop(manager);
    database.close().await;
}

#[tokio::test]
async fn request_capture_zero_retention_removes_finished_material_but_keeps_running_task() {
    let Some(database) = TestDatabase::create("capture_zero_retention").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    let finished_task = enable(&manager).await;
    let finished_request = trace(&manager, "req_zero_finished");
    select(&finished_request, "capture-owner");
    finished_request.record("request.finished", json!({"outcome":"Failed"}));
    drop(finished_request);
    let finished_record = wait_request(&manager, "req_zero_finished").await;
    manager.stop(&finished_task.id, &context()).await.unwrap();

    let running_task = enable(&manager).await;
    let running_request = trace(&manager, "req_zero_running");
    select(&running_request, "capture-owner");
    running_request.record("request.finished", json!({"outcome":"Failed"}));
    drop(running_request);
    let running_record = wait_request(&manager, "req_zero_running").await;
    let old = chrono::Utc::now() - chrono::Duration::days(1);
    sqlx::query("update request_capture_tasks set expires_at=$1 where id::text=$2")
        .bind(old)
        .bind(&finished_task.id)
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("update request_capture_records set created_at=$1 where id::text = any($2)")
        .bind(old)
        .bind(vec![finished_record.id.clone(), running_record.id.clone()])
        .execute(&database.pool)
        .await
        .unwrap();

    clean_capture_records(&database, manager.clone(), 0).await;

    assert!(
        manager
            .for_request("req_zero_finished")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !directory
            .path()
            .join(format!("{}.jsonl", finished_record.id))
            .exists()
    );
    assert_eq!(
        manager.for_request("req_zero_running").await.unwrap().len(),
        1
    );
    assert!(
        directory
            .path()
            .join(format!("{}.jsonl", running_record.id))
            .exists()
    );
    assert!(
        manager
            .status()
            .await
            .unwrap()
            .tasks
            .iter()
            .any(|task| task.id == running_task.id && task.status == CaptureTaskStatus::Running)
    );

    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    database.close().await;
}

#[tokio::test]
async fn request_capture_global_quota_pauses_without_affecting_delivery_and_can_resume() {
    let Some(database) = TestDatabase::create("capture_global_quota").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    let mut config = RequestCaptureConfig {
        enabled: true,
        global_errors: true,
        quota_mib: 1,
        ..Default::default()
    };
    manager.configure(config.clone(), &context()).await.unwrap();
    let body = serde_json::to_vec(&json!({"input":"x".repeat(600 * 1024)})).unwrap();
    for (index, id) in ["req_quota_first", "req_quota_overflow"]
        .into_iter()
        .enumerate()
    {
        let request = trace(&manager, id);
        request.capture("client.request.body", &body);
        request.record("request.finished", json!({"outcome":"Failed"}));
        drop(request);
        if index == 0 {
            wait_records(&manager, 1).await;
        }
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if !manager.settings().await.unwrap().global_active {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let settings = manager.settings().await.unwrap();
    assert!(!settings.storage_fault);
    assert!(settings.config.enabled);
    assert!(settings.skipped > 0);
    assert!(manager.start("req_paused", "any-key", &[]).is_none());
    assert!(
        manager
            .for_request("req_quota_overflow")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        manager.for_request("req_quota_first").await.unwrap().len(),
        1
    );
    config.quota_mib = 2;
    manager.configure(config, &context()).await.unwrap();
    assert!(manager.settings().await.unwrap().global_active);
    let request = trace(&manager, "req_quota_resumed");
    request.capture("client.request.body", &body);
    request.record("request.finished", json!({"outcome":"Failed"}));
    drop(request);
    wait_records(&manager, 2).await;
    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    database.close().await;
}

#[tokio::test]
async fn request_capture_retention_removes_metadata_and_body_only_when_requested() {
    let Some(database) = TestDatabase::create("capture_retention").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    let task = enable(&manager).await;
    let request = trace(&manager, "req_retention");
    select(&request, "capture-owner");
    request.capture("upstream.error.body", br#"{"error":"fixture retention"}"#);
    request.record("attempt.failed", json!({"kind":"protocol"}));
    request.record("request.finished", json!({"outcome":"Failed"}));
    drop(request);
    let status = wait_records(&manager, 1).await;
    let body = directory
        .path()
        .join(format!("{}.jsonl", status.records[0].id));
    assert!(body.exists());
    manager.stop(&task.id, &context()).await.unwrap();
    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    sqlx::query(
        "update request_capture_tasks set expires_at=now()-interval '8 days' where id::text=$1",
    )
    .bind(&task.id)
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query("update request_capture_records set created_at=now()-interval '8 days' where task_id::text=$1")
        .bind(&task.id).execute(&database.pool).await.unwrap();
    let (manager, _) = CaptureManager::open(database.pool.clone(), directory.path().into())
        .await
        .unwrap();
    assert!(body.exists());
    clean_capture_records(&database, manager.clone(), 7).await;
    let status = manager.status().await.unwrap();
    assert!(status.tasks.is_empty());
    assert!(status.records.is_empty());
    assert!(!body.exists());
    drop(manager);
    database.close().await;
}

#[tokio::test]
async fn request_capture_overlapping_tasks_stop_independently_and_keep_media_policy() {
    let Some(database) = TestDatabase::create("capture_overlap").await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (manager, cancel, worker) = setup(&database, directory.path()).await;
    let stopped = enable(&manager).await;
    let continuing = manager
        .create(
            CreateCaptureTask {
                scope: CaptureScope::Account,
                target_id: "capture-owner".into(),
                minutes: 15,
                include_media: true,
            },
            &context(),
        )
        .await
        .unwrap();
    let request = trace(&manager, "req_overlap");
    select(&request, "capture-owner");
    request.record("attempt.failed", json!({"kind":"protocol"}));
    request.capture(
        "upstream.event",
        br#"{"image_url":"data:image/png;base64,fixture-media","content":[{"type":"input_audio","input_audio":{"data":"fixture-audio","format":"wav"}},{"type":"input_file","file_data":"fixture-file"},{"type":"image_generation_call","result":"fixture-generated-image"}]}"#,
    );
    manager.stop(&stopped.id, &context()).await.unwrap();
    request.capture("downstream.event", br#"{"text":"after-independent-stop"}"#);
    request.record("request.finished", json!({"outcome":"Failed"}));
    drop(request);
    let status = wait_records(&manager, 2).await;
    for record in status.records {
        let text = manager.read(&record.id, 0, &context()).await.unwrap().text;
        assert_eq!(
            text.contains("after-independent-stop"),
            record.task_id == continuing.id
        );
        assert_eq!(
            text.contains("fixture-media"),
            record.task_id == continuing.id
        );
        assert_eq!(record.incomplete, record.task_id == stopped.id);
        for media in ["fixture-audio", "fixture-file", "fixture-generated-image"] {
            assert_eq!(text.contains(media), record.task_id == continuing.id);
        }
    }
    cancel.cancel();
    worker.await.unwrap();
    drop(manager);
    database.close().await;
}
