use super::*;
use async_trait::async_trait;
use gateway_admin::{
    model::MutationContext,
    ports::request_capture::{CaptureExport, RequestCaptureStore},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

impl CaptureManager {
    fn enabled(&self) -> AdminStoreResult<()> {
        if self
            .shared
            .control
            .read()
            .map_err(unavailable)?
            .config
            .enabled
            && !self.shared.fault.load(Ordering::Acquire)
        {
            Ok(())
        } else {
            Err(conflict())
        }
    }

    async fn audit(
        &self,
        action: &'static str,
        id: &str,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        crate::postgres::insert_admin_audit_event(
            &mut tx,
            crate::mutation_audit(context, action, "request_capture", id, vec![]),
        )
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)
    }

    async fn record_file(&self, id: &str) -> AdminStoreResult<tokio::fs::File> {
        let id = uuid(id)?;
        let exists: bool = sqlx::query_scalar(
            "select exists(select 1 from request_capture_records r join request_capture_tasks t on t.id=r.task_id where r.id::text=$1 and t.instance_id::text=$2)"
        ).bind(&id).bind(&self.instance_id).fetch_one(&self.pool).await.map_err(unavailable)?;
        if !exists {
            return Err(missing());
        }
        let path = self.file(&id)?;
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(unavailable)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(unavailable("invalid file"));
        }
        tokio::fs::File::open(path).await.map_err(unavailable)
    }
}

#[async_trait]
impl RequestCaptureStore for CaptureManager {
    async fn status(&self) -> AdminStoreResult<RequestCaptureStatus> {
        let records: Vec<Value> = sqlx::query_scalar(
            "select r.record from request_capture_records r join request_capture_tasks t on t.id=r.task_id where t.instance_id::text=$1 order by r.created_at desc limit 200"
        ).bind(&self.instance_id).fetch_all(&self.pool).await.map_err(unavailable)?;
        let config = self
            .shared
            .control
            .read()
            .map_err(unavailable)?
            .config
            .clone();
        Ok(RequestCaptureStatus {
            config,
            instance_id: self.instance_id.clone(),
            tasks: self.tasks().await?,
            records: records
                .into_iter()
                .map(|row| serde_json::from_value(row).map_err(unavailable))
                .collect::<Result<_, _>>()?,
            skipped: self.shared.skipped.load(Ordering::Acquire),
            active_sessions: self.shared.sessions.load(Ordering::Acquire),
            buffered_bytes: self.shared.budget.0.load(Ordering::Acquire),
            storage_fault: self.shared.fault.load(Ordering::Acquire),
        })
    }

    async fn configure(
        &self,
        config: RequestCaptureConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        config.validate().map_err(unavailable)?;
        let _io = self.io.lock().await;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("update request_capture_config set config=$1 where singleton")
            .bind(serde_json::to_value(&config).map_err(unavailable)?)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        if !config.enabled {
            sqlx::query("update request_capture_tasks set task=jsonb_set(task,'{status}','\"stopped\"') where instance_id::text=$1 and task->>'status'='running'")
                .bind(&self.instance_id).execute(&mut *tx).await.map_err(unavailable)?;
        }
        crate::postgres::insert_admin_audit_event(
            &mut tx,
            crate::mutation_audit(
                context,
                "request_capture.configure",
                "request_capture",
                "config",
                vec!["config".into()],
            ),
        )
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        let mut control = self.shared.control.write().map_err(unavailable)?;
        if !config.enabled {
            for task in &control.tasks {
                task.stopped.store(true, Ordering::Release);
            }
            control.tasks.clear();
        }
        control.config = config;
        Ok(())
    }

    async fn create(
        &self,
        input: CreateCaptureTask,
        context: &MutationContext,
    ) -> AdminStoreResult<CaptureTask> {
        input.validate().map_err(unavailable)?;
        let _io = self.io.lock().await;
        self.enabled()?;
        if self
            .shared
            .control
            .read()
            .map_err(unavailable)?
            .tasks
            .iter()
            .filter(|task| {
                !task.stopped.load(Ordering::Acquire) && task.task.expires_at > Utc::now()
            })
            .count()
            >= 16
        {
            return Err(conflict());
        }
        let query = match input.scope {
            CaptureScope::Account => "select exists(select 1 from provider_accounts where id=$1)",
            CaptureScope::Group => "select exists(select 1 from account_groups where id=$1)",
            CaptureScope::Key => "select exists(select 1 from client_api_keys where id=$1)",
        };
        let exists: bool = sqlx::query_scalar(query)
            .bind(&input.target_id)
            .fetch_one(&self.pool)
            .await
            .map_err(unavailable)?;
        if !exists {
            return Err(missing());
        }
        let task = CaptureTask {
            id: Uuid::new_v4().to_string(),
            scope: input.scope,
            target_id: input.target_id,
            include_media: input.include_media,
            started_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(i64::from(input.minutes)),
            status: CaptureTaskStatus::Running,
        };
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("insert into request_capture_tasks(id,instance_id,task,expires_at) values($1::text::uuid,$2::text::uuid,$3,$4)")
            .bind(&task.id).bind(&self.instance_id).bind(serde_json::to_value(&task).map_err(unavailable)?)
            .bind(task.expires_at).execute(&mut *tx).await.map_err(unavailable)?;
        crate::postgres::insert_admin_audit_event(
            &mut tx,
            crate::mutation_audit(
                context,
                "request_capture.create",
                "request_capture",
                &task.id,
                vec![],
            ),
        )
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        self.shared
            .control
            .write()
            .map_err(unavailable)?
            .tasks
            .push(Arc::new(ActiveTask {
                task: task.clone(),
                stopped: AtomicBool::new(false),
            }));
        Ok(task)
    }

    async fn stop(&self, id: &str, context: &MutationContext) -> AdminStoreResult<()> {
        let id = uuid(id)?;
        let _io = self.io.lock().await;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let result = sqlx::query("update request_capture_tasks set task=jsonb_set(task,'{status}','\"stopped\"') where id::text=$1 and instance_id::text=$2")
            .bind(&id).bind(&self.instance_id).execute(&mut *tx).await.map_err(unavailable)?;
        if result.rows_affected() == 0 {
            return Err(missing());
        }
        crate::postgres::insert_admin_audit_event(
            &mut tx,
            crate::mutation_audit(
                context,
                "request_capture.stop",
                "request_capture",
                &id,
                vec![],
            ),
        )
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        let mut control = self.shared.control.write().map_err(unavailable)?;
        control.tasks.retain(|task| {
            if task.task.id != id {
                true
            } else {
                task.stopped.store(true, Ordering::Release);
                false
            }
        });
        Ok(())
    }

    async fn delete(&self, id: &str, context: &MutationContext) -> AdminStoreResult<()> {
        self.stop(id, context).await?;
        let _io = self.io.lock().await;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let records: Vec<String> = sqlx::query_scalar(
            "select id::text from request_capture_records where task_id::text=$1",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        sqlx::query("delete from request_capture_tasks where id::text=$1 and instance_id::text=$2")
            .bind(id)
            .bind(&self.instance_id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        crate::postgres::insert_admin_audit_event(
            &mut tx,
            crate::mutation_audit(
                context,
                "request_capture.delete",
                "request_capture",
                id,
                vec![],
            ),
        )
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        for record in records {
            match tokio::fs::remove_file(self.file(&record)?).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    self.fault();
                    return Err(unavailable(error));
                }
            }
        }
        Ok(())
    }

    async fn read(
        &self,
        id: &str,
        offset: u64,
        context: &MutationContext,
    ) -> AdminStoreResult<CapturePage> {
        let _io = self.io.lock().await;
        self.enabled()?;
        let mut file = self.record_file(id).await?;
        self.audit("request_capture.read", id, context).await?;
        let size = file.metadata().await.map_err(unavailable)?.len();
        if offset > size {
            return Err(missing());
        }
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(unavailable)?;
        let mut bytes = Vec::new();
        file.take(256 * 1024)
            .read_to_end(&mut bytes)
            .await
            .map_err(unavailable)?;
        // Pages end on a UTF-8 boundary; arbitrary offsets inside a character are rejected.
        let length = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(error) => return Err(unavailable(error)),
        };
        bytes.truncate(length);
        let next = offset + length as u64;
        Ok(CapturePage {
            text: String::from_utf8(bytes).map_err(unavailable)?,
            next_offset: (next < size).then_some(next),
        })
    }

    async fn export(&self, id: &str, context: &MutationContext) -> AdminStoreResult<CaptureExport> {
        let _io = self.io.lock().await;
        self.enabled()?;
        let permit = self
            .exports
            .clone()
            .try_acquire_owned()
            .map_err(|_| conflict())?;
        let mut file = self.record_file(id).await?;
        self.audit("request_capture.export", id, context).await?;
        Ok(Box::pin(async_stream::try_stream! {
            let _permit = permit;
            loop {
                let mut bytes = vec![0; 64 * 1024];
                let read = file.read(&mut bytes).await.map_err(unavailable)?;
                if read == 0 { break; }
                bytes.truncate(read);
                yield bytes;
            }
        }))
    }

    async fn export_task(
        &self,
        id: &str,
        context: &MutationContext,
    ) -> AdminStoreResult<CaptureExport> {
        let id = uuid(id)?;
        let _io = self.io.lock().await;
        self.enabled()?;
        let permit = self
            .exports
            .clone()
            .try_acquire_owned()
            .map_err(|_| conflict())?;
        let task: Value = sqlx::query_scalar(
            "select task from request_capture_tasks where id::text=$1 and instance_id::text=$2",
        )
        .bind(&id)
        .bind(&self.instance_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?
        .ok_or_else(missing)?;
        if task["status"] == "running" {
            return Err(conflict());
        }
        self.audit("request_capture.export_task", &id, context)
            .await?;
        let pool = self.pool.clone();
        let directory = self.directory.clone();
        let instance = self.instance_id.clone();
        let boundary = Utc::now();
        Ok(Box::pin(async_stream::try_stream! {
            let _permit = permit;
            let mut header = serde_json::to_vec(&json!({"schemaVersion":1,
                "instanceId":instance,"task":task,"snapshotAt":boundary})).map_err(unavailable)?;
            header.push(b'\n');
            yield header;
            let mut after: Option<String> = None;
            loop {
                let records: Vec<(String, Value)> = sqlx::query_as(
                    "select id::text,record from request_capture_records
                     where task_id::text=$1 and ($2::text is null or id::text>$2)
                       and created_at<=$3 order by id::text limit 128",
                ).bind(&id).bind(&after).bind(boundary).fetch_all(&pool).await.map_err(unavailable)?;
                if records.is_empty() { break; }
                for (record_id, record) in records {
                    let safe_id = uuid(&record_id)?;
                    let path = directory.join(format!("{safe_id}.jsonl"));
                    let meta = tokio::fs::symlink_metadata(&path).await.map_err(unavailable)?;
                    if !meta.is_file() || meta.file_type().is_symlink() {
                        Err(unavailable("invalid export file"))?;
                    }
                    let mut file = tokio::fs::File::open(path).await.map_err(unavailable)?;
                    let mut header = serde_json::to_vec(&json!({"record":record})).map_err(unavailable)?;
                    header.push(b'\n');
                    yield header;
                    loop {
                        let mut bytes = vec![0;64 * 1024];
                        let read = file.read(&mut bytes).await.map_err(unavailable)?;
                        if read == 0 { break; }
                        bytes.truncate(read);
                        yield bytes;
                    }
                    after = Some(record_id);
                }
            }
        }))
    }
}
