use super::*;

impl DefaultReloginService {
    pub(super) async fn export_selected(
        &self,
        ids: &[String],
        format: ReloginExportFormat,
        context: &MutationContext,
    ) -> Result<ReloginExport, AdminError> {
        validate_ids(ids)?;
        if ids.len() > 200 {
            return Err(AdminError::invalid("单次最多导出 200 条资料"));
        }
        let entries = self.entries().await?;
        let selected = ids
            .iter()
            .map(|id| {
                entries
                    .iter()
                    .find(|entry| &entry.id == id)
                    .ok_or_else(|| AdminError::not_found("部分重登资料不存在，请刷新"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut files = Vec::new();
        let mut skipped = Vec::new();
        match format {
            ReloginExportFormat::TwoFa => {
                let mut lines = Vec::with_capacity(selected.len());
                for entry in selected {
                    entry.validate_totp()?;
                    lines.push(format!(
                        "{}----{}----{}",
                        entry.email, entry.password, entry.mfa_secret
                    ));
                }
                files.push(ReloginExportFile {
                    name: "relogin-2fa.txt".to_owned(),
                    content: format!("{}\n", lines.join("\n")),
                });
            }
            ReloginExportFormat::Json => {
                let pool = self.pool().await?;
                let mut documents = Vec::with_capacity(selected.len());
                for entry in selected {
                    let outcome: Result<serde_json::Value, AdminError> = async {
                    let target = if let Some(target) = &entry.target {
                        Some(
                            pool.iter()
                                .find(|account| {
                                    target.matches_account(account)
                                        && account.email.as_ref().is_some_and(|email| {
                                            email.eq_ignore_ascii_case(&entry.email)
                                        })
                                })
                                .ok_or_else(|| {
                                    AdminError::conflict(
                                        "关联账号已删除或身份已变化，不能导出旧凭据",
                                    )
                                })?,
                        )
                    } else {
                        let matches = matching_accounts(entry, &pool);
                        match matches.as_slice() {
                            [] => None,
                            [account] => Some(*account),
                            _ => {
                                return Err(AdminError::conflict(
                                    "同邮箱存在多个工作区，请在账号管理中选择具体账号导出",
                                ));
                            }
                        }
                    };
                    let target = target.filter(|account| account.authentication_kind != "none");
                    let document = if let Some(account) = target.filter(|_| self.credential_target == CredentialTarget::Excel) {
                        let id = ProviderAccountId::new(account.id.clone())
                            .map_err(|_| AdminError::internal("账号 ID 无效"))?;
                        self.openai.export_excel_credential(&id).await?
                            .into_provider_data().into_inner()
                    } else if let Some(account) = target {
                        let id = ProviderAccountId::new(account.id.clone())
                            .map_err(|_| AdminError::internal("账号 ID 无效"))?;
                        let credentials = self
                            .accounts
                            .load_credentials_for_export(self.provider.provider_kind(), &[id])
                            .await
                            .map_err(store_error)?;
                        if credentials.len() != 1
                            || credentials[0].account.upstream_user_id != account.upstream_user_id
                            || credentials[0].account.upstream_account_id
                                != account.upstream_account_id
                        {
                            return Err(AdminError::conflict("账号身份已变化，请刷新后导出"));
                        }
                        self.provider
                            .export_credentials(credentials)
                            .await
                            .map_err(|error| map_provider_error(error, "relogin export"))?
                            .document
                            .into_provider_data()
                            .into_inner()
                    } else {
                        let credential = entry
                            .credential
                            .as_ref()
                            .filter(|credential| {
                                credential.email.eq_ignore_ascii_case(&entry.email)
                            })
                            .ok_or_else(|| {
                                AdminError::conflict("部分资料尚未获取 JSON，请先成功登录")
                            })?;
                        credential.document.clone()
                    };
                    Ok(if self.credential_target == CredentialTarget::Excel {
                        serde_json::json!({"provider": "openai", "credentialTarget": "excel", "document": document})
                    } else {
                        serde_json::json!({"provider": "openai", "document": document})
                    })
                    }.await;
                    match outcome {
                        Ok(document) => documents.push(document),
                        Err(error) => skipped.push(ReloginExportSkipped {
                            id: entry.id.clone(),
                            credential_target: self.credential_target,
                            message: error.message().to_owned(),
                        }),
                    }
                }
                if !documents.is_empty() {
                    files.push(ReloginExportFile {
                    name: match self.credential_target {
                        CredentialTarget::Codex => "relogin-accounts.json",
                        CredentialTarget::Excel => "relogin-excel-accounts.json",
                    }.to_owned(),
                    content: serde_json::to_string_pretty(&serde_json::json!({"exportedAt": Utc::now().to_rfc3339(), "documents": documents}))
                        .map_err(|_| AdminError::internal("JSON 导出失败"))?,
                });
                }
            }
        }
        self.store()?
            .record_export(ids, format, context)
            .await
            .map_err(store_error)?;
        Ok(ReloginExport { files, skipped })
    }
}
