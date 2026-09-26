//! Single-instance, administrator-controlled error body storage.

mod control;
mod filter;
mod session;

use chrono::Utc;
use futures::future::BoxFuture;
use gateway_admin::{
    model::request_capture::*,
    ports::store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
};
use gateway_core::{
    lifecycle::CancellationToken,
    task::{DaemonTask, WorkerTaskError},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, Semaphore, mpsc},
};
use uuid::Uuid;

use session::{ActiveTask, Budget, CaptureBundle};

pub struct CaptureManager {
    shared: Arc<Shared>,
    pool: PgPool,
    directory: PathBuf,
    instance_id: String,
    io: Mutex<()>,
    exports: Arc<Semaphore>,
    _directory_lock: Option<std::fs::File>,
}

struct Control {
    config: RequestCaptureConfig,
    tasks: Vec<Arc<ActiveTask>>,
}

struct Shared {
    control: RwLock<Control>,
    budget: Arc<Budget>,
    sessions: AtomicUsize,
    skipped: AtomicU64,
    fault: AtomicBool,
    sender: mpsc::Sender<CaptureBundle>,
}

pub struct CaptureWriter {
    manager: Arc<CaptureManager>,
    receiver: Mutex<mpsc::Receiver<CaptureBundle>>,
}

fn unavailable(_: impl std::fmt::Display) -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "request capture",
        "capture storage unavailable",
    )
}
fn conflict() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Conflict,
        "request capture",
        "capture is unavailable",
    )
}
fn missing() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::NotFound,
        "request capture",
        "capture not found",
    )
}
fn uuid(value: &str) -> AdminStoreResult<String> {
    Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| missing())
}

impl CaptureManager {
    pub async fn open(
        pool: PgPool,
        directory: PathBuf,
    ) -> AdminStoreResult<(Arc<Self>, CaptureWriter)> {
        let path = directory.clone();
        let storage = tokio::task::spawn_blocking(move || {
            use fs2::FileExt;
            std::fs::create_dir_all(&path)?;
            if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
                return Err(std::io::Error::other("capture directory is a symlink"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
                let lock = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .mode(0o600)
                    .open(path.join(".lock"))?;
                lock.try_lock_exclusive()?;
                let identity = path.join(".instance");
                let id = match std::fs::read_to_string(&identity) {
                    Ok(id) => Uuid::parse_str(id.trim())
                        .map_err(std::io::Error::other)?
                        .to_string(),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        let id = Uuid::new_v4().to_string();
                        let mut file = std::fs::OpenOptions::new()
                            .create_new(true)
                            .write(true)
                            .mode(0o600)
                            .open(identity)?;
                        std::io::Write::write_all(&mut file, id.as_bytes())?;
                        file.sync_all()?;
                        id
                    }
                    Err(error) => return Err(error),
                };
                Ok::<_, std::io::Error>((lock, id))
            }
            #[cfg(not(unix))]
            {
                Err::<(std::fs::File, String), _>(std::io::Error::other(
                    "private capture storage requires Unix permissions",
                ))
            }
        })
        .await
        .map_err(unavailable)?;
        // Diagnostics are optional: an inaccessible or already-owned directory
        // disables capture, never the gateway or the ordinary request path.
        let (lock, instance_id, storage_fault) = match storage {
            Ok((lock, id)) => (Some(lock), id, false),
            Err(_) => {
                tracing::warn!("request capture storage unavailable; capture disabled");
                (None, Uuid::new_v4().to_string(), true)
            }
        };
        let value: Value =
            sqlx::query_scalar("select config from request_capture_config where singleton")
                .fetch_one(&pool)
                .await
                .map_err(unavailable)?;
        let config: RequestCaptureConfig = serde_json::from_value(value).map_err(unavailable)?;
        config.validate().map_err(unavailable)?;
        if !storage_fault {
            sqlx::query("update request_capture_tasks set task=jsonb_set(jsonb_set(task,'{status}','\"interrupted\"'),'{expiresAt}',to_jsonb(least(expires_at,now()))), expires_at=least(expires_at,now()) where instance_id::text=$1 and task->>'status'='running'")
                .bind(&instance_id).execute(&pool).await.map_err(unavailable)?;
        }
        let (sender, receiver) = mpsc::channel(32);
        let manager = Arc::new(Self {
            shared: Arc::new(Shared {
                control: RwLock::new(Control {
                    config,
                    tasks: Vec::new(),
                }),
                budget: Arc::new(Budget(AtomicUsize::new(0))),
                sessions: AtomicUsize::new(0),
                skipped: AtomicU64::new(0),
                fault: AtomicBool::new(storage_fault),
                sender,
            }),
            pool,
            directory,
            instance_id,
            io: Mutex::new(()),
            exports: Arc::new(Semaphore::new(2)),
            _directory_lock: lock,
        });
        if !storage_fault && manager.cleanup().await.is_err() {
            manager.fault();
        }
        Ok((
            manager.clone(),
            CaptureWriter {
                manager,
                receiver: Mutex::new(receiver),
            },
        ))
    }

    fn fault(&self) {
        self.shared.fault.store(true, Ordering::Release);
        if let Ok(control) = self.shared.control.read() {
            for task in &control.tasks {
                task.stopped.store(true, Ordering::Release);
            }
        }
    }

    fn file(&self, id: &str) -> AdminStoreResult<PathBuf> {
        Ok(self.directory.join(format!("{}.jsonl", uuid(id)?)))
    }

    async fn tasks(&self) -> AdminStoreResult<Vec<CaptureTask>> {
        let rows: Vec<Value> = sqlx::query_scalar("select task from request_capture_tasks where instance_id::text=$1 order by expires_at desc limit 100")
            .bind(&self.instance_id).fetch_all(&self.pool).await.map_err(unavailable)?;
        rows.into_iter()
            .map(|row| serde_json::from_value(row).map_err(unavailable))
            .collect()
    }

    async fn cleanup(&self) -> AdminStoreResult<()> {
        if self.shared.fault.load(Ordering::Acquire) {
            return Ok(());
        }
        let _io = self.io.lock().await;
        let days = self
            .shared
            .control
            .read()
            .map_err(unavailable)?
            .config
            .retention_days;
        sqlx::query("delete from request_capture_tasks where instance_id::text=$1 and expires_at < now() - ($2::int * interval '1 day')")
            .bind(&self.instance_id).bind(i32::from(days)).execute(&self.pool).await.map_err(unavailable)?;
        sqlx::query("update request_capture_tasks set task=jsonb_set(task,'{status}','\"expired\"') where instance_id::text=$1 and expires_at<=now() and task->>'status'='running'")
            .bind(&self.instance_id).execute(&self.pool).await.map_err(unavailable)?;
        let keep: Vec<String> = sqlx::query_scalar(
            "select r.id::text from request_capture_records r join request_capture_tasks t on t.id=r.task_id where t.instance_id::text=$1"
        ).bind(&self.instance_id).fetch_all(&self.pool).await.map_err(unavailable)?;
        let keep: BTreeSet<_> = keep.into_iter().map(|id| format!("{id}.jsonl")).collect();
        let mut entries = tokio::fs::read_dir(&self.directory)
            .await
            .map_err(unavailable)?;
        while let Some(entry) = entries.next_entry().await.map_err(unavailable)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".jsonl") && !keep.contains(&name) {
                tokio::fs::remove_file(entry.path())
                    .await
                    .map_err(unavailable)?;
            }
        }
        self.shared
            .control
            .write()
            .map_err(unavailable)?
            .tasks
            .retain(|task| {
                if task.task.expires_at <= Utc::now() {
                    task.stopped.store(true, Ordering::Release);
                }
                !task.stopped.load(Ordering::Acquire)
            });
        Ok(())
    }

    async fn persist(&self, bundle: CaptureBundle) -> AdminStoreResult<()> {
        let _io = self.io.lock().await;
        if self.shared.fault.load(Ordering::Acquire) {
            return Ok(());
        }
        for (task_index, task) in &bundle.tasks {
            let exists: bool = sqlx::query_scalar("select exists(select 1 from request_capture_tasks where id::text=$1 and instance_id::text=$2)")
                .bind(&task.task.id).bind(&self.instance_id).fetch_one(&self.pool).await.map_err(unavailable)?;
            if !exists {
                continue;
            }
            let mut bodies = BTreeMap::new();
            for (index, fragment) in bundle.fragments.iter().enumerate() {
                if fragment.task_mask & (1 << task_index) == 0 {
                    continue;
                }
                if task.task.scope == CaptureScope::Account
                    && fragment.stage.starts_with("upstream.")
                    && fragment.account.as_deref() != Some(task.task.target_id.as_str())
                {
                    continue;
                }
                let separate = matches!(fragment.stage, "upstream.event" | "downstream.event");
                let key = (
                    fragment.stage,
                    fragment.attempt,
                    fragment.exchange,
                    separate.then_some(index),
                );
                bodies.entry(key).or_insert_with(Vec::new).push(fragment);
            }
            let mut incomplete =
                bundle.incomplete || bundle.incomplete_tasks & (1 << task_index) != 0;
            let id = Uuid::new_v4().to_string();
            let file = self.file(&id)?;
            let quota = u64::from(
                self.shared
                    .control
                    .read()
                    .map_err(unavailable)?
                    .config
                    .quota_mib,
            ) * 1024
                * 1024;
            let used: i64 = sqlx::query_scalar(
                "select coalesce(sum((r.record->>'bytes')::bigint),0)::bigint from request_capture_records r join request_capture_tasks t on t.id=r.task_id where t.instance_id::text=$1"
            ).bind(&self.instance_id).fetch_one(&self.pool).await.map_err(unavailable)?;
            let mut output = tokio::fs::OpenOptions::new();
            output.create_new(true).write(true);
            #[cfg(unix)]
            output.mode(0o600);
            let mut output = output.open(&file).await.map_err(unavailable)?;
            let mut bytes = 0u64;
            let result = async {
                write_line(&mut output, &json!({"schemaVersion":1,"requestId":bundle.request_id,
                    "accounts":bundle.accounts,"failedAccounts":bundle.failed_accounts,
                    "instanceId":self.instance_id,"error":true}), &mut bytes, used, quota).await?;
                // Materialize one bounded stage at a time, not the entire record.
                for ((stage, attempt, exchange, _), fragments) in bodies {
                    let mut body = filter::BodyFilter::new();
                    for fragment in fragments {
                        body.push(&fragment.bytes);
                    }
                    let media = task.task.include_media;
                    let (events, partial) = tokio::task::spawn_blocking(move || body.finish(media))
                        .await.map_err(unavailable)?;
                    incomplete |= partial;
                    for event in events {
                        write_line(&mut output,
                            &json!({"stage":stage,"attempt":attempt,"exchange":exchange,"body":event}),
                            &mut bytes, used, quota).await?;
                    }
                }
                write_line(&mut output, &json!({"captureEnd":{"incomplete":incomplete}}),
                    &mut bytes, used, quota).await?;
                output.flush().await.map_err(unavailable)?;
                output.sync_all().await.map_err(unavailable)?;
                let record = CaptureRecord { id: id.clone(), task_id:task.task.id.clone(),
                    request_id:bundle.request_id.clone(), bytes, incomplete, created_at:Utc::now() };
                sqlx::query("insert into request_capture_records(id,task_id,record) values($1::text::uuid,$2::text::uuid,$3)")
                    .bind(&id).bind(&task.task.id).bind(serde_json::to_value(record).map_err(unavailable)?)
                    .execute(&self.pool).await.map_err(unavailable)?;
                Ok::<_, AdminStoreError>(())
            }.await;
            drop(output);
            if let Err(error) = result {
                tokio::fs::remove_file(&file).await.map_err(unavailable)?;
                self.shared.skipped.fetch_add(1, Ordering::Relaxed);
                if error.kind() == AdminStoreErrorKind::Conflict {
                    sqlx::query("update request_capture_tasks set task=jsonb_set(jsonb_set(task,'{status}','\"stopped\"'),'{expiresAt}',to_jsonb(least(expires_at,now()))), expires_at=least(expires_at,now()) where instance_id::text=$1 and task->>'status'='running'")
                        .bind(&self.instance_id).execute(&self.pool).await.map_err(unavailable)?;
                    let control = self.shared.control.read().map_err(unavailable)?;
                    for task in &control.tasks {
                        task.stopped.store(true, Ordering::Release);
                    }
                    return Ok(());
                }
                return Err(error);
            }
        }
        Ok(())
    }
}

async fn write_line(
    output: &mut tokio::fs::File,
    line: &Value,
    bytes: &mut u64,
    used: i64,
    quota: u64,
) -> AdminStoreResult<()> {
    let mut encoded = serde_json::to_vec(line).map_err(unavailable)?;
    encoded.push(b'\n');
    let next = bytes.saturating_add(encoded.len() as u64);
    if (used.max(0) as u64).saturating_add(next) > quota || next > 128 * 1024 * 1024 {
        return Err(conflict());
    }
    output.write_all(&encoded).await.map_err(unavailable)?;
    *bytes = next;
    Ok(())
}

impl DaemonTask for CaptureWriter {
    fn run(&self, cancellation: CancellationToken) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let mut receiver = self.receiver.lock().await;
            let mut cleanup = tokio::time::interval(Duration::from_secs(30));
            cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => return Ok(()),
                    _ = cleanup.tick() => {
                        if self.manager.cleanup().await.is_err() { self.manager.fault(); }
                    }
                    bundle = receiver.recv() => {
                        let Some(bundle) = bundle else { return Ok(()) };
                        if self.manager.persist(bundle).await.is_err() {
                            self.manager.fault();
                            tracing::warn!("request capture storage disabled after write failure");
                        }
                    }
                }
            }
        })
    }
}
