use super::*;
use crate::model::{MutationActor, relogin::ReloginRequest};
use futures::{future::BoxFuture, future::join_all};
use gateway_core::{
    account::AccountErrorReason,
    task::{
        ScheduledTask, WorkerContribution, WorkerCycleContext, WorkerId, WorkerKind,
        WorkerRegistration, WorkerRunnable, WorkerSchedule, WorkerTaskError,
    },
};
use std::time::Duration;

pub(crate) fn contribution(
    service: Arc<DefaultReloginService>,
) -> Result<WorkerContribution, AdminError> {
    let id = WorkerId::try_new(WorkerKind::OAuthRefresh, "admin_relogin")
        .map_err(|_| AdminError::internal("重登 worker ID 不合法"))?;
    let schedule = WorkerSchedule::try_new(
        Duration::from_secs(2),
        Duration::from_secs(2),
        Duration::from_secs(30),
        Duration::from_secs(600),
        Duration::from_secs(60),
    )
    .map_err(|_| AdminError::internal("重登 worker 时序不合法"))?;
    Ok(WorkerContribution::Registration(
        WorkerRegistration::try_new(
            id,
            WorkerRunnable::Scheduled {
                schedule,
                lease: None,
                task: Box::new(ReloginWorker(service)),
            },
        )
        .map_err(|_| AdminError::internal("重登 worker 注册失败"))?,
    ))
}

struct ReloginWorker(Arc<DefaultReloginService>);
impl ScheduledTask for ReloginWorker {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            if self.0.store.is_none() {
                return Ok(());
            }
            self.0
                .cycle(context.cancellation())
                .await
                .map_err(|_| WorkerTaskError::safe("relogin cycle failed"))
        })
    }
}

pub fn needs_relogin(account: &AccountRecord) -> bool {
    account.enabled
        && account.authentication_kind == "oauth"
        && account.credential_state == CredentialState::Expired
        && !(account.has_refresh_token
            && account.last_error_reason == Some(AccountErrorReason::AccessTokenExpired))
        && matches!(
            account.last_error_reason,
            Some(AccountErrorReason::AccessTokenExpired | AccountErrorReason::CredentialExpired)
        )
}

impl DefaultReloginService {
    async fn cycle(&self, shutdown: &CancellationToken) -> Result<(), AdminError> {
        let jobs = {
            let mut gate = self.gate.lock().await;
            let settings = self.store()?.settings().await.map_err(store_error)?;
            let mut entries = self.entries().await?;
            let pool = self.pool().await?;
            // No jobs survive process shutdown. Never leave a persisted running row stuck forever.
            for entry in &mut entries {
                if entry.status == ReloginStatus::Pushing {
                    entry.status = ReloginStatus::Uncertain;
                    entry.message = "上次推送结果未确认，请检查号池并重新获取凭证".to_owned();
                    self.save(entry).await?;
                }
                if entry.status == ReloginStatus::Running && !gate.active.contains_key(&entry.email)
                {
                    entry.status = ReloginStatus::Failed;
                    entry.message = "上次重登已中断，未确认推送结果".to_owned();
                    entry.next_attempt_at = Some(Utc::now() + chrono::Duration::minutes(5));
                    self.save(entry).await?;
                }
            }
            if settings.paused || shutdown.is_cancelled() {
                return Ok(());
            }
            let mut jobs = Vec::new();
            for mut entry in entries {
                if entry.validate_totp().is_err() {
                    continue;
                }
                if gate.active.len() + jobs.len() >= settings.concurrency {
                    break;
                }
                if gate.active.contains_key(&entry.email)
                    || matches!(
                        entry.status,
                        ReloginStatus::Running | ReloginStatus::Uncertain
                    )
                {
                    continue;
                }
                if entry.status != ReloginStatus::Queued {
                    if !entry.automatic || entry.next_attempt_at.is_some_and(|at| at > Utc::now()) {
                        continue;
                    }
                    let target = match select_target(&entry, &pool) {
                        Ok(Some(target)) => target,
                        Ok(None) | Err(_) => continue,
                    };
                    let Some(account) = pool
                        .iter()
                        .find(|account| account.id == target.account_id && needs_relogin(account))
                    else {
                        continue;
                    };
                    if entry.attempted_target.as_ref() != Some(&target) {
                        entry.automatic_attempts = 0;
                    }
                    if entry.automatic_attempts >= 3 {
                        continue;
                    }
                    entry.target = Some(ReloginTarget::from_account(account)?);
                    entry.automatic_job = true;
                    entry.manual_push_context = None;
                }
                let target_account = entry
                    .target
                    .as_ref()
                    .and_then(|target| pool.iter().find(|account| account.id == target.account_id));
                if entry.automatic_job
                    && (!entry.automatic
                        || target_account.is_none_or(|account| !needs_relogin(account)))
                {
                    entry.status = ReloginStatus::Pending;
                    entry.message = "账号已恢复或自动重登已关闭".to_owned();
                    self.save(&mut entry).await?;
                    continue;
                }
                if let (Some(target), Some(account)) = (&entry.target, target_account)
                    && target.credential_revision != account.credential_revision.get()
                {
                    entry.status = ReloginStatus::Failed;
                    entry.message = "原凭据已变化，请重新发起重登".to_owned();
                    self.save(&mut entry).await?;
                    continue;
                }
                if entry.target.is_some() && target_account.is_none() {
                    entry.status = ReloginStatus::Failed;
                    entry.message = "原账号已删除".to_owned();
                    self.save(&mut entry).await?;
                    continue;
                }
                let request = ReloginRequest {
                    email: entry.email.clone(),
                    password: entry.password.clone(),
                    mfa_secret: entry.mfa_secret.clone(),
                    workspace_id: entry
                        .target
                        .as_ref()
                        .map(|target| target.workspace_id.clone())
                        .or_else(|| entry.preferred_workspace_id.clone()),
                    outbound_proxy: target_account
                        .and_then(|account| account.outbound_proxy.clone()),
                };
                if entry.automatic_job {
                    entry.automatic_attempts += 1;
                    entry.attempted_target = entry.target.clone();
                }
                entry.status = ReloginStatus::Running;
                entry.message = "正在登录并验证工作区".to_owned();
                self.save(&mut entry).await?;
                let cancellation = CancellationToken::new();
                jobs.push((entry, request, cancellation));
            }
            // Publish in-memory occupancy only after all fallible claim writes complete.
            for (entry, _, cancellation) in &jobs {
                gate.active
                    .insert(entry.email.clone(), cancellation.clone());
            }
            jobs
        };
        let outcomes = join_all(jobs.into_iter().map(|(entry, request, cancellation)| async move {
            let outcome = tokio::select! {
                biased;
                () = shutdown.cancelled() => Err(AdminError::conflict("服务正在停止，任务已取消")),
                () = cancellation.cancelled() => Err(AdminError::conflict("重登任务已取消")),
                outcome = tokio::time::timeout(Duration::from_secs(300), self.provider.relogin(request)) => {
                    match outcome {
                        Ok(outcome) => outcome.map_err(|error| map_provider_error(error, "relogin")),
                        Err(_) => Err(AdminError::unavailable("重登超时，请检查网络和代理")),
                    }
                }
            };
            let mut gate = self.gate.lock().await;
            gate.active.remove(&entry.email);
            let Some(mut current) = self.entries().await?.into_iter().find(|current| current.id == entry.id) else { return Ok(()) };
            if current.revision != entry.revision || cancellation.is_cancelled() { return Ok(()); }
            match outcome {
                Ok(credential) => {
                    if !credential.email.eq_ignore_ascii_case(&current.email)
                        || current.target.as_ref().is_some_and(|target| target.user_id != credential.user_id || target.workspace_id != credential.workspace_id)
                        || (current.manual_push_context.is_none() && current.preferred_workspace_id.as_ref().is_some_and(|workspace| workspace != &credential.workspace_id))
                    {
                        current.status = ReloginStatus::Failed;
                        current.message = "新凭据身份或工作区不一致，未推送".to_owned();
                    } else {
                        current.credential = Some(credential);
                        current.synced_at = None;
                        current.status = ReloginStatus::Ready;
                        current.message = "已获取新 JSON，凭证验证通过".to_owned();
                    }
                }
                Err(error) => {
                    current.status = ReloginStatus::Failed;
                    current.message = error.message().to_owned();
                }
            }
            current.next_attempt_at = Some(Utc::now() + chrono::Duration::minutes(5 * i64::from(current.automatic_attempts.max(1))));
            self.save(&mut current).await?;
            if current.status == ReloginStatus::Ready
                && ((current.automatic_job && current.automatic) || current.manual_push_context.is_some())
                && !shutdown.is_cancelled() && !self.store()?.settings().await.map_err(store_error)?.paused
            {
                let context = current.manual_push_context.clone().unwrap_or_else(|| MutationContext { actor: MutationActor::System, request_id: format!("relogin_{}", uuid::Uuid::now_v7()) });
                if let Err(error) = self.push_entry(&mut current, &context).await {
                    if current.status == ReloginStatus::Ready && current.manual_push_context.is_some() {
                        current.status = ReloginStatus::Failed;
                    }
                    current.message = format!("新凭据已保存，推送未完成：{}", error.message());
                    self.save(&mut current).await?;
                }
            }
            Ok::<_, AdminError>(())
        })).await;
        for outcome in outcomes {
            outcome?;
        }
        Ok(())
    }
}
