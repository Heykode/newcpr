//! Credential-free, versioned settings shared by account updates and relogin imports.

use super::{AdminError, accounts::AccountImportSettings};
use gateway_core::{
    account::{AccountConcurrencyLimit, AccountWeight},
    routing::AccountGroupId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Default)]
pub struct ReloginNewAccountOptions {
    pub template: Option<ReloginTemplateSelection>,
    pub custom_name: Option<String>,
    pub excel: Option<ExcelImportSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExcelImportSettings {
    pub responses_upstream: gateway_core::account::ResponsesUpstream,
    pub excel_models_follow_global: bool,
    #[serde(default)]
    pub excel_cache_creation_as_input: bool,
    #[serde(default)]
    pub excel_auto_disable_on_403: bool,
    pub excel_models: Option<gateway_core::account::ExcelModels>,
}

impl ExcelImportSettings {
    pub fn validate(&self) -> Result<(), AdminError> {
        if !self.excel_models_follow_global && self.excel_models.is_none() {
            return Err(AdminError::invalid("自定义模式必须提供 Excel 模型列表"));
        }
        Ok(())
    }

    pub fn apply(&self, settings: &mut AccountImportSettings) {
        settings.responses_upstream = Some(self.responses_upstream);
        settings.excel_cache_creation_as_input = Some(self.excel_cache_creation_as_input);
        settings.excel_auto_disable_on_403 = Some(self.excel_auto_disable_on_403);
        settings.excel_models_follow_global = Some(self.excel_models_follow_global);
        settings.excel_models = if self.excel_models_follow_global {
            None
        } else {
            self.excel_models.clone()
        };
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReloginTemplateConfig {
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub turn_state_injection_enabled: Option<bool>,
    #[serde(default)]
    pub responses_upstream: Option<gateway_core::account::ResponsesUpstream>,
    #[serde(default)]
    pub excel_models: Option<gateway_core::account::ExcelModels>,
    #[serde(default)]
    pub excel_models_follow_global: Option<bool>,
    #[serde(default)]
    pub excel_cache_creation_as_input: Option<bool>,
    #[serde(default)]
    pub excel_auto_disable_on_403: Option<bool>,
    pub concurrency_limit: Option<u32>,
    pub weight: u16,
    pub group_ids: Vec<String>,
    pub outbound_proxy_id: Option<String>,
}

impl ReloginTemplateConfig {
    pub fn settings(&self) -> Result<AccountImportSettings, AdminError> {
        if self.name.trim().is_empty()
            || self.name.len() > 128
            || self.name.chars().any(char::is_control)
        {
            return Err(AdminError::invalid("模板名称不能为空且不能超过 128 字节"));
        }
        if self.group_ids.len() > 1000
            || self.group_ids.iter().collect::<BTreeSet<_>>().len() != self.group_ids.len()
        {
            return Err(AdminError::invalid("模板分组重复或过多"));
        }
        if let Some(id) = &self.outbound_proxy_id {
            validate_template_id(id)?;
        }
        Ok(AccountImportSettings {
            model_access: None,
            custom_name: None,
            enabled: self.enabled,
            turn_state_injection_enabled: self.turn_state_injection_enabled,
            responses_upstream: self.responses_upstream,
            excel_models: self.excel_models.clone(),
            excel_models_follow_global: self.excel_models_follow_global,
            excel_cache_creation_as_input: self.excel_cache_creation_as_input,
            excel_auto_disable_on_403: self.excel_auto_disable_on_403,
            concurrency_limit: self
                .concurrency_limit
                .map(|value| {
                    AccountConcurrencyLimit::new(value)
                        .ok_or_else(|| AdminError::invalid("账号并发限制必须大于零"))
                })
                .transpose()?,
            weight: AccountWeight::new(self.weight)
                .ok_or_else(|| AdminError::invalid("权重必须为 1 至 100"))?,
            group_ids: self
                .group_ids
                .iter()
                .cloned()
                .map(|id| {
                    AccountGroupId::new(id).map_err(|_| AdminError::invalid("分组 ID 不合法"))
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReloginTemplate {
    pub id: String,
    pub revision: u64,
    pub config: ReloginTemplateConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReloginTemplateSelection {
    pub id: String,
    pub revision: u64,
}

pub fn validate_template_id(id: &str) -> Result<(), AdminError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.:".contains(&byte))
    {
        return Err(AdminError::invalid("模板或代理 ID 不合法"));
    }
    Ok(())
}
