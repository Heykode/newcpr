//! Provider 管理能力与动态注册表。

use std::{collections::BTreeMap, net::Ipv6Addr, sync::Arc};

use async_trait::async_trait;
use gateway_core::{
    account::ProviderAccountId,
    operation::Operation,
    routing::{ProviderKind, UpstreamModelId},
};
use heck::ToUpperCamelCase;

use crate::model::accounts::{AccountPlanFilter, ConnectionTestEndpoint, TurnStateProbeOutcome};
use crate::model::observability::{
    CalculatedBillingBreakdown, DashboardWireProfile, ProviderBillingInput,
};
use crate::model::provider_credentials::{
    AuthorizationStarted, CompleteAuthorization, ConsumeProviderResetCredit,
    PendingAuthorizationMutation, PrepareCredentialImport, PrepareCredentialRefresh,
    PrepareCredentialRotation, PreparedAuthorizationCommit, PreparedCredentialImport,
    PreparedCredentialRotation, ProviderExport, ProviderExportCredentialInput, ProviderModels,
    ProviderProfileAvatar, ProviderProfileStatistics, ProviderQuota, ProviderQuotaRequest,
    ProviderResetCreditResult, ProviderResetCredits, ProviderSubscription, explicit_plan_type,
};
use crate::model::user_agent::{OutboundUserAgentView, ProviderUserAgentOverride};

use crate::model::{
    provider_credentials::{ProviderDocument, ProviderQuotaWindow},
    quota_forecast_sampling::QuotaForecastObservation,
};

/// 按原生 Token 口径计算的报价输入，额外费用由调用方处理
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderUsagePricingInput {
    pub upstream_model_id: String,
    pub service_tier: Option<String>,
    pub usage: gateway_core::metering::Usage,
}

/// 账号目录中的模型与独立额度观测，缺少观测不能解释为可用
#[derive(Debug, Clone)]
pub struct ProviderPeerAccountModels {
    pub credential_revision: gateway_core::account::CredentialRevision,
    pub models: BTreeMap<String, Option<ProviderPeerModelQuota>>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderPeerModelQuota {
    pub state: gateway_core::account::QuotaState,
    pub valid_until: std::time::SystemTime,
}

/// Provider 管理失败的稳定分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAdminErrorKind {
    Invalid,
    Unsupported,
    NotFound,
    Conflict,
    /// Provider 已发起不可逆操作，但无法确认最终执行结果。
    Ambiguous,
    Unavailable,
    CredentialRefreshRequired,
    BadGateway,
    Internal,
}

/// 不公开 OAuth 请求材料的管理错误。
///
/// `message` 是 Provider 局部诊断，可能包含原始上游正文；通用管理用例不得自动把它作为公开文案。
/// 只有明确拥有原始诊断合同的调用方才能读取，`Debug` 始终只记录是否存在。
/// `public_message` 则是明确标记为可公开的静态提示，不允许携带动态上游材料。
#[derive(Clone, thiserror::Error)]
#[error("provider admin operation failed: {kind:?}")]
pub struct ProviderAdminError {
    source: Option<gateway_core::error::ErrorSource>,
    kind: ProviderAdminErrorKind,
    message: Option<String>,
    public_message: Option<&'static str>,
    relogin_stop_reason: Option<crate::model::relogin::ReloginStopReason>,
    relogin_workspace_choices: Vec<crate::model::relogin::ReloginWorkspaceChoice>,
    relogin_web_session: Option<Box<crate::model::relogin_maintenance::WebSession>>,
}

impl PartialEq for ProviderAdminError {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.message == other.message
            && self.public_message == other.public_message
            && self.relogin_stop_reason == other.relogin_stop_reason
            && self.relogin_workspace_choices == other.relogin_workspace_choices
            && self.relogin_web_session == other.relogin_web_session
    }
}

impl Eq for ProviderAdminError {}

impl std::fmt::Debug for ProviderAdminError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderAdminError")
            .field("kind", &self.kind)
            .field("message", &self.message.as_ref().map(|_| "<redacted>"))
            .field("public_message", &self.public_message)
            .field("relogin_stop_reason", &self.relogin_stop_reason)
            .field(
                "relogin_workspace_choices",
                &self.relogin_workspace_choices.len(),
            )
            .finish()
    }
}

impl ProviderAdminError {
    #[must_use]
    pub fn with_source(mut self, source: impl Into<gateway_core::error::ErrorSource>) -> Self {
        self.source = Some(source.into());
        self
    }

    #[must_use]
    pub const fn new(kind: ProviderAdminErrorKind) -> Self {
        Self {
            source: None,
            kind,
            message: None,
            public_message: None,
            relogin_stop_reason: None,
            relogin_workspace_choices: Vec::new(),
            relogin_web_session: None,
        }
    }

    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Provider 明确允许公开的静态提示；不得承载上游响应或凭据材料。
    #[must_use]
    pub const fn with_public_message(mut self, message: &'static str) -> Self {
        self.public_message = Some(message);
        self
    }

    #[must_use]
    pub const fn public_message(&self) -> Option<&'static str> {
        self.public_message
    }

    #[must_use]
    pub const fn with_relogin_stop_reason(
        mut self,
        reason: crate::model::relogin::ReloginStopReason,
    ) -> Self {
        self.relogin_stop_reason = Some(reason);
        self
    }

    #[must_use]
    pub const fn relogin_stop_reason(&self) -> Option<crate::model::relogin::ReloginStopReason> {
        self.relogin_stop_reason
    }

    #[must_use]
    pub fn with_relogin_workspace_choices(
        mut self,
        choices: Vec<crate::model::relogin::ReloginWorkspaceChoice>,
    ) -> Self {
        self.relogin_workspace_choices = choices;
        self
    }

    pub fn relogin_workspace_choices(&self) -> &[crate::model::relogin::ReloginWorkspaceChoice] {
        &self.relogin_workspace_choices
    }

    #[must_use]
    pub fn with_relogin_web_session(
        mut self,
        session: Option<crate::model::relogin_maintenance::WebSession>,
    ) -> Self {
        self.relogin_web_session = session.map(Box::new);
        self
    }

    pub fn relogin_web_session(&self) -> Option<&crate::model::relogin_maintenance::WebSession> {
        self.relogin_web_session.as_deref()
    }

    #[must_use]
    pub const fn kind(&self) -> ProviderAdminErrorKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

/// 一个具体 Provider 对管理控制面提供的解析、验证、上游交互与运行时资源回收能力。
///
/// 数据变更由 Provider 返回 prepared facts；config revision、审计与 PostgreSQL 事务
/// 全部由 [`crate::ports::store::AccountStore`] 提交。运行时资源通知只在事务成功后发生。
#[async_trait]
pub trait ProviderAdmin: Send + Sync {
    /// 独立本地统计不借用模型分组额度的重置时间
    fn account_usage_lookback(&self) -> crate::model::provider_credentials::AccountUsageLookback {
        Default::default()
    }

    fn compile_privacy_policy(
        &self,
        _policy: &gateway_core::settings::privacy::CodexPrivacyPolicy,
    ) -> Result<
        Arc<dyn gateway_core::settings::privacy::CompiledPrivacyPolicy>,
        gateway_core::settings::privacy::PrivacyError,
    > {
        Err(gateway_core::settings::privacy::PrivacyError {
            rule_index: 0,
            reason: "该服务商不支持隐私策略",
        })
    }

    fn preview_privacy_policy(
        &self,
        _request: gateway_core::settings::privacy::PrivacyPreviewRequest,
    ) -> Result<
        gateway_core::settings::privacy::PrivacyPreviewResult,
        gateway_core::settings::privacy::PrivacyError,
    > {
        Err(gateway_core::settings::privacy::PrivacyError {
            rule_index: 0,
            reason: "该服务商不支持隐私策略",
        })
    }

    /// 管理端允许为当前 Provider 覆盖的计费档位
    fn supported_pricing_bands(&self) -> &'static [&'static str] {
        &["standard"]
    }

    /// 只读内置价目，没有本地计价能力时返回空目录
    fn pricing_catalog(&self) -> crate::model::pricing::ProviderPricingCatalog {
        Default::default()
    }

    /// 外部参考价到本 Provider 上游模型的对应关系，与内置价共用登记
    fn pricing_source(&self) -> crate::ports::pricing::PricingCatalogSource {
        crate::ports::pricing::PricingCatalogSource::Direct
    }

    fn evaluate_usage_pricing(
        &self,
        _input: &ProviderUsagePricingInput,
        _pricing: Option<&gateway_core::metering::ModelPriceOverride>,
    ) -> Result<Option<gateway_core::metering::CalculatedCostBreakdown>, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// 提供该 Provider 的可选客户端身份；通用管理层不解释内部字段
    fn client_profile_options(
        &self,
    ) -> Result<gateway_core::account::OpaqueProviderData, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// 没有持久选择时使用的 Provider 默认画像；只返回已准备的本地事实
    fn default_client_profile(&self) -> Option<gateway_core::account::OpaqueProviderData> {
        None
    }

    /// 校验并投影客户端身份，结果不含认证或账号材料
    fn preview_client_profile(
        &self,
        _configuration: &gateway_core::account::OpaqueProviderData,
    ) -> Result<gateway_core::account::OpaqueProviderData, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// Key 可以继承当前全局画像；额外字段的合法性由对应 Provider 判定
    fn preview_key_client_profile(
        &self,
        key: &gateway_core::account::OpaqueProviderData,
        _global: Option<&gateway_core::account::OpaqueProviderData>,
    ) -> Result<gateway_core::account::OpaqueProviderData, ProviderAdminError> {
        self.preview_client_profile(key)
    }

    fn merge_batch_key_profile(
        &self,
        current: Option<&gateway_core::account::OpaqueProviderData>,
        update: Option<&Option<gateway_core::account::OpaqueProviderData>>,
        installation_seed: Option<uuid::Uuid>,
    ) -> Result<Option<gateway_core::account::OpaqueProviderData>, ProviderAdminError> {
        if installation_seed.is_some() {
            return Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported));
        }
        Ok(update.map_or_else(|| current.cloned(), Clone::clone))
    }

    fn validate_key_client_profile(
        &self,
        _profile: &gateway_core::account::OpaqueProviderData,
    ) -> Result<(), ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    fn provider_kind(&self) -> &ProviderKind;
    /// 与执行 Provider 共用的直连资格，未知实现默认不公开账号
    fn is_direct_account(&self, _authentication_kind: &str) -> bool {
        false
    }

    /// 仅依据本地账号事实判断通用模型资格；未知实现不能宣称完整覆盖
    fn peer_model_eligible(
        &self,
        _account: &crate::model::accounts::AccountRecord,
        _model: &str,
    ) -> Option<bool> {
        None
    }

    /// 批量读取已授权账号的原生模型事实，不访问上游；None 沿用整号额度合同
    async fn peer_model_facts(
        &self,
        _accounts: &[crate::model::accounts::AccountRecord],
    ) -> Result<Option<BTreeMap<String, ProviderPeerAccountModels>>, ProviderAdminError> {
        Ok(None)
    }

    fn account_capabilities(
        &self,
        _account_id: &ProviderAccountId,
        _authentication_kind: &str,
    ) -> crate::model::accounts::ProviderAccountCapabilities {
        Default::default()
    }

    fn relogin_egress_state(
        &self,
    ) -> Result<
        Option<std::sync::Arc<gateway_core::provider_ports::egress::ProviderEgressConfig>>,
        ProviderAdminError,
    > {
        Ok(None)
    }

    fn select_relogin_source(
        &self,
        _account: &crate::model::accounts::AccountRecord,
    ) -> Result<Option<std::net::Ipv6Addr>, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    async fn maintain_relogin(
        &self,
        _request: crate::model::relogin_maintenance::MaintenanceRequest,
    ) -> Result<crate::model::relogin_maintenance::MaintenanceOutcome, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unavailable))
    }

    async fn relogin(
        &self,
        _request: crate::model::relogin::ReloginRequest,
    ) -> Result<crate::model::relogin::ReloginCredential, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// Current default/effective outbound UA; syntax remains Provider-owned.
    fn outbound_user_agent(&self) -> Result<OutboundUserAgentView, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// Validate a selection and project its result without publishing it.
    fn preview_outbound_user_agent(
        &self,
        _selection: &ProviderUserAgentOverride,
    ) -> Result<OutboundUserAgentView, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// Apply an explicit selection to subsequent independent operations.
    fn apply_outbound_user_agent(
        &self,
        _selection: ProviderUserAgentOverride,
    ) -> Result<OutboundUserAgentView, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// 提供最终套餐展示名称；未覆盖时采用通用的大驼峰格式。
    fn plan_type_display(&self, plan_type: &str) -> String {
        default_plan_type_display(plan_type)
    }

    /// 返回与套餐列同名的全部原始类型；未提供筛选目录时不接受套餐组查询。
    fn plan_type_aliases(&self, _plan_type: &str) -> Option<Vec<String>> {
        None
    }

    /// 账号已经由控制面提交为不可调度状态，释放 Provider 持有的账号级运行时资源。
    ///
    /// 无账号级运行时资源的 Provider 不需要执行额外操作。该通知发生在 Store 事务
    /// 成功之后，不参与事务成败，也不得恢复或改写已经提交的账号状态。
    async fn account_unavailable(&self, account_id: &ProviderAccountId);

    /// 账号资格事实已经由控制面提交，失效 Provider 持有的可重建派生状态。
    ///
    /// 该通知发生在 Store 事务成功之后、下一份 RuntimeSnapshot 编译之前；通知
    /// 不参与已提交事务成败。没有账号派生状态的 Provider 可使用默认空实现。
    async fn account_facts_changed(&self, _account_ids: &[ProviderAccountId]) {}

    async fn request_turn_state_probe(
        &self,
        _account_id: &ProviderAccountId,
        _model: &UpstreamModelId,
    ) -> Result<TurnStateProbeOutcome, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// Egress policy changed after a successful control-plane transaction.
    async fn egress_configuration_changed(&self) -> Result<(), ProviderAdminError> {
        Ok(())
    }

    /// Validate local source binding without attempting an Internet probe.
    fn validate_egress_sources(&self, _addresses: &[Ipv6Addr]) -> Result<(), ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// 生成一次连接测试所需的 Provider-owned operation；Core 负责实际执行与落账。
    fn connection_test_operation(
        &self,
        upstream_model: &UpstreamModelId,
        input_text: &str,
    ) -> Result<Operation, ProviderAdminError>;

    /// 扩展诊断选项；未实现选项的 Provider 保持旧诊断行为，不能静默忽略请求参数。
    fn connection_test_operation_with_options(
        &self,
        upstream_model: &UpstreamModelId,
        input_text: &str,
        endpoint: ConnectionTestEndpoint,
        stream: bool,
    ) -> Result<Operation, ProviderAdminError> {
        if endpoint != ConnectionTestEndpoint::Responses || !stream {
            return Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported)
                .with_public_message("该 Provider 不支持所选连接测试选项"));
        }
        self.connection_test_operation(upstream_model, input_text)
    }

    /// 生成 Excel/BPS 恢复探针；Provider 可为该内部探针固定较低推理档位。
    fn excel_recovery_operation(
        &self,
        upstream_model: &UpstreamModelId,
        input_text: &str,
    ) -> Result<Operation, ProviderAdminError> {
        self.connection_test_operation_with_options(
            upstream_model,
            input_text,
            ConnectionTestEndpoint::Responses,
            true,
        )
    }

    /// 返回该 Provider 实际持有的 Dashboard 上游身份画像。
    fn dashboard_wire_profile(&self) -> Option<DashboardWireProfile>;

    fn configured_wire_profile(
        &self,
        _configuration: &gateway_core::account::OpaqueProviderData,
    ) -> Option<DashboardWireProfile> {
        self.dashboard_wire_profile()
    }

    /// 使用 Provider-owned 价格规则恢复持久请求的逐项费用。
    fn calculated_billing(
        &self,
        input: &ProviderBillingInput,
    ) -> Result<Option<CalculatedBillingBreakdown>, ProviderAdminError>;

    async fn prepare_import(
        &self,
        command: PrepareCredentialImport,
    ) -> Result<PreparedCredentialImport, ProviderAdminError>;

    async fn start_authorization(
        &self,
        pending: PendingAuthorizationMutation,
    ) -> Result<AuthorizationStarted, ProviderAdminError>;

    async fn complete_authorization(
        &self,
        command: CompleteAuthorization,
    ) -> Result<PreparedAuthorizationCommit, ProviderAdminError>;

    async fn prepare_rotation(
        &self,
        command: PrepareCredentialRotation,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError>;

    /// Explicit manual relogin only; ordinary refresh/rotation remains workspace-locked.
    async fn prepare_relogin_workspace_switch(
        &self,
        _command: PrepareCredentialRotation,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Invalid)
            .with_public_message("不支持手动切换工作区"))
    }

    async fn prepare_refresh(
        &self,
        command: PrepareCredentialRefresh,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError>;

    async fn quota(
        &self,
        request: ProviderQuotaRequest,
    ) -> Result<ProviderQuota, ProviderAdminError>;

    async fn read_quotas(
        &self,
        requests: &[crate::model::provider_credentials::ProviderQuotaReadRequest<'_>],
    ) -> Vec<Result<ProviderQuota, ProviderAdminError>> {
        futures::future::join_all(requests.iter().map(|request| async {
            let account_id = ProviderAccountId::new(request.account.id.clone())
                .map_err(|_| ProviderAdminError::new(ProviderAdminErrorKind::Invalid))?;
            self.quota(ProviderQuotaRequest {
                account_id,
                refresh: false,
                rolling_usage: request.rolling_usage.cloned(),
            })
            .await
        }))
        .await
    }

    /// 历史观测的协议字段仅由具体 Provider 解释；不支持时保留累计估算。
    fn quota_forecast_observation(
        &self,
        _document: &ProviderDocument,
        _window: &ProviderQuotaWindow,
    ) -> Option<QuotaForecastObservation> {
        None
    }

    /// 查询 Provider 官方个人资料统计；不支持该能力的 Provider 使用默认拒绝。
    /// 只由个人信息显式查询，不在额度刷新或后台任务中预取。
    async fn subscription(
        &self,
        _account_id: &ProviderAccountId,
    ) -> Result<Option<ProviderSubscription>, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    async fn profile_statistics(
        &self,
        _account_id: &ProviderAccountId,
    ) -> Result<ProviderProfileStatistics, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// 打开 Provider 官方头像字节流；不支持该能力的 Provider 使用默认拒绝。
    async fn profile_avatar(
        &self,
        _account_id: &ProviderAccountId,
    ) -> Result<ProviderProfileAvatar, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// 查询 Provider 主动额度重置卡；不支持该能力的 Provider 使用默认拒绝。
    async fn reset_credits(
        &self,
        _account_id: &ProviderAccountId,
    ) -> Result<ProviderResetCredits, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    /// 消费 Provider 主动额度重置卡；不支持该能力的 Provider 使用默认拒绝。
    async fn consume_reset_credit(
        &self,
        _command: ConsumeProviderResetCredit,
    ) -> Result<ProviderResetCreditResult, ProviderAdminError> {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    async fn models(
        &self,
        account_id: &ProviderAccountId,
        refresh: bool,
    ) -> Result<ProviderModels, ProviderAdminError>;

    /// Quality editor suggestions. Aggregate calls must never initiate network requests.
    async fn quality_model_choices(
        &self,
        _account_id: &ProviderAccountId,
        _exact: bool,
    ) -> Result<Vec<crate::model::quality_ops::QualityModelChoice>, ProviderAdminError> {
        Ok(Vec::new())
    }

    async fn model_catalog_document(
        &self,
        _account_id: &ProviderAccountId,
    ) -> Result<crate::model::provider_credentials::ProviderModelCatalogDocument, ProviderAdminError>
    {
        Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    async fn export_credentials(
        &self,
        credentials: Vec<ProviderExportCredentialInput>,
    ) -> Result<ProviderExport, ProviderAdminError>;
}

/// 按 ProviderKind 动态发现管理能力；不含具体 Provider 分支。
#[derive(Clone)]
pub struct ProviderAdminRegistry {
    providers: Arc<BTreeMap<ProviderKind, Arc<dyn ProviderAdmin>>>,
}

impl ProviderAdminRegistry {
    pub(crate) fn pricing_source_scope(&self) -> crate::ports::pricing::PricingSourceScope {
        crate::ports::pricing::PricingSourceScope {
            catalogs: self
                .providers
                .iter()
                .map(|(kind, provider)| (kind.to_string(), provider.pricing_source()))
                .collect(),
        }
    }

    pub(crate) fn supported_pricing_bands(&self) -> BTreeMap<String, Vec<String>> {
        self.providers
            .iter()
            .map(|(kind, provider)| {
                let bands = provider
                    .supported_pricing_bands()
                    .iter()
                    .map(|&band| band.to_owned())
                    .collect();
                (kind.to_string(), bands)
            })
            .collect()
    }

    pub fn pricing_catalog(&self) -> gateway_core::metering::PricingOverrides {
        self.providers
            .iter()
            .map(|(kind, provider)| (kind.as_str().to_owned(), provider.pricing_catalog()))
            .collect()
    }

    /// 创建无重复 ProviderKind 的注册表。
    ///
    /// # Errors
    ///
    /// 重复注册同一 ProviderKind 时返回 Conflict。
    pub fn new(
        providers: impl IntoIterator<Item = Arc<dyn ProviderAdmin>>,
    ) -> Result<Self, ProviderAdminError> {
        let mut registered = BTreeMap::new();
        for provider in providers {
            let kind = provider.provider_kind().clone();
            if registered.insert(kind, provider).is_some() {
                return Err(ProviderAdminError::new(ProviderAdminErrorKind::Conflict));
            }
        }
        Ok(Self {
            providers: Arc::new(registered),
        })
    }

    pub fn require(
        &self,
        provider_kind: &ProviderKind,
    ) -> Result<Arc<dyn ProviderAdmin>, ProviderAdminError> {
        self.providers
            .get(provider_kind)
            .cloned()
            .ok_or_else(|| ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    pub fn validate_egress_sources(
        &self,
        provider_kind: &ProviderKind,
        addresses: &[Ipv6Addr],
    ) -> Result<(), ProviderAdminError> {
        self.require(provider_kind)?
            .validate_egress_sources(addresses)
    }

    pub async fn egress_configuration_changed(
        &self,
        provider_kind: &ProviderKind,
    ) -> Result<(), ProviderAdminError> {
        self.require(provider_kind)?
            .egress_configuration_changed()
            .await
    }

    /// 账号目录与关联列表共用套餐补全和展示规则，已知账号套餐优先于额度快照。
    pub(crate) fn resolve_account_plan(
        &self,
        provider_kind: &str,
        plan_type: &mut Option<String>,
        quota: Option<&ProviderQuota>,
    ) -> Option<String> {
        if let Some(quota) = quota {
            quota.fill_missing_plan_type(plan_type);
        }
        self.plan_type_display(provider_kind, plan_type.as_deref())
    }

    /// 显式的 provider:plan 查询复用 Provider 名称映射；旧原始值保持精确查询。
    pub(crate) fn resolve_account_plan_filter(
        &self,
        filter: AccountPlanFilter,
    ) -> Result<AccountPlanFilter, ProviderAdminError> {
        let AccountPlanFilter::Raw(raw) = &filter else {
            return Ok(filter);
        };
        let Some((provider, plan)) = raw.split_once(':') else {
            return Ok(filter);
        };
        let Ok(kind) = ProviderKind::new(provider.to_owned()) else {
            return Ok(filter);
        };
        let Some(provider) = self.providers.get(&kind) else {
            return Ok(filter);
        };
        let plan_types = provider
            .plan_type_aliases(plan)
            .filter(|plans| !plans.is_empty())
            .ok_or_else(|| ProviderAdminError::new(ProviderAdminErrorKind::Invalid))?;
        Ok(AccountPlanFilter::ProviderPlans {
            provider_kind: kind,
            plan_types,
        })
    }

    /// 账号页和 Dashboard 共用 Provider 的最终名称，不改写官方拼写或原始套餐值。
    pub(crate) fn plan_type_display(
        &self,
        provider_kind: &str,
        plan_type: Option<&str>,
    ) -> Option<String> {
        let plan_type = explicit_plan_type(plan_type)?.trim();
        let provider = ProviderKind::new(provider_kind.to_owned())
            .ok()
            .and_then(|kind| self.providers.get(&kind));
        Some(provider.map_or_else(
            || default_plan_type_display(plan_type),
            |provider| provider.plan_type_display(plan_type),
        ))
    }

    /// 返回所有已注册 Provider 的 Dashboard 上游身份画像。
    pub fn dashboard_wire_profiles(&self) -> Vec<DashboardWireProfile> {
        self.providers
            .values()
            .filter_map(|provider| provider.dashboard_wire_profile())
            .collect()
    }

    /// 动态分派 Provider-owned 费用规则，不含任何具体 Provider 分支。
    pub fn calculated_billing(
        &self,
        provider_kind: &ProviderKind,
        input: &ProviderBillingInput,
    ) -> Result<Option<CalculatedBillingBreakdown>, ProviderAdminError> {
        self.require(provider_kind)?.calculated_billing(input)
    }
}

fn default_plan_type_display(plan_type: &str) -> String {
    plan_type.replace('+', " Plus ").to_upper_camel_case()
}

pub struct ProviderBundle {
    pub core: Arc<dyn gateway_core::engine::provider::Provider>,
    pub admin: Arc<dyn ProviderAdmin>,
    pub workers: Vec<gateway_core::task::WorkerContribution>,
}
