use super::*;

impl DefaultReloginService {
    pub(super) async fn arrange_enrollment(
        &self,
        command: EnrollmentPlanCommand,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        let gate = self.gate.lock().await;
        let mut entry = self
            .entries()
            .await?
            .into_iter()
            .find(|row| row.id == command.id)
            .ok_or_else(|| AdminError::not_found("账号资料不存在"))?;
        if entry.revision != command.revision {
            return Err(AdminError::conflict("资料已变化，请刷新后重新确认"));
        }
        if entry.status.active()
            || entry.status == ReloginStatus::Uncertain
            || gate.active.contains_key(&entry.email)
            || entry.maintenance.blocks_login()
        {
            return Err(AdminError::conflict("请先完成或取消当前账号任务"));
        }
        let pool = self.pool().await?;
        if !matching_accounts(&entry, &pool).is_empty() {
            return Err(AdminError::conflict(
                "账号已在号池中，请通过账号维护更新凭据",
            ));
        }
        if command.mode == EnrollmentPlanMode::Cancel {
            let plan = entry
                .enrollment_plan
                .as_mut()
                .filter(|plan| plan.state == EnrollmentPlanState::Scheduled)
                .ok_or_else(|| AdminError::conflict("当前没有待执行的定时安排"))?;
            plan.state = EnrollmentPlanState::Draft;
            plan.when = None;
            entry.message = "已取消定时，保留入池设置".to_owned();
        } else {
            entry.validate_totp()?;
            if command.mode == EnrollmentPlanMode::Now
                && self.store()?.settings().await.map_err(store_error)?.paused
            {
                return Err(AdminError::conflict("重登队列已暂停"));
            }
            if command.mode == EnrollmentPlanMode::Scheduled {
                if command.when.is_none_or(|time| time <= Utc::now()) {
                    return Err(AdminError::invalid("请选择晚于现在的执行时间"));
                }
            } else if command.when.is_some() {
                return Err(AdminError::invalid("仅定时执行可指定执行时间"));
            }
            let mut enrollment = command
                .enrollment
                .ok_or_else(|| AdminError::invalid("缺少入池设置"))?;
            if let Some(selection) = &command.template {
                let template = self.templates.resolve(selection.clone()).await?;
                enrollment.config.name = template.config.name;
                enrollment.config.turn_state_injection_enabled = enrollment
                    .config
                    .turn_state_injection_enabled
                    .or(template.config.turn_state_injection_enabled);
            }
            enrollment.settings()?;
            self.store()?
                .validate_template_references(&enrollment.config)
                .await
                .map_err(super::super::account_templates::template_error)?;
            entry.clear_enrollment();
            entry.maintenance.import_egress =
                Some(crate::model::relogin_maintenance::ReloginImportEgress {
                    proxy_id: enrollment.config.outbound_proxy_id.clone(),
                    credential_proxy_id: enrollment.config.credential_proxy_id.clone().flatten(),
                    mode: enrollment.config.egress_mode,
                });
            if command.mode != EnrollmentPlanMode::Draft {
                entry.maintenance.login_paused = false;
            }
            entry.manual_push_context = None;
            entry.automatic_job = false;
            entry.automatic_attempts = 0;
            entry.next_attempt_at = None;
            entry.stop_reason = None;
            entry.enrollment_plan = Some(ReloginEnrollmentPlan {
                state: if command.mode == EnrollmentPlanMode::Scheduled {
                    EnrollmentPlanState::Scheduled
                } else {
                    EnrollmentPlanState::Draft
                },
                when: command.when,
                enrollment,
                template: command.template,
            });
            entry.message = if command.mode == EnrollmentPlanMode::Scheduled {
                "已安排定时入池"
            } else {
                "入池设置已保存"
            }
            .to_owned();
            if command.mode == EnrollmentPlanMode::Now {
                Self::activate_enrollment_plan(&mut entry, &pool)?;
            }
        }
        let expected = entry.revision;
        entry.revision = expected
            .checked_add(1)
            .filter(|revision| *revision <= i64::MAX as u64)
            .ok_or_else(|| AdminError::internal("重登资料版本超出范围"))?;
        entry.updated_at = Utc::now();
        self.store()?
            .save_enrollment_plan(&entry, expected, context)
            .await
            .map_err(store_error)
    }

    pub(super) fn activate_enrollment_plan(
        entry: &mut ReloginEntry,
        pool: &[AccountRecord],
    ) -> Result<(), AdminError> {
        if !matching_accounts(entry, pool).is_empty() {
            return Err(AdminError::conflict("账号已在号池中，未执行新账号入池"));
        }
        workspace::prepare_queue(entry, pool, ReloginWorkspaceMode::Original)?;
        let plan = entry
            .enrollment_plan
            .as_mut()
            .ok_or_else(|| AdminError::invalid("入池安排不存在"))?;
        plan.state = EnrollmentPlanState::Running;
        plan.when = None;
        entry.enrollment = Some(plan.enrollment.clone());
        entry.credential = None;
        entry.synced_at = None;
        entry.maintenance.login_paused = false;
        entry.automatic_job = false;
        entry.manual_push_context = None;
        entry.status = ReloginStatus::Queued;
        entry.message = "等待登录验证并入池".to_owned();
        Ok(())
    }
}

pub(super) fn view(
    entry: &ReloginEntry,
    exit_label: &str,
    credential_exit_label: &str,
) -> Option<ReloginEnrollmentPlanView> {
    // Existing immediate imports already persist their complete enrollment settings.
    let legacy = entry
        .enrollment
        .as_ref()
        .map(|enrollment| ReloginEnrollmentPlan {
            state: EnrollmentPlanState::Running,
            when: None,
            enrollment: enrollment.clone(),
            template: None,
        });
    let plan = entry.enrollment_plan.as_ref().or(legacy.as_ref())?;
    let state = match plan.state {
        EnrollmentPlanState::Draft => "draft",
        EnrollmentPlanState::Scheduled => "scheduled",
        EnrollmentPlanState::Done => "done",
        EnrollmentPlanState::Failed => "failed",
        EnrollmentPlanState::Running => match entry.status {
            ReloginStatus::Failed => {
                if entry.automatic_stop_reason().is_some() {
                    "verification"
                } else {
                    "failed"
                }
            }
            ReloginStatus::AwaitingWorkspace => "verification",
            ReloginStatus::Uncertain => "uncertain",
            ReloginStatus::Queued => "queued",
            _ => "running",
        },
    };
    let mut config = plan.enrollment.config.clone();
    config.model_access = plan.enrollment.model_access.clone();
    Some(ReloginEnrollmentPlanView {
        state,
        when: plan.when,
        config,
        custom_name: plan.enrollment.custom_name.clone(),
        purchase_cost: plan.enrollment.purchase_cost.clone(),
        quality_template: plan.enrollment.quality_template.clone(),
        template: plan.template.clone(),
        exit_label: exit_label.to_owned(),
        credential_exit_label: credential_exit_label.to_owned(),
    })
}
