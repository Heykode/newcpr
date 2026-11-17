use super::*;
use crate::model::{MutationActor, relogin::ReloginRequest};
use futures::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use gateway_core::{
    account::AccountErrorReason,
    task::{
        ScheduledTask, WorkerContribution, WorkerCycleContext, WorkerId, WorkerKind,
        WorkerRegistration, WorkerRunnable, WorkerSchedule, WorkerTaskError,
    },
};
use std::time::Duration;

const RELOGIN_SCAN_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) fn contribution(
    service: Arc<DefaultReloginService>,
) -> Result<WorkerContribution, AdminError> {
    let id = WorkerId::try_new(WorkerKind::OAuthRefresh, "admin_relogin")
        .map_err(|_| AdminError::internal("重登 worker ID 不合法"))?;
    let schedule = WorkerSchedule::try_new(
        RELOGIN_SCAN_INTERVAL,
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
    // 调用方筛选有效 2FA 和自动重登开关；refresh token 不应阻止失效重登。
    // OAuth refresh 仍可恢复账号，推送前会重查账号状态和凭据版本。
    account.enabled
        && account.authentication_kind == "oauth"
        && account.credential_state == CredentialState::Expired
        && matches!(
            account.last_error_reason,
            Some(AccountErrorReason::AccessTokenExpired | AccountErrorReason::CredentialExpired)
        )
}

impl DefaultReloginService {
    async fn cycle(&self, shutdown: &CancellationToken) -> Result<(), AdminError> {
        let owner = Arc::new(());
        let mut active = FuturesUnordered::new();
        let mut scan = tokio::time::interval_at(
            tokio::time::Instant::now() + RELOGIN_SCAN_INTERVAL,
            RELOGIN_SCAN_INTERVAL,
        );
        scan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut failure = None;
        loop {
            if failure.is_none() {
                // Settlement may hold gate across an await. Keep polling exchanges
                // while claiming; awaiting claim alone could deadlock its lock owner.
                let claim = self.claim(shutdown, &owner);
                tokio::pin!(claim);
                let claimed = loop {
                    tokio::select! {
                        result = &mut claim => break result,
                        outcome = active.next(), if !active.is_empty() => {
                            if let Some(Err(error)) = outcome {
                                failure = Some(error);
                            }
                        }
                    }
                };
                match claimed {
                    Ok(jobs) => {
                        for (entry, request, cancellation) in jobs {
                            active.push(self.execute(entry, request, cancellation, shutdown));
                        }
                    }
                    Err(error) => failure = Some(error),
                }
            }
            if active.is_empty() {
                return failure.map_or(Ok(()), Err);
            }
            tokio::select! {
                outcome = active.next() => {
                    if let Some(Err(error)) = outcome {
                        failure = Some(error);
                    }
                    if active.is_empty() {
                        return failure.map_or(Ok(()), Err);
                    }
                }
                _ = scan.tick(), if failure.is_none() => {},
            }
        }
    }

    async fn claim(
        &self,
        shutdown: &CancellationToken,
        owner: &Arc<()>,
    ) -> Result<Vec<(ReloginEntry, ReloginRequest, CancellationToken)>, AdminError> {
        let jobs = {
            let mut gate = self.gate.lock().await;
            // Dropped/panicked cycles own no live exchanges. User cancellation alone
            // does not free a slot until its exchange has actually finished.
            gate.active
                .retain(|_, active| active.owner.upgrade().is_some());
            let settings = self.store()?.settings().await.map_err(store_error)?;
            let mut entries = self.entries().await?;
            let pool = self.pool().await?;
            // No jobs survive process shutdown. Never leave a persisted running row stuck forever.
            for entry in &mut entries {
                if entry.maintenance.view.state == "running"
                    && !gate.active.contains_key(&entry.email)
                {
                    entry.maintenance.uncertain |= entry.maintenance.view.action.is_some_and(
                        crate::model::relogin_maintenance::MaintenanceAction::destructive,
                    );
                    entry.maintenance.view.state = if entry.maintenance.view.action.is_some_and(
                        crate::model::relogin_maintenance::MaintenanceAction::destructive,
                    ) {
                        "uncertain"
                    } else {
                        "failed"
                    }
                    .to_owned();
                    entry.maintenance.view.message = "上次维护中断，请核对结果".to_owned();
                    entry.maintenance.queued_login = None;
                    let attempt = if entry
                        .maintenance
                        .view
                        .action
                        .is_some_and(|action| action.authorizes())
                    {
                        &mut entry.last_attempt
                    } else {
                        &mut entry.maintenance.last_activity
                    };
                    if let Some(attempt) = attempt
                        && attempt.operation.is_some()
                    {
                        attempt.result = ReloginAttemptResult::Failed;
                        attempt.finished_at = Some(Utc::now());
                        attempt.message = entry.maintenance.view.message.clone();
                    }
                    self.save(entry).await?;
                }

                if entry.status == ReloginStatus::Pushing && !gate.active.contains_key(&entry.email)
                {
                    entry.status = ReloginStatus::Uncertain;
                    entry.message = "上次推送结果未确认，请检查号池并重新获取凭证".to_owned();
                    self.save(entry).await?;
                }
                if entry.status == ReloginStatus::Running && !gate.active.contains_key(&entry.email)
                {
                    entry.status = ReloginStatus::Failed;
                    entry.message = "上次重登已中断，未确认推送结果".to_owned();
                    entry.next_attempt_at = Some(
                        Utc::now()
                            + chrono::Duration::seconds(i64::from(settings.retry_interval_seconds)),
                    );
                    self.save(entry).await?;
                }
            }
            if settings.paused || shutdown.is_cancelled() {
                return Ok(Vec::new());
            }
            let mut jobs = Vec::new();
            for mut entry in entries {
                if entry.maintenance.blocks_login() || entry.maintenance.login_paused {
                    continue;
                }
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
                    entry.maintenance.queued_login = None;
                    if entry.status == ReloginStatus::Ready
                        && entry.synced_at.is_none()
                        && let Some(enrollment) = entry.enrollment.clone()
                    {
                        if let Err(error) = self.push_entry(&mut entry, &enrollment.context).await {
                            if entry.status == ReloginStatus::Ready {
                                entry.status = ReloginStatus::Failed;
                            }
                            entry.message = format!("入池未完成：{}", error.message());
                            self.save(&mut entry).await?;
                        }
                        continue;
                    }
                    if !recovery::view(&entry, &pool, &settings, Utc::now()).waiting() {
                        continue;
                    }
                    if entry.enrollment.is_some() && entry.target.is_none() {
                        // Older failed enrollments did not count their first login.
                        entry.automatic_attempts = entry.automatic_attempts.max(1);
                    } else {
                        let target = match select_target(&entry, &pool) {
                            Ok(Some(target)) => target,
                            Ok(None) | Err(_) => continue,
                        };
                        let Some(account) = pool.iter().find(|account| {
                            account.id == target.account_id && needs_relogin(account)
                        }) else {
                            continue;
                        };
                        if entry.synced_at.is_some() || !recovery::same_attempt(&entry, account) {
                            entry.automatic_attempts = 0;
                            entry.next_attempt_at = None;
                        }
                        entry.target = Some(ReloginTarget::from_account(account)?);
                        entry.automatic_job = true;
                        entry.workspace_mode = ReloginWorkspaceMode::Original;
                        entry.workspace_targets.clear();
                        entry.workspace_choices.clear();
                        entry.selected_workspace_id = None;
                        entry.manual_push_context = None;
                        entry.clear_enrollment();
                    }
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
                entry.begin_attempt();
                if let (Some(target), Some(account)) = (&entry.target, target_account)
                    && !target.matches_account(account)
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
                if entry.workspace_mode == ReloginWorkspaceMode::Highest {
                    let candidates: Vec<_> = entry
                        .workspace_targets
                        .iter()
                        .filter_map(|target| {
                            pool.iter().find(|account| {
                                target.matches_account(account)
                                    && account.email.as_ref().is_some_and(|email| {
                                        email.eq_ignore_ascii_case(&entry.email)
                                    })
                            })
                        })
                        .collect();
                    if candidates.len() != entry.workspace_targets.len()
                        || (entry
                            .maintenance
                            .queued_login
                            .as_ref()
                            .and_then(|queued| queued.route.as_ref())
                            .is_none()
                            && candidates.first().is_some_and(|first| {
                                candidates.iter().any(|account| {
                                    account.effective_outbound_proxy()
                                        != first.effective_outbound_proxy()
                                })
                            }))
                    {
                        entry.status = ReloginStatus::Failed;
                        entry.message = "原账号或代理已变化，请重新发起重登".to_owned();
                        self.save(&mut entry).await?;
                        continue;
                    }
                }
                if entry.automatic_job || (entry.enrollment.is_some() && entry.target.is_none()) {
                    entry.automatic_attempts += 1;
                }
                let workspace_id = if entry.workspace_mode == ReloginWorkspaceMode::Highest {
                    entry.selected_workspace_id.clone()
                } else {
                    entry
                        .target
                        .as_ref()
                        .map(|target| target.workspace_id.clone())
                        .or_else(|| entry.selected_workspace_id.clone())
                        .or_else(|| entry.preferred_workspace_id.clone())
                };
                let mut route_entry = entry.clone();
                if let Some(route) = entry
                    .maintenance
                    .queued_login
                    .as_ref()
                    .and_then(|queued| queued.route.as_ref())
                {
                    if !pool.iter().any(|account| route.matches_account(account)) {
                        entry.status = ReloginStatus::Failed;
                        entry.message = "原账号出口归属已变化，请重新发起重登".to_owned();
                        self.save(&mut entry).await?;
                        continue;
                    }
                    route_entry.target = Some(route.clone());
                }
                let request = match self.login_request(&route_entry, &pool, workspace_id).await {
                    Ok(request) => request,
                    Err(error) => {
                        entry.status = ReloginStatus::Failed;
                        entry.message = error.message().to_owned();
                        entry.next_attempt_at = Some(
                            Utc::now()
                                + chrono::Duration::seconds(i64::from(
                                    settings.retry_interval_seconds,
                                )),
                        );
                        self.save(&mut entry).await?;
                        continue;
                    }
                };
                if entry.automatic_job {
                    let now = Utc::now();
                    entry.automatic_started_at = recovery::recent_starts(&entry, now);
                    entry.automatic_started_at.push(now);
                }
                entry.attempted_target = entry.target.clone();
                entry.stop_reason = None;
                entry.synced_at = None;
                entry.credential = None;
                entry.status = ReloginStatus::Running;
                entry.message = "正在登录并验证工作区".to_owned();
                if let Some(queued) = &entry.maintenance.queued_login {
                    entry.maintenance.view.action = Some(queued.action.action());
                    entry.maintenance.view.state = "running".to_owned();
                    entry.maintenance.view.message =
                        format!("正在{}", queued.action.action().label());
                    entry.maintenance.view.checked_at = Some(Utc::now());
                }
                self.save(&mut entry).await?;
                let cancellation = CancellationToken::new();
                jobs.push((entry, request, cancellation));
            }
            // Publish in-memory occupancy only after all fallible claim writes complete.
            for (entry, _, cancellation) in &jobs {
                gate.active.insert(
                    entry.email.clone(),
                    ActiveLogin {
                        cancellation: cancellation.clone(),
                        owner: Arc::downgrade(owner),
                    },
                );
            }
            jobs
        };
        Ok(jobs)
    }

    async fn execute(
        &self,
        entry: ReloginEntry,
        request: ReloginRequest,
        cancellation: CancellationToken,
        shutdown: &CancellationToken,
    ) -> Result<(), AdminError> {
        if let Some(queued) = &entry.maintenance.queued_login {
            let action = queued.action.action();
            let request = crate::model::relogin_maintenance::MaintenanceRequest {
                target: entry.selected_workspace_id.clone(),
                login: request,
                action,
                device_id_hash: None,
                pending_totp: None,
            };
            let outcome = tokio::select! {
                biased;
                () = shutdown.cancelled() => Err(AdminError::conflict("服务正在停止，任务已取消")),
                () = cancellation.cancelled() => Err(AdminError::conflict("重登任务已取消")),
                outcome = tokio::time::timeout(Duration::from_secs(300), self.provider.maintain_relogin(request)) => {
                    match outcome {
                        Ok(outcome) => outcome.map_err(|error| map_provider_error(error, "relogin maintenance")),
                        Err(_) => Err(AdminError::unavailable("重登超时，请检查网络和代理")),
                    }
                }
            };
            return self
                .finish_maintenance(&entry, action, &cancellation, outcome)
                .await
                .map(|_| ());
        }
        let workspace_locked = request.workspace_id.is_some();
        let outcome = tokio::select! {
            biased;
            () = shutdown.cancelled() => Err((AdminError::conflict("服务正在停止，任务已取消"), None, Vec::new(), None)),
            () = cancellation.cancelled() => Err((AdminError::conflict("重登任务已取消"), None, Vec::new(), None)),
            outcome = tokio::time::timeout(Duration::from_secs(300), self.provider.relogin(request)) => {
                match outcome {
                    Ok(outcome) => outcome.map_err(|error| {
                        let reason = error.relogin_stop_reason();
                        let choices = error.relogin_workspace_choices().to_vec();
                        let session = error.relogin_web_session().cloned();
                        (map_provider_error(error, "relogin"), reason, choices, session)
                    }),
                    Err(_) => Err((AdminError::unavailable("重登超时，请检查网络和代理"), None, Vec::new(), None)),
                }
            }
        };
        let mut gate = self.gate.lock().await;
        gate.active.remove(&entry.email);
        let Some(mut current) = self
            .entries()
            .await?
            .into_iter()
            .find(|current| current.id == entry.id)
        else {
            return Ok(());
        };
        if current.revision != entry.revision || cancellation.is_cancelled() {
            return Ok(());
        }
        let retry_interval = chrono::Duration::seconds(i64::from(
            self.store()?
                .settings()
                .await
                .map_err(store_error)?
                .retry_interval_seconds,
        ));
        current.workspace_choices.clear();
        match outcome {
            Ok(credential) => {
                if !credential.email.eq_ignore_ascii_case(&current.email)
                    || current
                        .selected_workspace_id
                        .as_ref()
                        .is_some_and(|workspace| workspace != &credential.workspace_id)
                    || current.target.as_ref().is_some_and(|target| {
                        target.user_id != credential.user_id
                            || target.workspace_id != credential.workspace_id
                    })
                    || (current.manual_push_context.is_none()
                        && current.workspace_mode == ReloginWorkspaceMode::Original
                        && current
                            .preferred_workspace_id
                            .as_ref()
                            .is_some_and(|workspace| workspace != &credential.workspace_id))
                    || (current.workspace_mode == ReloginWorkspaceMode::Highest
                        && (credential.user_id.is_empty()
                            || current
                                .workspace_targets
                                .iter()
                                .any(|target| target.user_id != credential.user_id)))
                {
                    current.status = ReloginStatus::Failed;
                    current.message = "新凭据身份或工作区不一致，未推送".to_owned();
                } else {
                    if let Some(session) = &credential.web_session {
                        current.maintenance.session = Some(session.clone());
                        current.maintenance.view.session_state = "valid".to_owned();
                        current.maintenance.view.session_checked_at = Some(session.verified_at);
                    }
                    current.credential = Some(credential);
                    current.synced_at = None;
                    current.status = ReloginStatus::Ready;
                    current.message = "已获取新 JSON，凭证验证通过".to_owned();
                }
            }
            Err((error, reason, choices, session)) => {
                if let Some(session) =
                    session.filter(|session| session.email.eq_ignore_ascii_case(&current.email))
                {
                    current.maintenance.view.session_state = "valid".to_owned();
                    current.maintenance.view.session_checked_at = Some(session.verified_at);
                    current.maintenance.session = Some(session);
                }
                if reason.is_none()
                    && !workspace_locked
                    && !current.automatic_job
                    && current.manual_push_context.is_none()
                    && current.target.is_none()
                    && validate_workspace_choices(&choices).is_ok()
                {
                    current.status = ReloginStatus::AwaitingWorkspace;
                    current.message = "有多个同级工作区，请选择后继续".to_owned();
                    current.workspace_choices = choices;
                } else {
                    current.status = ReloginStatus::Failed;
                    current.message = error.message().to_owned();
                    current.stop_reason = reason;
                }
            }
        }
        current.next_attempt_at = if current.status == ReloginStatus::Failed
            && current.automatic_stop_reason().is_none()
        {
            Some(Utc::now() + retry_interval)
        } else {
            None
        };
        self.save(&mut current).await?;
        if current.status == ReloginStatus::Ready
            && ((current.automatic_job && current.automatic)
                || current.manual_push_context.is_some()
                || current.enrollment.is_some())
            && !shutdown.is_cancelled()
            && !self.store()?.settings().await.map_err(store_error)?.paused
        {
            let context = current
                .manual_push_context
                .clone()
                .or_else(|| {
                    current
                        .enrollment
                        .as_ref()
                        .map(|enrollment| enrollment.context.clone())
                })
                .unwrap_or_else(|| MutationContext {
                    actor: MutationActor::System,
                    request_id: format!("relogin_{}", uuid::Uuid::now_v7()),
                });
            if let Err(error) = self.push_entry(&mut current, &context).await {
                current.next_attempt_at = Some(Utc::now() + retry_interval);
                if current.status == ReloginStatus::Ready
                    && (current.manual_push_context.is_some() || current.enrollment.is_some())
                {
                    current.status = ReloginStatus::Failed;
                }
                current.message = format!("新凭据已保存，推送未完成：{}", error.message());
                self.save(&mut current).await?;
            }
        }
        Ok(())
    }
}
