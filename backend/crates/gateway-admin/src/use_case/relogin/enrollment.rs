use super::*;

impl DefaultReloginService {
    pub(super) async fn import_entries(
        &self,
        text: &str,
        replace: bool,
        enrollment: Option<ReloginEnrollment>,
    ) -> Result<Vec<String>, AdminError> {
        let inputs = parse_relogin_import(text)?;
        let gate = self.gate.lock().await;
        if enrollment.is_some() && self.store()?.settings().await.map_err(store_error)?.paused {
            return Err(AdminError::conflict("重登队列已暂停，请先恢复队列"));
        }
        let entries = self.entries().await?;
        let pool = if enrollment.is_some() {
            self.pool().await?
        } else {
            Vec::new()
        };
        let existing: BTreeMap<_, _> = entries
            .into_iter()
            .map(|entry| (entry.email.clone(), entry))
            .collect();
        if !replace
            && inputs
                .iter()
                .any(|input| existing.contains_key(&input.email))
        {
            return Err(AdminError::conflict("存在重复邮箱，请勾选确认更新已有资料"));
        }
        if existing.len()
            + inputs
                .iter()
                .filter(|input| !existing.contains_key(&input.email))
                .count()
            > MAX_ENTRIES
        {
            return Err(AdminError::invalid("资料库最多保存 10000 个账号"));
        }
        if enrollment.is_some()
            && inputs.iter().any(|input| {
                existing.get(&input.email).is_some_and(|entry| {
                    entry.status.active()
                        || entry.status == ReloginStatus::Uncertain
                        || gate.active.contains_key(&entry.email)
                })
            })
        {
            return Err(AdminError::conflict(
                "账号已有活动任务或推送待确认，请先处理原任务",
            ));
        }
        let count = inputs.len();
        let mut changes = Vec::with_capacity(count);
        for input in inputs {
            if let Some(old) = existing.get(&input.email) {
                let mut entry = old.clone();
                Self::stop(&gate, &mut entry);
                entry.password = input.password;
                entry.mfa_secret = input.mfa_secret;
                entry.automatic_attempts = 0;
                entry.stop_reason = None;
                entry.next_attempt_at = None;
                entry.status = ReloginStatus::Pending;
                entry.credential = None;
                entry.synced_at = None;
                entry.target = None;
                entry.workspace_mode = ReloginWorkspaceMode::Original;
                entry.workspace_targets.clear();
                entry.workspace_choices.clear();
                entry.selected_workspace_id = None;
                entry.message = "资料已更新，等待处理".to_owned();
                let expected = entry.revision;
                entry.revision = expected
                    .checked_add(1)
                    .filter(|revision| *revision <= i64::MAX as u64)
                    .ok_or_else(|| AdminError::internal("重登资料版本超出范围"))?;
                entry.updated_at = Utc::now();
                entry.imported_at = Some(entry.updated_at);
                changes.push((entry, Some(expected)));
            } else {
                let entry = ReloginEntry {
                    id: format!("relogin_{}", uuid::Uuid::now_v7().simple()),
                    revision: 1,
                    email: input.email,
                    password: input.password,
                    mfa_secret: input.mfa_secret,
                    automatic: true,
                    preferred_workspace_id: None,
                    status: ReloginStatus::Pending,
                    message: String::new(),
                    credential: None,
                    target: None,
                    automatic_job: false,
                    workspace_mode: ReloginWorkspaceMode::Original,
                    workspace_targets: Vec::new(),
                    workspace_choices: Vec::new(),
                    selected_workspace_id: None,
                    manual_push_context: None,
                    enrollment: None,
                    automatic_attempts: 0,
                    stop_reason: None,
                    automatic_started_at: Vec::new(),
                    attempted_target: None,
                    next_attempt_at: None,
                    synced_at: None,
                    imported_at: Some(Utc::now()),
                    updated_at: Utc::now(),
                };
                changes.push((entry, None));
            }
        }
        if let Some(enrollment) = enrollment {
            for (entry, _) in &mut changes {
                workspace::prepare_queue(entry, &pool, ReloginWorkspaceMode::Original)?;
                entry.automatic_job = false;
                entry.manual_push_context = None;
                entry.enrollment = Some(enrollment.clone());
                entry.status = ReloginStatus::Queued;
                entry.message = "等待登录验证并入池".to_owned();
            }
        }
        let ids = changes.iter().map(|(entry, _)| entry.id.clone()).collect();
        self.store()?
            .save_batch(&changes)
            .await
            .map_err(store_error)?;
        Ok(ids)
    }
}
