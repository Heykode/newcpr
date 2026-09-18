//! 多 Provider 账号目录与连接测试的公共事实。

use std::{collections::BTreeMap, pin::Pin, str::FromStr};

use chrono::{DateTime, Utc};
use futures::Stream;

use gateway_core::{
    engine::probe::AccountProbeErrorSource, error::GatewayErrorKind, routing::ProviderKind,
    upstream::UpstreamSendState,
};

use super::{
    AdminModelError, PageSize, Revision, account_groups::AccountGroupRef, observability::TimeRange,
};

pub use gateway_core::account::{
    AccountConcurrencyLimit, AccountErrorReason, AccountStatus, AccountStatusFacts,
    AccountStatusProjection, AccountWeight, CredentialState, QuotaAccessState, QuotaEvidence,
    QuotaState, resolve_account_status,
};

/// 导入时统一应用的账号调度与分组设置；缺省时保留原有导入语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountImportSettings {
    pub enabled: bool,
    pub concurrency_limit: Option<AccountConcurrencyLimit>,
    pub weight: AccountWeight,
    pub group_ids: Vec<gateway_core::routing::AccountGroupId>,
}

/// 账号列表排序字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountSortField {
    Email,
    Status,
    PlanType,
    Usage,
    LastUsedAt,
    ReloginCount,
    CreatedAt,
    ExpiresAt,
}

/// 账号列表排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

/// 一组完整的账号排序规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountSort {
    pub field: AccountSortField,
    pub direction: SortDirection,
}

/// 账号列表的存储查询条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountListQuery {
    pub page: u32,
    pub page_size: PageSize,
    pub provider_kind: Option<ProviderKind>,
    pub group_filter: Option<AccountGroupFilter>,
    pub search: Option<String>,
    pub status: Option<AccountStatus>,
    pub plan_type: Option<String>,
    pub sort: Option<AccountSort>,
}

/// Admin query service 从运行态存储取得的当前账号冷却快照。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountRuntimeSnapshot {
    pub rate_limited_until: BTreeMap<String, DateTime<Utc>>,
    /// `None` 表示实时 lease 存储不可用；`Some` 中未出现的账号当前使用量为零。
    pub in_flight: Option<BTreeMap<String, u64>>,
}

/// Optional account membership filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountGroupFilter {
    Group(gateway_core::routing::AccountGroupId),
    Ungrouped,
}

/// Safe account/model readiness projection; never contains opaque State values.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountTurnStateStatus {
    pub required_models: Vec<String>,
    pub ready_models: Vec<(String, DateTime<Utc>)>,
}

/// 账号公共存储投影；Provider 专属字段不进入此结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    pub id: String,
    pub provider_kind: ProviderKind,
    pub groups: Vec<AccountGroupRef>,
    pub name: String,
    pub email: Option<String>,
    pub upstream_user_id: Option<String>,
    pub upstream_account_id: Option<String>,
    pub plan_type: Option<String>,
    pub authentication_kind: String,
    pub credential_revision: Revision,
    pub relogin_count: u64,
    pub last_relogin_at: Option<DateTime<Utc>>,
    pub has_refresh_token: bool,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub next_refresh_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub turn_state_injection_enabled: bool,
    pub concurrency_limit: Option<AccountConcurrencyLimit>,
    pub weight: AccountWeight,
    pub outbound_proxy: Option<gateway_core::account::OutboundProxy>,
    pub credential_state: CredentialState,
    pub credential_observed_at: DateTime<Utc>,
    pub quota: QuotaState,
    pub last_error_reason: Option<AccountErrorReason>,
    pub last_error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 单一货币的账号成本聚合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCost {
    pub currency: String,
    pub amount: super::observability::DecimalAmount,
}

/// 单一货币的累计成本；金额范围独立于单次请求和额度窗口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCumulativeCost {
    pub currency: String,
    pub amount: CumulativeCostAmount,
}

/// `numeric(40,10)` 的非负规范累计金额，不参与现有请求费用计算。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CumulativeCostAmount(String);

impl CumulativeCostAmount {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CumulativeCostAmount {
    type Err = AdminModelError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        let mut parts = input.split('.');
        let whole = parts.next().unwrap_or_default();
        let fraction = parts.next();
        let valid = !whole.is_empty()
            && whole.len() <= 30
            && whole.bytes().all(|byte| byte.is_ascii_digit())
            && parts.next().is_none()
            && fraction.is_none_or(|value| {
                !value.is_empty()
                    && value.len() <= 10
                    && value.bytes().all(|byte| byte.is_ascii_digit())
            });
        if !valid {
            return Err(AdminModelError::InvalidCumulativeCostAmount);
        }
        let whole = whole.trim_start_matches('0');
        let whole = if whole.is_empty() { "0" } else { whole };
        let fraction = fraction.unwrap_or_default().trim_end_matches('0');
        let canonical = if fraction.is_empty() {
            whole.to_owned()
        } else {
            format!("{whole}.{fraction}")
        };
        Ok(Self(canonical))
    }
}

impl std::fmt::Display for CumulativeCostAmount {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 账号在一个模型上的历史用量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountModelUsage {
    pub model: String,
    pub request_count: u64,
    pub success_count: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub image_input_tokens: Option<u64>,
    pub image_output_tokens: Option<u64>,
    pub image_request_count: u64,
    pub image_request_failed_count: u64,
    pub total_tokens: Option<u64>,
    pub cost_coverage: super::observability::CostCoverage,
    pub costs: Vec<AccountCost>,
    pub last_used_at: DateTime<Utc>,
}

/// 账号在一个小时窗口内的请求数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRequestBucket {
    pub bucket_start: DateTime<Utc>,
    pub request_count: u64,
    pub success_count: u64,
    pub error_count: u64,
    pub non_completion_count: u64,
}

/// 账号历史用量聚合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountUsage {
    pub account_id: String,
    pub request_count: u64,
    pub success_count: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub image_input_tokens: Option<u64>,
    pub image_output_tokens: Option<u64>,
    pub image_request_count: u64,
    pub image_request_failed_count: u64,
    pub total_tokens: Option<u64>,
    pub cost_coverage: super::observability::CostCoverage,
    pub costs: Vec<AccountCost>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub request_buckets: Vec<AccountRequestBucket>,
    pub models: Vec<AccountModelUsage>,
}

/// 某个账号在调用方指定时间窗口内的本地用量查询。
///
/// `key` 只用于把聚合结果关联回调用方的窗口，不承载 Provider 私有语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountUsageWindowQuery {
    pub account_id: String,
    pub key: String,
    pub range: TimeRange,
}

/// 一个账号时间窗口用量查询的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountUsageWindowResult {
    pub account_id: String,
    pub key: String,
    pub usage: AccountUsage,
}

/// 账号列表页所需的完整存储事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPage {
    pub config_revision: Revision,
    pub items: Vec<AccountPageItem>,
    pub total: u64,
    pub summary: AccountSummary,
}

/// 同一状态快照下的账号事实与唯一状态投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPageItem {
    pub account: AccountRecord,
    pub projection: AccountStatusProjection,
    pub in_flight: Option<u64>,
}

/// 统一账号目录的全局状态计数，不受当前筛选和分页影响。
///
/// 管理页五态互斥；暂停账号只计入 disabled，不再计入正常或异常状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountSummary {
    pub total: u64,
    pub normal: u64,
    pub quota_exhausted: u64,
    pub rate_limited: u64,
    pub disabled: u64,
    pub error: u64,
}

/// 账号可编辑事实的一次性替换命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateAccount {
    pub account_id: String,
    pub enabled: bool,
    pub turn_state_injection_enabled: Option<bool>,
    pub concurrency_limit: Option<AccountConcurrencyLimit>,
    pub weight: AccountWeight,
    pub group_ids: Vec<gateway_core::routing::AccountGroupId>,
    pub outbound_proxy: Option<super::proxies::AccountProxySelection>,
}

/// 账号更新结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountUpdateResult {
    pub config_revision: Revision,
    pub account_id: gateway_core::account::ProviderAccountId,
}

/// 一批账号可编辑事实的一次性替换命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchUpdateAccounts {
    pub account_ids: Vec<String>,
    pub enabled: Option<bool>,
    pub turn_state_injection_enabled: Option<bool>,
    pub concurrency_limit: Option<Option<AccountConcurrencyLimit>>,
    pub weight: Option<AccountWeight>,
    pub group_ids: Option<Vec<gateway_core::routing::AccountGroupId>>,
    pub outbound_proxy: Option<super::proxies::AccountProxySelection>,
}

/// 批量账号更新结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountsUpdateResult {
    pub config_revision: Revision,
    pub account_ids: Vec<gateway_core::account::ProviderAccountId>,
}

/// 账号连接测试所使用的接口语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionTestEndpoint {
    Responses,
    Completions,
}

impl ConnectionTestEndpoint {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::Completions => "completions",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Responses => "Responses",
            Self::Completions => "Completions",
        }
    }
}

/// 账号连接测试命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountConnectionTest {
    pub account_id: gateway_core::account::ProviderAccountId,
    pub upstream_model: gateway_core::routing::UpstreamModelId,
    pub endpoint: ConnectionTestEndpoint,
    pub input_text: String,
    pub stream: bool,
}

/// 账号批量删除命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteAccounts {
    pub account_ids: Vec<String>,
}

/// 账号连接测试的语义事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountConnectionTestEvent {
    Started {
        model: String,
        endpoint: ConnectionTestEndpoint,
    },
    Request {
        model: String,
        input_text: String,
        endpoint: ConnectionTestEndpoint,
        stream: bool,
        store: bool,
    },
    Content {
        text: String,
    },
    Completed,
    Failed {
        source: AccountProbeErrorSource,
        gateway_error_code: GatewayErrorKind,
        send_state: Option<UpstreamSendState>,
        message: String,
        provider_error_code: Option<String>,
        provider_error_type: Option<String>,
        upstream_status: Option<u16>,
        upstream_content_type: Option<String>,
        upstream_body: Option<String>,
    },
}

/// 每次连接测试独占的有限事件流。
pub type AccountConnectionTestEventStream =
    Pin<Box<dyn Stream<Item = AccountConnectionTestEvent> + Send + 'static>>;
