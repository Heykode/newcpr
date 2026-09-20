//! Relogin orchestration; managed account mutations retain their existing owners.

use super::{commit_credential_rotation, map_provider_error, map_store_error, publish_committed};
use crate::{
    OpenAiService,
    model::{
        AdminError, AdminErrorKind, MutationContext,
        accounts::{AccountRecord, CredentialState},
        provider_credentials::{
            CredentialListQuery, CredentialListWindow, CredentialMutationResult, ImportCredentials,
            PrepareCredentialRotation, ProviderDocument,
        },
        relogin::*,
        relogin_templates::{ReloginTemplate, ReloginTemplateConfig, ReloginTemplateSelection},
    },
    ports::{
        provider::ProviderAdmin,
        relogin::ReloginStore,
        store::{AccountRuntimeStore, AccountStore},
    },
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gateway_core::{
    account::{
        AccountStatusFacts, OpaqueProviderData, ProviderAccountId,
        resolve_account_operational_status,
    },
    lifecycle::CancellationToken,
    runtime::SnapshotControl,
};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;

mod recovery;
mod templates;
mod worker;
pub(crate) use worker::contribution;

const MAX_ROTATION_ATTEMPTS: usize = 3;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloginView {
    pub id: String,
    pub revision: u64,
    pub email: String,
    pub has_totp: bool,
    pub automatic: bool,
    pub status: ReloginStatus,
    pub message: String,
    pub recovery: recovery::RecoveryView,
    pub plan_type: Option<String>,
    pub workspace_id: Option<String>,
    pub preferred_workspace_id: Option<String>,
    pub credential_status: &'static str,
    pub pool_status: &'static str,
    pub pool_account_ids: Vec<String>,
    pub pool_accounts: Vec<ReloginPoolAccountView>,
    pub relogin_account_id: Option<String>,
    pub relogin_count: Option<u64>,
    pub last_relogin_at: Option<DateTime<Utc>>,
    pub verified_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub imported_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloginPoolAccountView {
    pub id: String,
    pub workspace_id: Option<String>,
    pub plan_type: Option<String>,
    pub enabled: bool,
    pub status: &'static str,
    pub error_reason: Option<&'static str>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloginList {
    pub settings: ReloginSettings,
    pub items: Vec<ReloginView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloginBatchResult {
    pub id: String,
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountReloginAction {
    pub account_id: String,
    pub entry_id: String,
    pub revision: u64,
    pub target: Option<ReloginTarget>,
    pub status: ReloginStatus,
    pub message: String,
    pub busy: bool,
    pub blocked_reason: Option<String>,
    pub synced_at: Option<DateTime<Utc>>,
}

#[async_trait]
pub trait ReloginService: Send + Sync {
    async fn templates(&self) -> Result<Vec<ReloginTemplate>, AdminError>;
    async fn save_template(
        &self,
        selection: Option<ReloginTemplateSelection>,
        config: ReloginTemplateConfig,
    ) -> Result<ReloginTemplate, AdminError>;
    async fn delete_template(&self, selection: ReloginTemplateSelection) -> Result<(), AdminError>;
    async fn push_with_template(
        &self,
        ids: &[String],
        revisions: &BTreeMap<String, u64>,
        template: Option<ReloginTemplateSelection>,
        context: &MutationContext,
    ) -> Result<Vec<ReloginBatchResult>, AdminError>;
    async fn list(&self) -> Result<ReloginList, AdminError>;
    async fn account_actions(
        &self,
        ids: &[String],
    ) -> Result<Vec<AccountReloginAction>, AdminError>;
    async fn queue_account(
        &self,
        entry_id: &str,
        revision: u64,
        target: &ReloginTarget,
        context: &MutationContext,
    ) -> Result<(), AdminError>;
    async fn import(&self, text: &str, replace: bool) -> Result<usize, AdminError>;
    async fn queue(&self, ids: &[String]) -> Result<Vec<ReloginBatchResult>, AdminError>;
    async fn push(
        &self,
        ids: &[String],
        revisions: &BTreeMap<String, u64>,
        context: &MutationContext,
    ) -> Result<Vec<ReloginBatchResult>, AdminError>;
    async fn delete(&self, ids: &[String]) -> Result<(), AdminError>;
    async fn automatic(&self, ids: &[String], enabled: bool) -> Result<(), AdminError>;
    async fn workspace(&self, id: &str, workspace: Option<String>) -> Result<(), AdminError>;
    async fn configure(&self, settings: ReloginSettings) -> Result<(), AdminError>;
}

#[derive(Default)]
struct Gate {
    active: BTreeMap<String, ActiveLogin>,
}

struct ActiveLogin {
    cancellation: CancellationToken,
    owner: std::sync::Weak<()>,
}

pub(crate) struct DefaultReloginService {
    store: Option<Arc<dyn ReloginStore>>,
    accounts: Arc<dyn AccountStore>,
    runtime: Arc<dyn AccountRuntimeStore>,
    provider: Arc<dyn ProviderAdmin>,
    openai: Arc<dyn OpenAiService>,
    snapshot: Arc<dyn SnapshotControl>,
    gate: Mutex<Gate>,
}

impl DefaultReloginService {
    pub(crate) fn new(
        store: Option<Arc<dyn ReloginStore>>,
        accounts: Arc<dyn AccountStore>,
        runtime: Arc<dyn AccountRuntimeStore>,
        provider: Arc<dyn ProviderAdmin>,
        openai: Arc<dyn OpenAiService>,
        snapshot: Arc<dyn SnapshotControl>,
    ) -> Self {
        Self {
            store,
            accounts,
            runtime,
            provider,
            openai,
            snapshot,
            gate: Mutex::new(Gate::default()),
        }
    }

    fn store(&self) -> Result<&dyn ReloginStore, AdminError> {
        self.store
            .as_deref()
            .ok_or_else(|| AdminError::unavailable("重登资料库未配置"))
    }

    async fn entries(&self) -> Result<Vec<ReloginEntry>, AdminError> {
        self.store()?.entries().await.map_err(store_error)
    }

    async fn pool(&self) -> Result<Vec<AccountRecord>, AdminError> {
        Ok(self
            .accounts
            .list_credentials(
                self.provider.provider_kind(),
                CredentialListQuery {
                    credential_state: None,
                    enabled: None,
                    window: CredentialListWindow::All,
                },
            )
            .await
            .map_err(store_error)?
            .items)
    }

    async fn save(&self, entry: &mut ReloginEntry) -> Result<(), AdminError> {
        let expected = entry.revision;
        entry.revision = expected
            .checked_add(1)
            .filter(|revision| *revision <= i64::MAX as u64)
            .ok_or_else(|| AdminError::internal("重登资料版本超出范围"))?;
        entry.updated_at = Utc::now();
        self.store()?
            .save(entry, Some(expected))
            .await
            .map_err(store_error)
    }

    fn stop(gate: &Gate, entry: &mut ReloginEntry) {
        entry.manual_push_context = None;
        if let Some(active) = gate.active.get(&entry.email) {
            active.cancellation.cancel();
        }
        if entry.status.active() {
            entry.status = ReloginStatus::Failed;
            entry.message = "任务已取消，未推送凭据".to_owned();
        }
    }

    async fn push_entry(
        &self,
        entry: &mut ReloginEntry,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        self.push_entry_with_template(entry, None, context).await
    }

    async fn push_entry_with_template(
        &self,
        entry: &mut ReloginEntry,
        template: Option<&ReloginTemplateConfig>,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        let outcome = self.push_checked(entry, template, context).await;
        if outcome.is_err() && entry.status == ReloginStatus::Pushing {
            entry.status = ReloginStatus::Uncertain;
            entry.message = "推送结果未确认，请检查号池并重新获取凭证后再推送".to_owned();
            self.save(entry).await?;
        }
        outcome
    }

    async fn current_rotation_target(
        &self,
        entry: &ReloginEntry,
    ) -> Result<AccountRecord, AdminError> {
        let target = entry
            .target
            .as_ref()
            .ok_or_else(|| AdminError::conflict("指定账号任务缺少目标，不能新建账号"))?;
        let id = ProviderAccountId::new(target.account_id.clone())
            .map_err(|_| AdminError::invalid("目标账号 ID 无效"))?;
        let current = self
            .accounts
            .credential_details(self.provider.provider_kind(), &id)
            .await
            .map_err(|error| map_store_error(error, "relogin target"))?
            .ok_or_else(|| AdminError::conflict("原账号已删除，不能自动重新入池"))?
            .credential;
        if !target.matches_account(&current)
            || current
                .email
                .as_ref()
                .is_none_or(|email| !email.eq_ignore_ascii_case(&entry.email))
        {
            return Err(AdminError::conflict(
                "原凭据或账号身份已变化，本次结果未覆盖，请重新获取凭证",
            ));
        }
        if entry.automatic_job && !worker::needs_relogin(&current) {
            return Err(AdminError::conflict(
                "原账号已恢复或已禁用，不再自动替换凭据",
            ));
        }
        Ok(current)
    }

    async fn rotate_existing(
        &self,
        entry: &mut ReloginEntry,
        mut current: AccountRecord,
        document: ProviderDocument,
        context: &MutationContext,
    ) -> Result<CredentialMutationResult, AdminError> {
        let target = entry
            .target
            .as_ref()
            .ok_or_else(|| AdminError::conflict("指定账号任务缺少目标，不能新建账号"))?;
        let operation_id = format!(
            "relogin:{}:{}",
            target.account_id, target.credential_revision
        );
        for _ in 0..MAX_ROTATION_ATTEMPTS {
            let prepared = self
                .provider
                .prepare_rotation(PrepareCredentialRotation {
                    account: current,
                    provider_material: document.clone(),
                })
                .await
                .map_err(|error| map_provider_error(error, "relogin rotation"))?;
            current = self.current_rotation_target(entry).await?;
            let expected = prepared.facts().expected_credential_revision;
            if prepared.facts().account_id.as_str() != current.id
                || prepared.facts().provider_kind != current.provider_kind
            {
                return Err(AdminError::conflict("凭据准备结果与目标账号不一致"));
            }
            if expected != current.credential_revision {
                continue;
            }
            // Exact CAS still owns the commit; only a definitive conflict may retry.
            entry.status = ReloginStatus::Pushing;
            self.save(entry).await?;
            match commit_credential_rotation(
                self.accounts.as_ref(),
                prepared,
                context,
                "relogin rotation",
                Some(operation_id.clone()),
            )
            .await
            {
                Ok(result) => return Ok(result),
                Err(error) if error.kind() == AdminErrorKind::Conflict => {
                    entry.status = ReloginStatus::Ready;
                    self.save(entry).await?;
                    current = self.current_rotation_target(entry).await?;
                    if current.credential_revision == expected {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(AdminError::conflict(
            "账号 Cookie 更新频繁，本次未写入，请重试推送",
        ))
    }

    async fn push_checked(
        &self,
        entry: &mut ReloginEntry,
        template: Option<&ReloginTemplateConfig>,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        entry.validate_totp()?;
        if entry.manual_push_context.is_some() && entry.target.is_none() {
            return Err(AdminError::conflict("指定账号任务缺少目标，不能新建账号"));
        }
        if entry.status != ReloginStatus::Ready {
            return Err(AdminError::conflict("请先成功获取新凭据"));
        }
        let credential = entry
            .credential
            .as_ref()
            .ok_or_else(|| AdminError::invalid("尚未获取凭证"))?;
        if credential.expires_at <= Utc::now() {
            return Err(AdminError::invalid("缓存凭证已过期，请重新登录"));
        }
        if !credential.email.eq_ignore_ascii_case(&entry.email) {
            return Err(AdminError::conflict("凭据邮箱与资料不一致"));
        }
        let pool = self.pool().await?;
        let matches = matching_accounts(entry, &pool);
        let current = if let Some(target) = &entry.target {
            let current = pool
                .iter()
                .find(|account| account.id == target.account_id)
                .ok_or_else(|| AdminError::conflict("原账号已删除，不能自动重新入池"))?;
            if current.upstream_user_id.as_deref() != Some(credential.user_id.as_str())
                || current.upstream_account_id.as_deref() != Some(credential.workspace_id.as_str())
                || current
                    .email
                    .as_ref()
                    .is_none_or(|email| !email.eq_ignore_ascii_case(&entry.email))
            {
                return Err(AdminError::conflict("原账号身份或工作区与新凭据不一致"));
            }
            if entry.synced_at.is_some() {
                return Ok(());
            }
            if entry.automatic_job && !worker::needs_relogin(current) {
                return Err(AdminError::conflict(
                    "原账号已恢复或已禁用，不再自动替换凭据",
                ));
            }
            if !target.matches_account(current) {
                return Err(AdminError::conflict(
                    "原凭据已变化，本次结果未覆盖，请重新获取凭证",
                ));
            }
            Some(current)
        } else {
            if !matches.is_empty() {
                return Err(AdminError::conflict(
                    "获取凭证后号池出现同邮箱账号，请重新获取凭证以锁定目标",
                ));
            }
            None
        };
        let document = ProviderDocument::new(OpaqueProviderData::new(credential.document.clone()));
        if let Some(current) = current {
            if entry.synced_at.is_some() {
                return Ok(());
            }
            let result = self
                .rotate_existing(entry, current.clone(), document, context)
                .await?;
            self.provider
                .account_facts_changed(std::slice::from_ref(&result.account_id))
                .await;
            publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
            entry.target = Some(ReloginTarget::from_account(current)?);
        } else {
            let settings = template.map(ReloginTemplateConfig::settings).transpose()?;
            if let Some(config) = template {
                self.store()?
                    .validate_template_references(config)
                    .await
                    .map_err(templates::template_error)?;
            }
            // Import owns both preparation and commit, so fence before entering it.
            entry.status = ReloginStatus::Pushing;
            self.save(entry).await?;
            let result = self
                .openai
                .import_new_document(ImportCredentials {
                    outbound_proxy_id: template.and_then(|config| config.outbound_proxy_id.clone()),
                    settings,
                    context: context.clone(),
                    document,
                })
                .await?;
            let id = result
                .credential_ids
                .first()
                .ok_or_else(|| AdminError::internal("导入未返回账号"))?;
            let current = self
                .accounts
                .credential_details(self.provider.provider_kind(), id)
                .await
                .map_err(store_error)?
                .ok_or_else(|| AdminError::conflict("入池后账号已变化"))?;
            entry.target = Some(ReloginTarget::from_account(&current.credential)?);
        }
        entry.synced_at = Some(Utc::now());
        entry.next_attempt_at = None;
        entry.automatic_attempts = 0;
        entry.attempted_target = None;
        entry.status = ReloginStatus::Ready;
        entry.message = "凭据已同步到号池".to_owned();
        self.save(entry).await
    }
}

fn store_error(error: crate::ports::store::AdminStoreError) -> AdminError {
    map_store_error(error, "relogin")
}

pub fn matching_accounts<'a>(
    entry: &ReloginEntry,
    pool: &'a [AccountRecord],
) -> Vec<&'a AccountRecord> {
    pool.iter()
        .filter(|account| {
            account.authentication_kind == "oauth"
                && account
                    .email
                    .as_ref()
                    .is_some_and(|email| email.eq_ignore_ascii_case(&entry.email))
        })
        .collect()
}

pub fn select_target(
    entry: &ReloginEntry,
    pool: &[AccountRecord],
) -> Result<Option<ReloginTarget>, AdminError> {
    let matches: Vec<_> = matching_accounts(entry, pool)
        .into_iter()
        .filter(|account| {
            entry
                .preferred_workspace_id
                .as_ref()
                .is_none_or(|workspace| account.upstream_account_id.as_ref() == Some(workspace))
        })
        .collect();
    if matches.len() > 1 {
        return Err(AdminError::conflict(
            "同邮箱有多个工作区，请先指定工作区 ID",
        ));
    }
    matches
        .first()
        .map(|account| ReloginTarget::from_account(account))
        .transpose()
}

fn statistics_account<'a>(
    entry: &ReloginEntry,
    matches: &[&'a AccountRecord],
) -> Option<&'a AccountRecord> {
    if let Some(target) = &entry.target {
        return matches.iter().copied().find(|account| {
            account.id == target.account_id
                && account.upstream_user_id.as_deref() == Some(target.user_id.as_str())
                && account.upstream_account_id.as_deref() == Some(target.workspace_id.as_str())
        });
    }
    let mut candidates = matches.iter().copied().filter(|account| {
        entry
            .preferred_workspace_id
            .as_ref()
            .is_none_or(|workspace| account.upstream_account_id.as_ref() == Some(workspace))
    });
    let first = candidates.next()?;
    candidates.next().is_none().then_some(first)
}

fn result(id: String, outcome: Result<(), AdminError>) -> ReloginBatchResult {
    match outcome {
        Ok(()) => ReloginBatchResult {
            id,
            success: true,
            message: "完成".to_owned(),
        },
        Err(error) => ReloginBatchResult {
            id,
            success: false,
            message: error.message().to_owned(),
        },
    }
}

#[async_trait]
impl ReloginService for DefaultReloginService {
    async fn templates(&self) -> Result<Vec<ReloginTemplate>, AdminError> {
        self.store()?
            .templates()
            .await
            .map_err(templates::template_error)
    }

    async fn save_template(
        &self,
        selection: Option<ReloginTemplateSelection>,
        mut config: ReloginTemplateConfig,
    ) -> Result<ReloginTemplate, AdminError> {
        config.name = config.name.trim().to_owned();
        config.settings()?;
        let (id, expected) = match selection {
            Some(selection) => {
                templates::validate_selection(&selection)?;
                (selection.id, Some(selection.revision))
            }
            None => (format!("template_{}", uuid::Uuid::now_v7().simple()), None),
        };
        let template = ReloginTemplate {
            id,
            revision: expected.unwrap_or(0) + 1,
            config,
        };
        self.store()?
            .save_template(&template, expected)
            .await
            .map_err(templates::template_error)?;
        Ok(template)
    }

    async fn delete_template(&self, selection: ReloginTemplateSelection) -> Result<(), AdminError> {
        templates::validate_selection(&selection)?;
        self.store()?
            .delete_template(&selection.id, selection.revision)
            .await
            .map_err(templates::template_error)
    }

    async fn account_actions(
        &self,
        ids: &[String],
    ) -> Result<Vec<AccountReloginAction>, AdminError> {
        validate_ids(ids)?;
        let gate = self.gate.lock().await;
        let entries = self.entries().await?;
        let pool = self.pool().await?;
        let settings = self.store()?.settings().await.map_err(store_error)?;
        let by_email: BTreeMap<_, _> = entries
            .iter()
            .filter(|entry| entry.validate_totp().is_ok())
            .map(|entry| (entry.email.to_ascii_lowercase(), entry))
            .collect();
        Ok(pool
            .iter()
            .filter(|account| ids.contains(&account.id) && account.authentication_kind == "oauth")
            .filter_map(|account| {
                let entry = by_email.get(&account.email.as_ref()?.to_ascii_lowercase())?;
                let target = ReloginTarget::from_account(account);
                let busy = entry.status.active() || gate.active.contains_key(&entry.email);
                let blocked_reason = if settings.paused {
                    Some("重登队列已暂停".to_owned())
                } else if busy {
                    Some("该邮箱已有重登任务".to_owned())
                } else {
                    target
                        .as_ref()
                        .err()
                        .map(|error| error.message().to_owned())
                };
                let owns_result = entry.target.as_ref().is_some_and(|target| {
                    target.account_id == account.id
                        && account.upstream_user_id.as_ref() == Some(&target.user_id)
                        && account.upstream_account_id.as_ref() == Some(&target.workspace_id)
                });
                Some(AccountReloginAction {
                    account_id: account.id.clone(),
                    entry_id: entry.id.clone(),
                    revision: entry.revision,
                    target: target.ok(),
                    status: if owns_result || busy {
                        entry.status
                    } else {
                        ReloginStatus::Pending
                    },
                    message: if owns_result || busy {
                        entry.message.clone()
                    } else {
                        String::new()
                    },
                    busy,
                    blocked_reason,
                    synced_at: owns_result.then_some(entry.synced_at).flatten(),
                })
            })
            .collect())
    }

    async fn queue_account(
        &self,
        entry_id: &str,
        revision: u64,
        target: &ReloginTarget,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        validate_ids(&[entry_id.to_owned()])?;
        validate_ids(std::slice::from_ref(&target.account_id))?;
        let gate = self.gate.lock().await;
        if self.store()?.settings().await.map_err(store_error)?.paused {
            return Err(AdminError::conflict("重登队列已暂停"));
        }
        let mut entry = self
            .entries()
            .await?
            .into_iter()
            .find(|entry| entry.id == entry_id)
            .ok_or_else(|| AdminError::not_found("重登资料不存在"))?;
        entry.validate_totp()?;
        if entry.revision != revision {
            return Err(AdminError::conflict("重登资料已变化，请重新确认"));
        }
        if entry.status.active() || gate.active.contains_key(&entry.email) {
            return Err(AdminError::conflict("该邮箱已有重登任务"));
        }
        let pool = self.pool().await?;
        let account = matching_accounts(&entry, &pool)
            .into_iter()
            .find(|account| account.id == target.account_id)
            .ok_or_else(|| AdminError::conflict("目标账号已删除或不再匹配该邮箱"))?;
        if !target.matches_account(account) {
            return Err(AdminError::conflict(
                "目标账号身份、工作区或凭据已变化，请重新确认",
            ));
        }
        entry.target = Some(target.clone());
        entry.automatic_job = false;
        entry.manual_push_context = Some(context.clone());
        entry.credential = None;
        entry.synced_at = None;
        entry.status = ReloginStatus::Queued;
        entry.message = "等待重登，验证后同步到指定账号".to_owned();
        self.save(&mut entry).await
    }

    async fn list(&self) -> Result<ReloginList, AdminError> {
        let mut entries = self.entries().await?;
        entries.sort_by(|left, right| {
            right
                .import_time()
                .cmp(&left.import_time())
                .then_with(|| right.id.cmp(&left.id))
        });
        let pool = self.pool().await?;
        let runtime = self
            .runtime
            .active_rate_limits()
            .await
            .map_err(store_error)?;
        let settings = self.store()?.settings().await.map_err(store_error)?;
        let now = Utc::now();
        let items = entries
            .into_iter()
            .map(|entry| {
                let recovery = recovery::view(&entry, &pool, &settings, now);
                let imported_at = entry.import_time();
                let matches = matching_accounts(&entry, &pool);
                let statistics = statistics_account(&entry, &matches);
                let material_error = entry.validate_totp().err();
                let has_totp = material_error.is_none();
                let credential = entry.credential.as_ref().filter(|_| has_totp);
                let uncertain = matches!(
                    entry.status,
                    ReloginStatus::Pushing | ReloginStatus::Uncertain
                );
                let pool_status = if matches.is_empty() {
                    "absent"
                } else if entry.synced_at.is_some()
                    && entry.target.as_ref().is_some_and(|target| {
                        matches.iter().any(|account| {
                            account.id == target.account_id
                                && account.upstream_user_id.as_ref() == Some(&target.user_id)
                                && account.upstream_account_id.as_ref()
                                    == Some(&target.workspace_id)
                        })
                    })
                {
                    "synced"
                } else if credential.is_some() {
                    "pending_push"
                } else {
                    "present"
                };
                ReloginView {
                    recovery,
                    id: entry.id,
                    revision: entry.revision,
                    email: entry.email,
                    has_totp,
                    automatic: entry.automatic && has_totp,
                    status: if material_error.is_some() && !uncertain {
                        ReloginStatus::Failed
                    } else {
                        entry.status
                    },
                    message: if uncertain {
                        entry.message
                    } else {
                        material_error.map_or(entry.message, |error| error.message().to_owned())
                    },
                    preferred_workspace_id: entry.preferred_workspace_id,
                    plan_type: credential.map(|value| value.plan_type.clone()).or_else(|| {
                        matches
                            .first()
                            .and_then(|account| account.plan_type.clone())
                    }),
                    workspace_id: credential.map(|value| value.workspace_id.clone()),
                    credential_status: match credential {
                        None => "none",
                        Some(value) if value.expires_at <= now => "expired",
                        Some(_) => "verified",
                    },
                    pool_status,
                    pool_account_ids: matches.iter().map(|account| account.id.clone()).collect(),
                    pool_accounts: matches
                        .iter()
                        .map(|account| {
                            let projection = resolve_account_operational_status(
                                &AccountStatusFacts {
                                    enabled: account.enabled,
                                    credential_state: account.credential_state,
                                    access_token_expires_at: account
                                        .access_token_expires_at
                                        .map(Into::into),
                                    quota: account.quota,
                                    rate_limited_until: runtime
                                        .rate_limited_until
                                        .get(&account.id)
                                        .copied()
                                        .map(Into::into),
                                    last_error_reason: account.last_error_reason,
                                    last_error_message: account.last_error_message.clone(),
                                },
                                now.into(),
                            );
                            ReloginPoolAccountView {
                                id: account.id.clone(),
                                workspace_id: account.upstream_account_id.clone(),
                                plan_type: account.plan_type.clone(),
                                enabled: account.enabled,
                                status: projection.status.as_str(),
                                error_reason: projection.error_reason.map(|reason| reason.as_str()),
                                error_message: projection.error_message,
                            }
                        })
                        .collect(),
                    relogin_account_id: statistics.map(|account| account.id.clone()),
                    relogin_count: statistics
                        .map(|account| account.relogin_count)
                        .or_else(|| matches.is_empty().then_some(0)),
                    last_relogin_at: statistics.and_then(|account| account.last_relogin_at),
                    verified_at: credential.map(|value| value.verified_at),
                    expires_at: credential.map(|value| value.expires_at),
                    imported_at,
                    updated_at: entry.updated_at,
                }
            })
            .collect();
        Ok(ReloginList { settings, items })
    }

    async fn import(&self, text: &str, replace: bool) -> Result<usize, AdminError> {
        let inputs = parse_relogin_import(text)?;
        let gate = self.gate.lock().await;
        let entries = self.entries().await?;
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
        let count = inputs.len();
        let mut changes = Vec::with_capacity(count);
        for input in inputs {
            if let Some(old) = existing.get(&input.email) {
                let mut entry = old.clone();
                Self::stop(&gate, &mut entry);
                entry.password = input.password;
                entry.mfa_secret = input.mfa_secret;
                entry.automatic_attempts = 0;
                entry.next_attempt_at = None;
                entry.status = ReloginStatus::Pending;
                entry.credential = None;
                entry.synced_at = None;
                entry.target = None;
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
                    manual_push_context: None,
                    automatic_attempts: 0,
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
        self.store()?
            .save_batch(&changes)
            .await
            .map_err(store_error)?;
        Ok(count)
    }

    async fn queue(&self, ids: &[String]) -> Result<Vec<ReloginBatchResult>, AdminError> {
        validate_ids(ids)?;
        let gate = self.gate.lock().await;
        let entries = self.entries().await?;
        let pool = self.pool().await?;
        let mut results = Vec::new();
        for id in ids {
            let outcome = async {
                let mut entry = entries
                    .iter()
                    .find(|entry| &entry.id == id)
                    .cloned()
                    .ok_or_else(|| AdminError::not_found("资料不存在"))?;
                entry.validate_totp()?;
                if entry.status.active() || gate.active.contains_key(&entry.email) {
                    return Err(AdminError::conflict("账号已有活动任务"));
                }
                entry.target = select_target(&entry, &pool)?;
                entry.automatic_job = false;
                entry.manual_push_context = None;
                entry.status = ReloginStatus::Queued;
                entry.message = "等待重登".to_owned();
                self.save(&mut entry).await
            }
            .await;
            results.push(result(id.clone(), outcome));
        }
        Ok(results)
    }

    async fn push(
        &self,
        ids: &[String],
        revisions: &BTreeMap<String, u64>,
        context: &MutationContext,
    ) -> Result<Vec<ReloginBatchResult>, AdminError> {
        self.push_with_template(ids, revisions, None, context).await
    }

    async fn push_with_template(
        &self,
        ids: &[String],
        revisions: &BTreeMap<String, u64>,
        template: Option<ReloginTemplateSelection>,
        context: &MutationContext,
    ) -> Result<Vec<ReloginBatchResult>, AdminError> {
        validate_ids(ids)?;
        let _gate = self.gate.lock().await;
        let template = self.resolve_template(template).await?;
        let entries = self.entries().await?;
        let mut results = Vec::new();
        for id in ids {
            let outcome = match entries.iter().find(|entry| &entry.id == id) {
                Some(entry) if revisions.get(id) == Some(&entry.revision) => {
                    self.push_entry_with_template(
                        &mut entry.clone(),
                        template.as_ref().map(|template| &template.config),
                        context,
                    )
                    .await
                }
                Some(_) => Err(AdminError::conflict("资料已变化，请刷新列表后重新确认推送")),
                None => Err(AdminError::not_found("资料不存在")),
            };
            results.push(result(id.clone(), outcome));
        }
        Ok(results)
    }

    async fn delete(&self, ids: &[String]) -> Result<(), AdminError> {
        validate_ids(ids)?;
        let gate = self.gate.lock().await;
        for mut entry in self
            .entries()
            .await?
            .into_iter()
            .filter(|entry| ids.contains(&entry.id))
        {
            Self::stop(&gate, &mut entry);
            self.store()?
                .delete(&entry.id, entry.revision)
                .await
                .map_err(store_error)?;
        }
        Ok(())
    }

    async fn automatic(&self, ids: &[String], enabled: bool) -> Result<(), AdminError> {
        validate_ids(ids)?;
        let gate = self.gate.lock().await;
        for mut entry in self
            .entries()
            .await?
            .into_iter()
            .filter(|entry| ids.contains(&entry.id))
        {
            if enabled {
                entry.validate_totp()?;
            }
            if entry.automatic == enabled {
                continue;
            }
            Self::stop(&gate, &mut entry);
            entry.automatic = enabled;
            self.save(&mut entry).await?;
        }
        Ok(())
    }

    async fn workspace(&self, id: &str, workspace: Option<String>) -> Result<(), AdminError> {
        if workspace.as_ref().is_some_and(|id| {
            id.is_empty() || id.len() > 128 || id.chars().any(char::is_whitespace)
        }) {
            return Err(AdminError::invalid("工作区 ID 不合法"));
        }
        let gate = self.gate.lock().await;
        let mut entry = self
            .entries()
            .await?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| AdminError::not_found("资料不存在"))?;
        if let Some(workspace) = &workspace {
            let pool = self.pool().await?;
            let known = matching_accounts(&entry, &pool)
                .iter()
                .any(|account| account.upstream_account_id.as_ref() == Some(workspace))
                || entry
                    .credential
                    .as_ref()
                    .is_some_and(|credential| &credential.workspace_id == workspace);
            if !known {
                return Err(AdminError::invalid("请选择该邮箱已识别的工作区"));
            }
        }
        Self::stop(&gate, &mut entry);
        entry.preferred_workspace_id = workspace;
        entry.credential = None;
        entry.target = None;
        entry.synced_at = None;
        entry.automatic_attempts = 0;
        entry.status = ReloginStatus::Pending;
        entry.message = "工作区选择已更新，等待重新获取凭据".to_owned();
        self.save(&mut entry).await
    }

    async fn configure(&self, settings: ReloginSettings) -> Result<(), AdminError> {
        settings.validate()?;
        let gate = self.gate.lock().await;
        self.store()?
            .save_settings(&settings)
            .await
            .map_err(store_error)?;
        if settings.paused {
            for mut entry in self
                .entries()
                .await?
                .into_iter()
                .filter(|entry| entry.status == ReloginStatus::Running)
            {
                Self::stop(&gate, &mut entry);
                self.save(&mut entry).await?;
            }
        }
        Ok(())
    }
}
