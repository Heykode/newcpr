//! 管理控制面所需的持久化能力。
//!
//! 端口按业务资源拆分，方法使用领域模型，不暴露连接池、事务或 Redis client。

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::BoxStream;
use gateway_core::routing::ProviderKind;

use super::backup::BackupStorePorts;
use crate::model::{
    MutationContext, Revision,
    account_groups::{
        AccountGroupListQuery, AccountGroupMemberFact, AccountGroupMutation, AccountGroupPage,
        DeleteAccountGroup, NewAccountGroup, SetAccountGroupEnabled, UpdateAccountGroup,
    },
    accounts::{
        AccountCumulativeCost, AccountListQuery, AccountPage, AccountPageItem,
        AccountRuntimeSnapshot, AccountUpdateResult, AccountUsage, AccountUsageWindowQuery,
        AccountUsageWindowResult, AccountsUpdateResult, BatchUpdateAccounts, DeleteAccounts,
        UpdateAccount,
    },
    auth::{AdminAuditEvent, AdminSession},
    client_keys::{
        ClientKeyListQuery, ClientKeyPage, ClientKeyRecord, ClientKeySecret, DeleteClientKey,
        NewClientKey, SetClientKeyEnabled, UpdateClientKey,
    },
    egress::{ProviderEgressMutation, ReplaceProviderEgress, SetProviderAccountEgress},
    observability::{
        DashboardObservation, DashboardRuntimeSlots, DiagnosticDimension, DiagnosticObservation,
        OpsErrorPage, OpsErrorQuery, RequestMetricPoint, TimeRange, UsageCalculatedBillingFact,
        UsageDetail, UsageFilter, UsageOverview, UsagePage, UsageQuery,
    },
    provider_credentials::{
        AuthorizationCommit, CredentialDetails, CredentialImportCommit, CredentialImportResult,
        CredentialListQuery, CredentialMutationResult, CredentialPage, CredentialRotationCommit,
        ProviderExportCredentialInput,
    },
    quota_forecast_sampling::QuotaForecastHistory,
    quota_learning::{QuotaLearningEstimate, QuotaLearningObservation},
    settings::{AdminApiKey, AdminApiKeyMutation, ReplaceRuntimeSettings, RuntimeSettings},
    user_agent::ProviderUserAgentOverride,
};

/// 管理端可判定的持久化失败类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminStoreErrorKind {
    Invalid,
    NotFound,
    StaleRevision,
    Conflict,
    Unavailable,
}

/// 隐藏数据库实现细节的持久化错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{resource} store operation failed: {message}")]
pub struct AdminStoreError {
    kind: AdminStoreErrorKind,
    resource: &'static str,
    message: String,
}

impl AdminStoreError {
    #[must_use]
    pub fn new(
        kind: AdminStoreErrorKind,
        resource: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            resource,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> AdminStoreErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn resource(&self) -> &'static str {
        self.resource
    }
}

pub type AdminStoreResult<T> = Result<T, AdminStoreError>;

#[async_trait]
pub trait ProviderEgressStore: Send + Sync {
    async fn load(
        &self,
    ) -> AdminStoreResult<gateway_core::provider_ports::egress::ProviderEgressConfig>;
    async fn replace(
        &self,
        command: ReplaceProviderEgress,
        context: &MutationContext,
    ) -> AdminStoreResult<ProviderEgressMutation>;
    async fn set_account(
        &self,
        command: SetProviderAccountEgress,
        context: &MutationContext,
    ) -> AdminStoreResult<ProviderEgressMutation>;
}

/// 账号目录与公共账号写操作。
#[async_trait]
pub trait AccountStore: Send + Sync {
    async fn load_turn_state_status(
        &self,
        _account_ids: &[String],
    ) -> AdminStoreResult<BTreeMap<String, crate::model::accounts::AccountTurnStateStatus>> {
        Ok(BTreeMap::new())
    }

    async fn list_accounts(
        &self,
        query: AccountListQuery,
        runtime: AccountRuntimeSnapshot,
    ) -> AdminStoreResult<AccountPage>;

    async fn load_account(
        &self,
        account_id: &str,
        runtime: AccountRuntimeSnapshot,
    ) -> AdminStoreResult<Option<AccountPageItem>>;

    async fn load_account_usage(
        &self,
        range: TimeRange,
        account_ids: &[String],
    ) -> AdminStoreResult<Vec<AccountUsage>>;

    async fn load_account_cumulative_costs(
        &self,
        account_ids: &[String],
    ) -> AdminStoreResult<BTreeMap<String, Vec<AccountCumulativeCost>>>;

    /// Load the short health timeline using five-minute request buckets.
    ///
    /// Stores that do not provide a specialized projection fall back to the
    /// regular usage query; the PostgreSQL adapter overrides this to keep the
    /// existing hourly dashboard contract unchanged.
    async fn load_account_health_timeline(
        &self,
        range: TimeRange,
        account_ids: &[String],
    ) -> AdminStoreResult<Vec<AccountUsage>> {
        self.load_account_usage(range, account_ids).await
    }

    async fn load_account_usage_by_windows(
        &self,
        windows: &[AccountUsageWindowQuery],
    ) -> AdminStoreResult<Vec<AccountUsageWindowResult>>;

    /// Read bounded observations and cumulative usage from one database snapshot.
    async fn load_quota_forecast_history(
        &self,
        window: &AccountUsageWindowQuery,
    ) -> AdminStoreResult<QuotaForecastHistory>;

    /// Persist quota observations and return the Tools-compatible effective
    /// capacity for each observed account/window.
    async fn record_quota_learning(
        &self,
        observations: &[QuotaLearningObservation],
    ) -> AdminStoreResult<Vec<QuotaLearningEstimate>> {
        let _ = observations;
        Ok(Vec::new())
    }

    async fn list_credentials(
        &self,
        provider_kind: &gateway_core::routing::ProviderKind,
        query: CredentialListQuery,
    ) -> AdminStoreResult<CredentialPage>;

    async fn credential_details(
        &self,
        provider_kind: &gateway_core::routing::ProviderKind,
        account_id: &gateway_core::account::ProviderAccountId,
    ) -> AdminStoreResult<Option<CredentialDetails>>;

    async fn load_credentials_for_export(
        &self,
        provider_kind: &gateway_core::routing::ProviderKind,
        account_ids: &[gateway_core::account::ProviderAccountId],
    ) -> AdminStoreResult<Vec<ProviderExportCredentialInput>>;

    async fn commit_credential_import(
        &self,
        command: CredentialImportCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialImportResult>;

    /// Same import initialization, but atomically refuse any existing identity.
    async fn commit_new_credential_import(
        &self,
        _command: CredentialImportCommit,
        _context: &MutationContext,
    ) -> AdminStoreResult<CredentialImportResult> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "credential import",
            "create-only import unavailable",
        ))
    }

    async fn commit_authorization(
        &self,
        command: AuthorizationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialMutationResult>;

    async fn commit_credential_rotation(
        &self,
        command: CredentialRotationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialMutationResult>;

    async fn commit_credential_refresh(
        &self,
        command: CredentialRotationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialMutationResult>;

    async fn update_account(
        &self,
        command: UpdateAccount,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountUpdateResult>;

    async fn recover_account(
        &self,
        account_id: &gateway_core::account::ProviderAccountId,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountUpdateResult>;

    async fn batch_update_accounts(
        &self,
        command: BatchUpdateAccounts,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountsUpdateResult>;

    async fn delete_accounts(
        &self,
        command: DeleteAccounts,
        context: &MutationContext,
    ) -> AdminStoreResult<Revision>;

    async fn record_credential_export(
        &self,
        account_ids: &[gateway_core::account::ProviderAccountId],
        context: &MutationContext,
    ) -> AdminStoreResult<()>;
}

/// 可丢失账号运行态的管理读端口；跨存储编排由 Admin application service 拥有。
#[async_trait]
pub trait AccountRuntimeStore: Send + Sync {
    async fn active_rate_limits(&self) -> AdminStoreResult<AccountRuntimeSnapshot>;

    async fn account_runtime(
        &self,
        account_ids: &[String],
    ) -> AdminStoreResult<AccountRuntimeSnapshot>;

    /// Read existing admission leases; unavailable is not zero occupancy.
    async fn client_in_flight(
        &self,
        _client_key_ids: &[String],
    ) -> AdminStoreResult<Option<std::collections::BTreeMap<String, u64>>> {
        Ok(None)
    }
}

/// 管理员密码、会话和安全审计。
#[async_trait]
pub trait AuthStore: Send + Sync {
    async fn load_password_hash(&self, admin_user_id: &str) -> AdminStoreResult<Option<String>>;

    async fn create_password_hash_if_absent(
        &self,
        admin_user_id: &str,
        password_hash: &str,
    ) -> AdminStoreResult<bool>;

    async fn load_admin_api_key(&self) -> AdminStoreResult<Option<AdminApiKey>>;

    async fn load_session(&self, session_id: &str) -> AdminStoreResult<Option<AdminSession>>;

    async fn store_session(&self, session_id: &str, session: &AdminSession)
    -> AdminStoreResult<()>;

    async fn delete_session(&self, session_id: &str) -> AdminStoreResult<Option<AdminSession>>;

    async fn append_audit_event(&self, event: AdminAuditEvent) -> AdminStoreResult<()>;
}

/// Client API Key 管理写入。
#[async_trait]
pub trait ClientKeyStore: Send + Sync {
    async fn list_client_keys(&self, query: ClientKeyListQuery) -> AdminStoreResult<ClientKeyPage>;

    async fn reveal_client_key(
        &self,
        id: &gateway_core::policy::ClientApiKeyId,
    ) -> AdminStoreResult<Option<ClientKeySecret>>;

    async fn create_client_key(
        &self,
        command: NewClientKey,
        context: &MutationContext,
    ) -> AdminStoreResult<(Revision, ClientKeyRecord)>;

    async fn update_client_key(
        &self,
        command: UpdateClientKey,
        context: &MutationContext,
    ) -> AdminStoreResult<(Revision, ClientKeyRecord)>;

    async fn set_client_key_enabled(
        &self,
        command: SetClientKeyEnabled,
        context: &MutationContext,
    ) -> AdminStoreResult<(Revision, ClientKeyRecord)>;

    async fn delete_client_key(
        &self,
        command: DeleteClientKey,
        context: &MutationContext,
    ) -> AdminStoreResult<Revision>;
}

/// Provider-neutral account group management transactions.
#[async_trait]
pub trait AccountGroupStore: Send + Sync {
    async fn load_group_monitor(
        &self,
        observed_at: chrono::DateTime<chrono::Utc>,
    ) -> AdminStoreResult<crate::model::group_monitor::GroupMonitorFacts>;

    async fn save_group_monitor(
        &self,
        report: &crate::model::group_monitor::GroupMonitorReport,
        config_revision: u64,
    ) -> AdminStoreResult<()>;

    async fn read_group_monitor(
        &self,
        group_ids: &[gateway_core::routing::AccountGroupId],
    ) -> AdminStoreResult<Option<crate::model::group_monitor::GroupMonitorReport>>;

    async fn list_account_groups(
        &self,
        query: AccountGroupListQuery,
    ) -> AdminStoreResult<AccountGroupPage>;

    async fn load_account_group_members(
        &self,
        group_ids: &[gateway_core::routing::AccountGroupId],
    ) -> AdminStoreResult<Vec<AccountGroupMemberFact>>;

    async fn create_account_group(
        &self,
        command: NewAccountGroup,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation>;

    async fn update_account_group(
        &self,
        command: UpdateAccountGroup,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation>;

    async fn set_account_group_enabled(
        &self,
        command: SetAccountGroupEnabled,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation>;

    async fn delete_account_group(
        &self,
        command: DeleteAccountGroup,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation>;
}

/// 逐条读取的已计算费用事实；消费结束或丢弃时释放查询资源。
pub type UsageCalculatedBillingStream<'a> =
    BoxStream<'a, AdminStoreResult<UsageCalculatedBillingFact>>;

/// 用量、趋势、诊断与运维错误的只读能力。
#[async_trait]
pub trait ObservabilityStore: Send + Sync {
    /// 返回历史统计区间和指定观测时刻下的实时账号状态。
    async fn dashboard_summary(
        &self,
        range: TimeRange,
        observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<DashboardObservation>;

    /// 返回 Dashboard 可选的实时槽位事实。
    ///
    /// 该状态来自可丢失的运行时存储；无实现或运行时存储不可用时返回 `None`，不影响
    /// 持久观测数据的读取。
    async fn dashboard_runtime_slots(
        &self,
        _observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<Option<DashboardRuntimeSlots>> {
        Ok(None)
    }

    async fn dashboard_trend(&self, range: TimeRange) -> AdminStoreResult<Vec<RequestMetricPoint>>;

    async fn usage_trend(
        &self,
        range: TimeRange,
        filter: UsageFilter,
    ) -> AdminStoreResult<Vec<RequestMetricPoint>>;

    /// 流式返回可由 Provider 重新校验的已计算费用事实，不保证顺序。
    /// 查询及解码错误由流返回；调用方应逐条聚合，避免收集整个区间。
    fn usage_calculated_billing_facts(
        &self,
        range: TimeRange,
        filter: UsageFilter,
    ) -> UsageCalculatedBillingStream<'_>;

    async fn list_usage_records(&self, query: UsageQuery) -> AdminStoreResult<UsagePage>;

    async fn usage_record_detail(&self, request_id: &str) -> AdminStoreResult<UsageDetail>;

    async fn usage_summary(
        &self,
        range: TimeRange,
        filter: UsageFilter,
    ) -> AdminStoreResult<UsageOverview>;

    async fn usage_diagnostics(
        &self,
        range: TimeRange,
        filter: UsageFilter,
        dimension: DiagnosticDimension,
    ) -> AdminStoreResult<Vec<DiagnosticObservation>>;

    async fn list_ops_errors(&self, query: OpsErrorQuery) -> AdminStoreResult<OpsErrorPage>;
}

/// Runtime settings 与管理员 API Key 写入。
#[async_trait]
pub trait SettingsStore: Send + Sync {
    async fn load_runtime_settings(&self) -> AdminStoreResult<RuntimeSettings>;

    async fn admin_api_key_exists(&self) -> AdminStoreResult<bool>;

    async fn replace_runtime_settings(
        &self,
        command: ReplaceRuntimeSettings,
        context: &MutationContext,
    ) -> AdminStoreResult<RuntimeSettings>;

    async fn replace_admin_api_key(
        &self,
        key: AdminApiKey,
        context: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation>;

    async fn delete_admin_api_key(
        &self,
        context: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation>;

    async fn load_user_agent_override(
        &self,
        _provider_kind: &ProviderKind,
    ) -> AdminStoreResult<ProviderUserAgentOverride> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "outbound user-agent",
            "user-agent settings are not supported by this store",
        ))
    }

    async fn replace_user_agent_override(
        &self,
        _provider_kind: &ProviderKind,
        _selection: ProviderUserAgentOverride,
        _context: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "outbound user-agent",
            "user-agent settings are not supported by this store",
        ))
    }
}

/// 账号目录、运行态与分组所需的 Store 能力集合。
#[derive(Clone)]
pub struct AdminAccountStorePorts {
    accounts: Arc<dyn AccountStore>,
    runtime: Arc<dyn AccountRuntimeStore>,
    groups: Arc<dyn AccountGroupStore>,
    proxies: Arc<dyn super::proxy::ProxyStore>,
}

impl AdminAccountStorePorts {
    #[must_use]
    pub fn new(
        accounts: Arc<dyn AccountStore>,
        runtime: Arc<dyn AccountRuntimeStore>,
        groups: Arc<dyn AccountGroupStore>,
        proxies: Arc<dyn super::proxy::ProxyStore>,
    ) -> Self {
        Self {
            accounts,
            runtime,
            groups,
            proxies,
        }
    }
}

/// 管理用例所需能力的封闭集合。
///
/// 字段保持私有，每个 getter 只交出一种明确能力。该类型不提供通用拆包入口。
#[derive(Clone)]
pub struct AdminStorePorts {
    accounts: AdminAccountStorePorts,
    auth: Arc<dyn AuthStore>,
    client_keys: Arc<dyn ClientKeyStore>,
    observability: Arc<dyn ObservabilityStore>,
    settings: Arc<dyn SettingsStore>,
    backup: BackupStorePorts,
    egress: Option<Arc<dyn ProviderEgressStore>>,
    relogin: Option<Arc<dyn super::relogin::ReloginStore>>,
}

impl AdminStorePorts {
    #[must_use]
    pub fn new(
        accounts: AdminAccountStorePorts,
        auth: Arc<dyn AuthStore>,
        client_keys: Arc<dyn ClientKeyStore>,
        observability: Arc<dyn ObservabilityStore>,
        settings: Arc<dyn SettingsStore>,
        backup: BackupStorePorts,
    ) -> Self {
        Self {
            accounts,
            auth,
            client_keys,
            observability,
            settings,
            backup,
            egress: None,
            relogin: None,
        }
    }

    #[must_use]
    pub fn accounts(&self) -> Arc<dyn AccountStore> {
        self.accounts.accounts.clone()
    }

    #[must_use]
    pub fn account_runtime(&self) -> Arc<dyn AccountRuntimeStore> {
        self.accounts.runtime.clone()
    }

    #[must_use]
    pub fn account_groups(&self) -> Arc<dyn AccountGroupStore> {
        self.accounts.groups.clone()
    }

    #[must_use]
    pub fn proxies(&self) -> Arc<dyn super::proxy::ProxyStore> {
        self.accounts.proxies.clone()
    }

    #[must_use]
    pub fn auth(&self) -> Arc<dyn AuthStore> {
        self.auth.clone()
    }

    #[must_use]
    pub fn client_keys(&self) -> Arc<dyn ClientKeyStore> {
        self.client_keys.clone()
    }

    #[must_use]
    pub fn observability(&self) -> Arc<dyn ObservabilityStore> {
        self.observability.clone()
    }

    #[must_use]
    pub fn settings(&self) -> Arc<dyn SettingsStore> {
        self.settings.clone()
    }

    #[must_use]
    pub fn backup(&self) -> BackupStorePorts {
        self.backup.clone()
    }

    #[must_use]
    pub fn with_egress(mut self, egress: Arc<dyn ProviderEgressStore>) -> Self {
        self.egress = Some(egress);
        self
    }

    #[must_use]
    pub fn egress(&self) -> Option<Arc<dyn ProviderEgressStore>> {
        self.egress.clone()
    }

    #[must_use]
    pub fn with_relogin(mut self, store: Arc<dyn super::relogin::ReloginStore>) -> Self {
        self.relogin = Some(store);
        self
    }

    #[must_use]
    pub fn relogin(&self) -> Option<Arc<dyn super::relogin::ReloginStore>> {
        self.relogin.clone()
    }
}
