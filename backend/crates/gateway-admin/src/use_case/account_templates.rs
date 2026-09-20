//! One template catalog for existing-account settings and create-only relogin imports.

use super::{accounts::AccountsService, map_store_error};
use crate::{
    model::{
        AdminError, MutationContext,
        accounts::{AccountsUpdateResult, BatchUpdateAccounts},
        proxies::AccountProxySelection,
        relogin_templates::{
            ReloginTemplate, ReloginTemplateConfig, ReloginTemplateSelection, validate_template_id,
        },
    },
    ports::{
        relogin::ReloginStore,
        store::{AdminStoreError, AdminStoreErrorKind},
    },
};
use async_trait::async_trait;
use gateway_core::account::ProviderAccountId;
use std::{collections::BTreeSet, sync::Arc};

#[async_trait]
pub trait AccountTemplatesService: Send + Sync {
    async fn templates(&self) -> Result<Vec<ReloginTemplate>, AdminError>;
    async fn save_template(
        &self,
        selection: Option<ReloginTemplateSelection>,
        config: ReloginTemplateConfig,
    ) -> Result<ReloginTemplate, AdminError>;
    async fn delete_template(&self, selection: ReloginTemplateSelection) -> Result<(), AdminError>;
    async fn resolve(
        &self,
        selection: ReloginTemplateSelection,
    ) -> Result<ReloginTemplate, AdminError>;
    async fn apply(
        &self,
        account_ids: Vec<String>,
        selection: ReloginTemplateSelection,
        context: &MutationContext,
    ) -> Result<AccountsUpdateResult, AdminError>;
}

pub(crate) struct DefaultAccountTemplatesService {
    store: Option<Arc<dyn ReloginStore>>,
    accounts: Arc<dyn AccountsService>,
}

impl DefaultAccountTemplatesService {
    pub(crate) fn new(
        store: Option<Arc<dyn ReloginStore>>,
        accounts: Arc<dyn AccountsService>,
    ) -> Self {
        Self { store, accounts }
    }

    fn store(&self) -> Result<&dyn ReloginStore, AdminError> {
        self.store
            .as_deref()
            .ok_or_else(|| AdminError::unavailable("账号模板库未配置"))
    }
}

fn validate_selection(selection: &ReloginTemplateSelection) -> Result<(), AdminError> {
    validate_template_id(&selection.id)?;
    if selection.revision == 0 || selection.revision >= 9_007_199_254_740_991 {
        return Err(AdminError::invalid("模板版本不合法"));
    }
    Ok(())
}

pub(super) fn template_error(error: AdminStoreError) -> AdminError {
    match error.kind() {
        AdminStoreErrorKind::Invalid => {
            AdminError::invalid("模板分组或代理不可用，或模板数量已达上限，请检查模板")
        }
        AdminStoreErrorKind::Conflict | AdminStoreErrorKind::StaleRevision => {
            AdminError::conflict("模板名称重复或版本已变化，请刷新模板后重试")
        }
        _ => map_store_error(error, "account templates"),
    }
}

#[async_trait]
impl AccountTemplatesService for DefaultAccountTemplatesService {
    async fn templates(&self) -> Result<Vec<ReloginTemplate>, AdminError> {
        self.store()?.templates().await.map_err(template_error)
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
                validate_selection(&selection)?;
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
            .map_err(template_error)?;
        Ok(template)
    }

    async fn delete_template(&self, selection: ReloginTemplateSelection) -> Result<(), AdminError> {
        validate_selection(&selection)?;
        self.store()?
            .delete_template(&selection.id, selection.revision)
            .await
            .map_err(template_error)
    }

    async fn resolve(
        &self,
        selection: ReloginTemplateSelection,
    ) -> Result<ReloginTemplate, AdminError> {
        validate_selection(&selection)?;
        let template = self
            .templates()
            .await?
            .into_iter()
            .find(|template| template.id == selection.id)
            .ok_or_else(|| AdminError::conflict("模板已删除，请重新选择"))?;
        if template.revision != selection.revision {
            return Err(AdminError::conflict("模板已修改，请重新选择并确认配置"));
        }
        template.config.settings()?;
        Ok(template)
    }

    async fn apply(
        &self,
        account_ids: Vec<String>,
        selection: ReloginTemplateSelection,
        context: &MutationContext,
    ) -> Result<AccountsUpdateResult, AdminError> {
        if account_ids.is_empty()
            || account_ids.len() > 1000
            || account_ids.iter().collect::<BTreeSet<_>>().len() != account_ids.len()
            || account_ids
                .iter()
                .any(|id| ProviderAccountId::new(id.clone()).is_err())
        {
            return Err(AdminError::invalid("请选择 1 至 1000 个不重复的有效账号"));
        }
        let template = self.resolve(selection).await?;
        self.store()?
            .validate_template_references(&template.config)
            .await
            .map_err(template_error)?;
        let settings = template.config.settings()?;
        self.accounts
            .batch_update(
                context,
                BatchUpdateAccounts {
                    account_ids,
                    enabled: Some(settings.enabled),
                    turn_state_injection_enabled: settings.turn_state_injection_enabled,
                    concurrency_limit: Some(settings.concurrency_limit),
                    weight: Some(settings.weight),
                    group_ids: Some(settings.group_ids),
                    outbound_proxy: Some(match template.config.outbound_proxy_id {
                        Some(id) => AccountProxySelection::Saved(id),
                        None => AccountProxySelection::Direct,
                    }),
                },
            )
            .await
    }
}
