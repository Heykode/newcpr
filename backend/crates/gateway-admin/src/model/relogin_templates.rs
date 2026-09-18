//! Credential-free, versioned defaults for create-only relogin imports.

use super::{AdminError, accounts::AccountImportSettings};
use gateway_core::{
    account::{AccountConcurrencyLimit, AccountWeight},
    routing::AccountGroupId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReloginTemplateConfig {
    pub name: String,
    pub enabled: bool,
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
            enabled: self.enabled,
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
