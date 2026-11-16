use super::*;
use crate::model::relogin_maintenance::*;

impl DefaultReloginService {
    pub(super) async fn maintain_entry(
        &self,
        command: MaintenanceCommand,
        context: &MutationContext,
    ) -> Result<MaintenanceView, AdminError> {
        validate_ids(std::slice::from_ref(&command.id))?;
        if command.action == MaintenanceAction::EditMaterial {
            return Err(AdminError::invalid("请使用登录资料编辑入口"));
        }
        if command.action.destructive() && command.confirm.as_deref() != Some("maintain_account") {
            return Err(AdminError::invalid("请确认此操作对账号登录状态的影响"));
        }
        if [&command.target, &command.device_id_hash]
            .into_iter()
            .flatten()
            .any(|value| {
                value.is_empty() || value.len() > 256 || value.chars().any(char::is_control)
            })
        {
            return Err(AdminError::invalid("维护目标不合法"));
        }
        let owner = Arc::new(());
        let cancellation = CancellationToken::new();
        let (entry, request) = {
            let mut gate = self.gate.lock().await;
            gate.active
                .retain(|_, active| active.owner.upgrade().is_some());
            let settings = self.store()?.settings().await.map_err(store_error)?;
            let mut entry = self
                .entries()
                .await?
                .into_iter()
                .find(|entry| entry.id == command.id)
                .ok_or_else(|| AdminError::not_found("账号资料不存在"))?;
            if entry.revision != command.revision {
                return Err(AdminError::conflict("资料已变化，请刷新后重试"));
            }
            if entry.maintenance.view.state == "running"
                || entry.status.active()
                || entry.status == ReloginStatus::Uncertain
                || gate.active.contains_key(&entry.email)
            {
                return Err(AdminError::conflict("账号有活动任务或推送待确认"));
            }
            if settings.paused || gate.active.len() >= settings.concurrency {
                return Err(AdminError::conflict("维护队列已暂停或并发已满，请稍后重试"));
            }
            if entry.maintenance.uncertain && command.action.destructive() {
                return Err(AdminError::conflict(
                    "上次维护结果待核实，请先核对安全状态或设备",
                ));
            }
            if entry.maintenance.pending_totp.is_some()
                && matches!(
                    command.action,
                    MaintenanceAction::PrepareTotp | MaintenanceAction::DisableTotp
                )
            {
                return Err(AdminError::conflict("已有待激活密钥，请先激活或人工核实"));
            }
            let pool = self.pool().await?;
            let route_entry = entry.clone();
            if command.action.authorizes() {
                if entry.maintenance.blocks_login() {
                    return Err(AdminError::conflict("请先核实并完成安全维护"));
                }
                entry.maintenance.login_paused = false;
                entry.validate_totp()?;
                let mode = if command.action == MaintenanceAction::SwitchWorkspace {
                    ReloginWorkspaceMode::Highest
                } else {
                    entry.workspace_mode
                };
                workspace::prepare_queue(&mut entry, &pool, mode)?;
                if command.action == MaintenanceAction::SwitchWorkspace {
                    let target = command
                        .target
                        .as_ref()
                        .ok_or_else(|| AdminError::invalid("请选择工作区"))?;
                    if !entry
                        .maintenance
                        .view
                        .workspaces
                        .iter()
                        .any(|w| &w.id == target && w.available)
                    {
                        return Err(AdminError::conflict("请重新获取可用工作区"));
                    }
                    entry.selected_workspace_id = Some(target.clone());
                }
                entry.automatic_job = false;
                entry.manual_push_context = None;
                entry.credential = None;
                entry.synced_at = None;
                entry.status = ReloginStatus::Pending;
            }
            self.store()?
                .record_maintenance(&entry.id, Some(command.action), context)
                .await
                .map_err(store_error)?;
            let request = MaintenanceRequest {
                login: self
                    .login_request(
                        &route_entry,
                        &pool,
                        entry
                            .selected_workspace_id
                            .clone()
                            .or_else(|| {
                                entry
                                    .target
                                    .as_ref()
                                    .map(|target| target.workspace_id.clone())
                            })
                            .or_else(|| entry.preferred_workspace_id.clone()),
                    )
                    .await?,
                action: command.action,
                target: command.target,
                device_id_hash: command.device_id_hash,
                pending_totp: entry.maintenance.pending_totp.clone(),
            };
            let login_attempt = entry.last_attempt.take();
            entry.begin_attempt();
            if let Some(attempt) = &mut entry.last_attempt {
                attempt.operation = Some(command.action);
                attempt.push_result = ReloginPushResult::NotRequested;
            }
            if !command.action.authorizes() {
                entry.maintenance.last_activity = entry.last_attempt.take();
                entry.last_attempt = login_attempt;
            }
            entry.maintenance.view.action = Some(command.action);
            entry.maintenance.view.state = "running".to_owned();
            entry.maintenance.view.message = format!("正在{}", command.action.label());
            entry.maintenance.view.checked_at = Some(Utc::now());
            self.save(&mut entry).await?;
            gate.active.insert(
                entry.email.clone(),
                ActiveLogin {
                    cancellation: cancellation.clone(),
                    owner: Arc::downgrade(&owner),
                },
            );
            (entry, request)
        };
        let outcome = tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(AdminError::conflict("维护已取消")),
            outcome = tokio::time::timeout(std::time::Duration::from_secs(300), self.provider.maintain_relogin(request)) => {
                match outcome { Ok(outcome) => outcome.map_err(|error| map_provider_error(error, "relogin maintenance")), Err(_) => Err(AdminError::unavailable("维护超时")) }
            }
        };
        let mut gate = self.gate.lock().await;
        gate.active.remove(&entry.email);
        let mut current = self
            .entries()
            .await?
            .into_iter()
            .find(|row| row.id == entry.id)
            .ok_or_else(|| AdminError::not_found("资料已删除，已丢弃结果"))?;
        if current.revision != entry.revision || cancellation.is_cancelled() {
            return Err(AdminError::conflict(
                "资料已变化或维护已取消，已丢弃迟到结果",
            ));
        }
        // Validate before applying any part of a provider result; failed identity checks
        // still finish the durable attempt instead of leaving it running.
        let outcome = outcome.and_then(|outcome| {
            if outcome.credential.as_ref().is_some_and(|credential| {
                !credential.email.eq_ignore_ascii_case(&current.email)
                    || current.target.as_ref().is_some_and(|target| {
                        target.user_id != credential.user_id
                            || target.workspace_id != credential.workspace_id
                    })
                    || current
                        .workspace_targets
                        .iter()
                        .any(|target| target.user_id != credential.user_id)
                    || current
                        .selected_workspace_id
                        .as_ref()
                        .is_some_and(|id| id != &credential.workspace_id)
            }) || outcome
                .session
                .as_ref()
                .is_some_and(|session| !session.email.eq_ignore_ascii_case(&current.email))
            {
                return Err(AdminError::conflict("新凭据或会话身份不符，未保存"));
            }
            Ok(outcome)
        });
        match outcome {
            Ok(outcome) => {
                let succeeded = outcome.view.state == "succeeded";
                let previous = current.maintenance.view.clone();
                let mut view = outcome.view;
                // Unrelated reads do not erase established facts or uncertain mutation state.
                if view.session_state.is_empty() {
                    view.session_state = previous.session_state;
                }
                if view.session_checked_at.is_none() {
                    view.session_checked_at = previous.session_checked_at;
                }
                if view.state != "awaiting_workspace"
                    && (!succeeded
                        || !matches!(
                            command.action,
                            MaintenanceAction::Workspaces | MaintenanceAction::CheckSession
                        ))
                {
                    view.workspaces = previous.workspaces;
                }
                if command.action != MaintenanceAction::Devices || !succeeded {
                    view.devices = previous.devices;
                }
                if !matches!(
                    command.action,
                    MaintenanceAction::Security
                        | MaintenanceAction::DisableTotp
                        | MaintenanceAction::ActivateTotp
                ) || !succeeded
                {
                    view.totp_enabled = view.totp_enabled.or(previous.totp_enabled);
                    view.password_enabled = view.password_enabled.or(previous.password_enabled);
                }
                // A status probe cannot prove which TOTP secret was activated or a logout completed.
                if current.maintenance.uncertain {
                    view.state = "uncertain".to_owned();
                    view.message = "已读取状态，上次维护结果仍需核实".to_owned();
                }
                if let Some(session) = outcome.session {
                    current.maintenance.session = Some(session);
                }
                if succeeded
                    && matches!(
                        command.action,
                        MaintenanceAction::LogoutAll | MaintenanceAction::LogoutAndRelogin
                    )
                {
                    current.maintenance.session = None;
                    current.maintenance.login_paused =
                        command.action == MaintenanceAction::LogoutAll;
                    view.session_state = "absent".to_owned();
                    if command.action == MaintenanceAction::LogoutAndRelogin {
                        current.status = ReloginStatus::Queued;
                        current.message = "退出请求已受理，等待重新登录授权".to_owned();
                    }
                }
                if let Some(candidate) = outcome.pending_totp {
                    current.maintenance.pending_totp = Some(candidate);
                }
                if let Some(secret) = outcome.new_secret {
                    current.maintenance.material_history.push(MaterialVersion {
                        password: current.password.clone(),
                        mfa_secret: current.mfa_secret.clone(),
                        saved_at: Utc::now(),
                        pending_totp: None,
                    });
                    if current.maintenance.material_history.len() > 10 {
                        current.maintenance.material_history.remove(0);
                    }
                    current.mfa_secret = secret;
                    current.maintenance.pending_totp = None;
                }
                view.pending_totp = current.maintenance.pending_totp.is_some();
                if view.state == "awaiting_workspace" {
                    current.workspace_choices = view
                        .workspaces
                        .iter()
                        .map(|workspace| ReloginWorkspaceChoice {
                            id: workspace.id.clone(),
                            name: workspace.name.clone(),
                            plan_type: workspace.plan_type.clone().unwrap_or_default(),
                        })
                        .collect();
                    validate_workspace_choices(&current.workspace_choices)?;
                    current.status = ReloginStatus::AwaitingWorkspace;
                    current.message = view.message.clone();
                }
                if let Some(credential) = outcome.credential {
                    current.credential = Some(credential);
                    current.synced_at = None;
                    current.status = ReloginStatus::Ready;
                    current.stop_reason = None;
                    current.message = "已重新授权并验证，等待推送".to_owned();
                }
                if view.state == "uncertain" {
                    current.maintenance.uncertain = true;
                }
                current.maintenance.view = view;
            }
            Err(error) => {
                current.maintenance.uncertain |= command.action.destructive();
                current.maintenance.view.state = if command.action.destructive() {
                    "uncertain"
                } else {
                    "failed"
                }
                .to_owned();
                current.maintenance.view.message = error.message().to_owned();
            }
        }
        if command.action.authorizes()
            && matches!(
                current.maintenance.view.state.as_str(),
                "failed" | "uncertain"
            )
        {
            current.status = ReloginStatus::Failed;
            current.message = current.maintenance.view.message.clone();
        }
        let attempt = if command.action.authorizes() {
            &mut current.last_attempt
        } else {
            &mut current.maintenance.last_activity
        };
        if let Some(attempt) = attempt {
            attempt.result = if current.maintenance.view.state == "awaiting_workspace" {
                ReloginAttemptResult::AwaitingWorkspace
            } else if current.maintenance.view.state == "succeeded" {
                ReloginAttemptResult::Succeeded
            } else {
                ReloginAttemptResult::Failed
            };
            attempt.finished_at = Some(Utc::now());
            attempt.message = current.maintenance.view.message.clone();
        }
        current.maintenance.view.uncertain = current.maintenance.uncertain;
        current.maintenance.view.login_paused = current.maintenance.login_paused;
        self.save(&mut current).await?;
        Ok(current.maintenance.view)
    }

    pub(super) async fn cancel_maintenance(
        &self,
        id: &str,
        revision: u64,
    ) -> Result<(), AdminError> {
        let gate = self.gate.lock().await;
        let mut entry = self
            .entries()
            .await?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| AdminError::not_found("账号不存在"))?;
        if entry.revision != revision {
            return Err(AdminError::conflict("资料已变化，请刷新"));
        }
        Self::stop(&gate, &mut entry);
        if entry.maintenance.view.state == "running" {
            entry.maintenance.view.state = if entry
                .maintenance
                .view
                .action
                .is_some_and(MaintenanceAction::destructive)
            {
                "uncertain"
            } else {
                "cancelled"
            }
            .to_owned();
            entry.maintenance.view.message = "任务已取消，已提交的远端操作需单独核实".to_owned();
        }
        self.save(&mut entry).await
    }
}

impl DefaultReloginService {
    pub(super) async fn read_maintenance_material(
        &self,
        id: &str,
        context: &MutationContext,
    ) -> Result<MaintenanceMaterial, AdminError> {
        validate_ids(&[id.to_owned()])?;
        let entry = self
            .entries()
            .await?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| AdminError::not_found("资料不存在"))?;
        self.store()?
            .record_totp_access(id, context)
            .await
            .map_err(store_error)?;
        Ok(MaintenanceMaterial {
            revision: entry.revision,
            needs_resolution: entry.maintenance.uncertain
                || entry.maintenance.pending_totp.is_some(),
            password: entry.password,
            mfa_secret: entry.mfa_secret,
            pending_totp_secret: entry.maintenance.pending_totp.map(|p| p.secret),
            history: entry.maintenance.material_history,
        })
    }
    pub(super) async fn edit_maintenance_material(
        &self,
        command: MaterialUpdate,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        let gate = self.gate.lock().await;
        let mut entry = self
            .entries()
            .await?
            .into_iter()
            .find(|entry| entry.id == command.id)
            .ok_or_else(|| AdminError::not_found("资料不存在"))?;
        if entry.revision != command.revision
            || entry.status.active()
            || entry.status == ReloginStatus::Uncertain
            || gate.active.contains_key(&entry.email)
        {
            return Err(AdminError::conflict("资料已变化或任务仍在执行，请刷新"));
        }
        let secret = totp_secret(&command.mfa_secret)
            .ok_or_else(|| AdminError::invalid("2FA 密钥格式无效"))?;
        if command.password.is_empty()
            || command.password.len() > 1024
            || command.password.chars().any(char::is_control)
        {
            return Err(AdminError::invalid("密码格式无效"));
        }
        self.store()?
            .record_maintenance(&entry.id, None, context)
            .await
            .map_err(store_error)?;
        entry.maintenance.material_history.push(MaterialVersion {
            password: entry.password.clone(),
            mfa_secret: entry.mfa_secret.clone(),
            saved_at: Utc::now(),
            pending_totp: entry.maintenance.pending_totp.clone(),
        });
        if entry.maintenance.material_history.len() > 10 {
            entry.maintenance.material_history.remove(0);
        }
        entry.password = command.password;
        entry.mfa_secret = secret;
        // Ordinary edits never resolve an uncertain remote mutation. Explicit manual
        // reconciliation preserves the candidate (including its enrollment) in history.
        if command.confirm_maintenance_resolution {
            entry.maintenance.pending_totp = None;
            entry.maintenance.uncertain = false;
            entry.maintenance.view.state = "".to_owned();
            entry.maintenance.view.message = "已按人工核实结果更新本地资料".to_owned();
            entry.maintenance.view.totp_enabled = None;
            entry.maintenance.view.pending_totp = false;
        }
        entry.stop_reason = None;
        entry.automatic_attempts = 0;
        entry.next_attempt_at = None;
        let login_attempt = entry.last_attempt.take();
        entry.begin_attempt();
        if let Some(attempt) = &mut entry.last_attempt {
            attempt.operation = Some(MaintenanceAction::EditMaterial);
            attempt.result = ReloginAttemptResult::Succeeded;
            attempt.finished_at = Some(Utc::now());
            attempt.message = "本地密码与 2FA 资料已保存".into();
            attempt.push_result = ReloginPushResult::NotRequested;
        }
        entry.maintenance.last_activity = entry.last_attempt.take();
        entry.last_attempt = login_attempt;
        self.save(&mut entry).await
    }
}
