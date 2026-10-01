use super::{
    AdminError, MutationContext,
    provider_credentials::{ProviderResetCredit, ProviderResetCreditResult, ProviderResetCredits},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetInventory {
    pub account_id: String,
    pub checked_at: DateTime<Utc>,
    pub credits: Option<ProviderResetCredits>,
    pub error: Option<String>,
    pub pending: Option<ResetPending>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetPending {
    pub redeem_request_id: Uuid,
    pub credit_id: Option<String>,
    pub retry_after: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetItemStatus {
    Ready,
    Queued,
    Running,
    Succeeded,
    Skipped,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetBatchItem {
    pub account_id: String,
    pub available_count: Option<u64>,
    pub credit: Option<ProviderResetCredit>,
    pub redeem_request_id: Uuid,
    pub status: ResetItemStatus,
    pub message: String,
    pub updated_at: DateTime<Utc>,
    pub claim_id: Option<Uuid>,
    pub retry: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetBatch {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub confirmed: bool,
    pub reset_type: Option<String>,
    pub items: Vec<ResetBatchItem>,
    #[serde(default, skip_serializing)]
    pub context: Option<MutationContext>,
}

#[derive(Debug, Clone)]
pub enum ResetConsumePermit {
    Execute(Uuid),
    Completed(ProviderResetCreditResult),
}

/// Never rely on upstream list order, nor silently choose between different windows.
pub fn earliest_credit(
    inventory: &ProviderResetCredits,
    reset_type: Option<&str>,
    now: DateTime<Utc>,
) -> Result<ProviderResetCredit, AdminError> {
    if inventory.available_count == 0 {
        return Err(AdminError::invalid("没有可用重置次数"));
    }
    let mut credits: Vec<_> = inventory
        .credits
        .iter()
        .filter(|c| c.status.as_deref() == Some("available"))
        .filter(|c| c.expires_at.is_none_or(|expires| expires > now))
        .filter(|c| reset_type.is_none_or(|kind| c.reset_type.as_deref() == Some(kind)))
        .collect();
    if credits.is_empty() {
        return Err(AdminError::invalid(
            "没有可选的有效重置卡，无法保证到期优先",
        ));
    }
    let first_type = &credits[0].reset_type;
    if credits.iter().any(|c| &c.reset_type != first_type) {
        return Err(AdminError::invalid(
            "存在不同重置类型，请选择重置类型后重新预览",
        ));
    }
    if credits
        .iter()
        .any(|c| c.expires_at.is_none() || c.id.is_empty())
    {
        return Err(AdminError::invalid(
            "卡片缺少到期时间或标识，请在单账号中核对",
        ));
    }
    let ids: std::collections::BTreeSet<_> = credits.iter().map(|c| &c.id).collect();
    if ids.len() != credits.len() {
        return Err(AdminError::invalid("上游返回重复卡片标识，请刷新后核对"));
    }
    credits.sort_by(|a, b| a.expires_at.cmp(&b.expires_at).then(a.id.cmp(&b.id)));
    Ok(credits[0].clone())
}
