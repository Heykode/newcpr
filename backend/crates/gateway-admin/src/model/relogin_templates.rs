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
    pub form: Option<ReloginNewAccountForm>,
}

/// Final settings from the shared account-import form, applied only to new accounts.
pub struct ReloginNewAccountForm {
    pub settings: AccountImportSettings,
    pub outbound_proxy_id: Option<String>,
    pub quality_template: Option<super::quality_ops::QualityTemplateSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReloginTemplateConfig {
    #[serde(
        default,
        deserialize_with = "deserialize_credential_proxy",
        skip_serializing_if = "Option::is_none"
    )]
    pub credential_proxy_id: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "discard_retired_setting"
    )]
    pub excel_recovery: Option<()>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_access: Option<gateway_core::account::AccountModelAccess>,
    #[serde(
        default,
        deserialize_with = "deserialize_egress_mode",
        skip_serializing_if = "Option::is_none"
    )]
    pub egress_mode: Option<Option<gateway_core::provider_ports::egress::EgressMode>>,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub turn_state_injection_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "discard_retired_setting"
    )]
    pub responses_upstream: Option<gateway_core::account::ResponsesUpstream>,
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "discard_retired_setting"
    )]
    pub excel_models: Option<gateway_core::account::ExcelModels>,
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "discard_retired_setting"
    )]
    pub excel_models_follow_global: Option<bool>,
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "discard_retired_setting"
    )]
    pub excel_cache_creation_as_input: Option<bool>,
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "discard_retired_setting"
    )]
    pub excel_ignore_encrypted_content: Option<bool>,
    pub request_proxy_source:
        Option<gateway_core::provider_ports::session_proxy::RequestProxySource>,
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "discard_retired_setting"
    )]
    pub excel_auto_disable_on_403: Option<bool>,
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "discard_retired_setting"
    )]
    pub excel_403_action: Option<gateway_core::account::Excel403Action>,
    pub concurrency_limit: Option<u32>,
    pub weight: u16,
    pub group_ids: Vec<String>,
    #[serde(default)]
    pub preserve_outbound_proxy: bool,
    pub outbound_proxy_id: Option<String>,
}

impl ReloginTemplateConfig {
    pub fn settings(&self) -> Result<AccountImportSettings, AdminError> {
        if self.preserve_outbound_proxy && self.outbound_proxy_id.is_some() {
            return Err(AdminError::invalid("保持原代理与指定代理不能同时设置"));
        }
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
            if self
                .egress_mode
                .flatten()
                .is_some_and(|mode| mode.is_active())
            {
                return Err(AdminError::invalid("账号代理与 IPv6 出口策略不能同时启用"));
            }
        }
        if let Some(Some(id)) = &self.credential_proxy_id {
            validate_template_id(id)?;
        }
        Ok(AccountImportSettings {
            credential_proxy_id: self.credential_proxy_id.clone(),
            template_proxy_mode: None,
            purchase_cost: None,
            clear_outbound_proxy: !self.preserve_outbound_proxy && self.outbound_proxy_id.is_none(),
            egress_mode: self.egress_mode,
            model_access: self.model_access.clone(),
            custom_name: None,
            enabled: self.enabled,
            turn_state_injection_enabled: self.turn_state_injection_enabled,
            request_proxy_source: self.request_proxy_source,
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

fn deserialize_egress_mode<'de, D>(
    deserializer: D,
) -> Result<Option<Option<gateway_core::provider_ports::egress::EgressMode>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<gateway_core::provider_ports::egress::EgressMode>::deserialize(deserializer).map(Some)
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

fn deserialize_credential_proxy<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

/// Only explicitly named retired fields use this reader; unknown fields still fail closed.
pub fn discard_retired_setting<'de, D, T: Default>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(T::default())
}
