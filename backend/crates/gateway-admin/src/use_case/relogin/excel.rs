use super::*;
use crate::model::accounts::ExcelCredentialImportTarget;
use gateway_core::account::CredentialRevision;

impl DefaultReloginService {
    pub(super) async fn queue_account_channels_checked(
        &self,
        entry_id: &str,
        revision: u64,
        targets: &[ReloginTarget],
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        validate_ids(&[entry_id.to_owned()])?;
        if targets.is_empty()
            || targets.len() > 2
            || (targets.len() == 2
                && (targets[0].credential_target == targets[1].credential_target
                    || targets[0].account_id != targets[1].account_id))
        {
            return Err(AdminError::invalid(
                "请选择同一账号的 Codex、Excel 或两种凭据",
            ));
        }
        let gate = self.gate.lock().await;
        if self.store()?.settings().await.map_err(store_error)?.paused {
            return Err(AdminError::conflict("重登队列已暂停"));
        }
        let mut stored = self
            .store()?
            .entries()
            .await
            .map_err(store_error)?
            .into_iter()
            .find(|entry| entry.id == entry_id)
            .ok_or_else(|| AdminError::not_found("重登资料不存在"))?;
        stored.validate_totp()?;
        if stored.revision != revision || gate.active.contains_key(&stored.email) {
            return Err(AdminError::conflict(
                "重登资料已变化或已有活动任务，请重新确认",
            ));
        }
        let pool = self.for_target(CredentialTarget::Codex).pool().await?;
        for target in targets {
            let mut channel = stored.clone().project_channel(target.credential_target);
            if channel.status.active() {
                return Err(AdminError::conflict("所选凭据已有活动任务"));
            }
            let account = pool
                .iter()
                .find(|account| account.id == target.account_id)
                .ok_or_else(|| AdminError::conflict("目标账号已删除"))?;
            if !target.matches_account(account)
                || account
                    .email
                    .as_ref()
                    .is_none_or(|email| !email.eq_ignore_ascii_case(&stored.email))
            {
                return Err(AdminError::conflict("目标账号或凭据已变化，请重新确认"));
            }
            channel.target = Some(target.clone());
            channel.workspace_mode = ReloginWorkspaceMode::Original;
            channel.workspace_targets.clear();
            channel.workspace_choices.clear();
            channel.selected_workspace_id = None;
            channel.automatic_job = false;
            channel.manual_push_context = Some(context.clone());
            channel.enrollment = None;
            channel.status = ReloginStatus::Queued;
            channel.message = "等待重登，验证后同步所选凭据".to_owned();
            if target.credential_target == CredentialTarget::Codex {
                stored = channel;
            } else {
                stored.excel = Some(channel.channel_state());
            }
        }
        self.for_target(CredentialTarget::Codex)
            .save(&mut stored)
            .await
    }

    pub(super) async fn push_excel_checked(
        &self,
        entry: &mut ReloginEntry,
        template: Option<&ReloginTemplateConfig>,
        custom_name: Option<&str>,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        entry.validate_totp()?;
        if !matches!(entry.status, ReloginStatus::Ready | ReloginStatus::Failed) {
            return Err(AdminError::conflict("请先成功获取 Excel 凭据"));
        }
        let credential = entry
            .credential
            .clone()
            .ok_or_else(|| AdminError::invalid("尚未获取 Excel 凭据"))?;
        if credential.expires_at <= Utc::now() {
            return Err(AdminError::invalid("Excel 缓存凭据已过期，请重新登录"));
        }
        if !credential.email.eq_ignore_ascii_case(&entry.email) {
            return Err(AdminError::conflict("Excel 凭据与资料身份不一致"));
        }
        let pool = self.pool().await?;
        let target = if let Some(target) = &entry.target {
            let current = pool
                .iter()
                .find(|account| account.id == target.account_id)
                .ok_or_else(|| AdminError::conflict("原账号已删除，本次结果不能重新入池"))?;
            if target.credential_target != CredentialTarget::Excel
                || !target.matches_account(current)
                || current.upstream_user_id.as_deref() != Some(credential.user_id.as_str())
                || current.upstream_account_id.as_deref() != Some(credential.workspace_id.as_str())
            {
                return Err(AdminError::conflict(
                    "Excel 凭据、用户或工作区已变化，请重新获取",
                ));
            }
            if entry.synced_at.is_some() {
                return Ok(());
            }
            if entry.automatic_job && !worker::needs_relogin(current) {
                return Err(AdminError::conflict(
                    "Excel 凭据已恢复或账号已禁用，未覆盖现有凭据",
                ));
            }
            Some(ExcelCredentialImportTarget {
                account_id: target.account_id.clone(),
                expected_revision: if target.credential_revision == 0 {
                    None
                } else {
                    Some(
                        CredentialRevision::new(target.credential_revision)
                            .map_err(|_| AdminError::conflict("Excel 凭据版本不合法"))?,
                    )
                },
            })
        } else {
            if entry.manual_push_context.is_some() || entry.automatic_job {
                return Err(AdminError::conflict("指定账号任务缺少目标，不能新建账号"));
            }
            if pool.iter().any(|account| {
                account.upstream_user_id.as_deref() == Some(credential.user_id.as_str())
                    && account.upstream_account_id.as_deref()
                        == Some(credential.workspace_id.as_str())
            }) {
                return Err(AdminError::conflict(
                    "获取后出现相同工作区账号，请重新获取以锁定目标",
                ));
            }
            None
        };
        let mut settings = if target.is_none() {
            template.map(ReloginTemplateConfig::settings).transpose()?
        } else {
            None
        };
        if target.is_none() && custom_name.is_some() && settings.is_none() {
            settings = Some(crate::model::accounts::AccountImportSettings {
                template_proxy_mode: None,
                excel_recovery: None,
                purchase_cost: None,
                clear_outbound_proxy: false,
                egress_mode: None,
                model_access: None,
                custom_name: None,
                enabled: true,
                turn_state_injection_enabled: None,
                responses_upstream: None,
                excel_models: None,
                excel_models_follow_global: None,
                excel_cache_creation_as_input: None,
                excel_ignore_encrypted_content: None,
                request_proxy_source: None,
                excel_auto_disable_on_403: None,
                excel_403_action: None,
                concurrency_limit: None,
                weight: crate::model::accounts::AccountWeight::DEFAULT,
                group_ids: Vec::new(),
            });
        }
        if let Some(settings) = &mut settings {
            settings.custom_name = custom_name.map(str::to_owned);
            settings.responses_upstream = Some(gateway_core::account::ResponsesUpstream::Codex);
        }
        let outbound_proxy_id = target
            .is_none()
            .then(|| template.and_then(|value| value.outbound_proxy_id.as_deref()))
            .flatten();
        let reservation =
            super::super::import_proxy_binding(self.proxies.as_ref(), outbound_proxy_id).await?;
        let prepared = self
            .provider
            .prepare_excel_import(
                crate::model::provider_credentials::PrepareCredentialImport {
                    default_outbound_proxy: reservation
                        .as_ref()
                        .map(|value| value.binding.proxy.clone()),
                    document: ProviderDocument::new(OpaqueProviderData::new(credential.document)),
                },
            )
            .await
            .map_err(|error| map_provider_error(error, "Excel relogin import preparation"))?;
        super::super::validate_prepared_import(
            self.provider.provider_kind(),
            &prepared,
            "Excel relogin import preparation",
        )?;
        if prepared.credentials.len() != 1
            || prepared.credentials[0].authentication_kind != "excel_oauth"
        {
            return Err(AdminError::invalid("Excel 凭据预校验失败"));
        }
        let prepared = prepared
            .credentials
            .into_iter()
            .next()
            .ok_or_else(|| AdminError::invalid("Excel 凭据缺失"))?;
        entry.status = ReloginStatus::Pushing;
        self.save(entry).await?;
        let committed = self
            .accounts
            .commit_excel_credential_import(prepared, target, settings, context)
            .await
            .map_err(store_error)?;
        drop(reservation);
        let account = &committed.account;
        let id = ProviderAccountId::new(account.id.clone())
            .map_err(|_| AdminError::internal("账号 ID 无效"))?;
        self.provider.account_facts_changed(&[id]).await;
        publish_committed(self.snapshot.as_ref(), committed.config_revision).await?;
        entry.target = Some(ReloginTarget::from_account_for_target(
            account,
            CredentialTarget::Excel,
        )?);
        entry.synced_at = Some(Utc::now());
        entry.status = ReloginStatus::Ready;
        entry.enrollment = None;
        entry.next_attempt_at = None;
        entry.automatic_attempts = 0;
        entry.stop_reason = None;
        entry.attempted_target = None;
        entry.message = "Excel 凭据已同步到号池".to_owned();
        self.save(entry).await
    }
}
