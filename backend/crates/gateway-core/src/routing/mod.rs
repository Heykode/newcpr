//! Provider、模型目录、精确模型映射与请求级候选计划。

mod catalog;
pub mod snapshot;

pub use crate::account::scope::{
    AccountGroupId, AccountRoutingScopeKind, AccountRoutingSnapshot, ClientRoutingScope,
    FrozenAccountScope, RoutingGroupSnapshot, RuntimeAccount, RuntimeAccountDirectory,
};
pub use crate::identity::ProviderKind;
pub use catalog::{
    ProviderCatalogGeneration, ProviderCatalogPort, ProviderCatalogUnavailable,
    ProviderModelCapabilities, ProviderModelDescriptor, PublicModelDescriptor,
};
pub use snapshot::RuntimeSnapshot;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;

use crate::account::AccountSelectionPolicy;
use crate::operation::{CapabilityRequirements, Feature, OperationKind};
use crate::validation::{IdentifierError, RoutingError, validate_text};

pub const DEFAULT_MAX_REQUEST_ATTEMPTS: u32 = 32;
pub const DEFAULT_RESPONSES_MAX_DECOMPRESSED_BODY_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RESPONSES_MAX_DECOMPRESSED_BODY_BYTES: u64 = 256 * 1024 * 1024;

/// OpenAI managed turn-state injection policy published with the runtime snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiTurnStatePolicy {
    enabled: bool,
    models: Arc<BTreeSet<UpstreamModelId>>,
    probe_proxy: Option<crate::account::OutboundProxy>,
}

impl Default for OpenAiTurnStatePolicy {
    fn default() -> Self {
        let models = ["gpt-6-astra", "gpt-5.6-sol", "gpt-5.6-terra"]
            .into_iter()
            .filter_map(|model| UpstreamModelId::new(model.to_owned()).ok())
            .collect();
        Self {
            enabled: false,
            models: Arc::new(models),
            probe_proxy: None,
        }
    }
}

impl OpenAiTurnStatePolicy {
    #[must_use]
    pub fn new(enabled: bool, models: BTreeSet<UpstreamModelId>) -> Self {
        Self {
            enabled,
            models: Arc::new(models),
            probe_proxy: None,
        }
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn contains(&self, model: &UpstreamModelId) -> bool {
        self.models.contains(model)
    }

    #[must_use]
    pub fn models(&self) -> &BTreeSet<UpstreamModelId> {
        &self.models
    }

    #[must_use]
    pub fn with_probe_proxy(mut self, proxy: Option<crate::account::OutboundProxy>) -> Self {
        self.probe_proxy = proxy;
        self
    }

    #[must_use]
    pub fn probe_proxy(&self) -> Option<&crate::account::OutboundProxy> {
        self.probe_proxy.as_ref()
    }
}

/// 请求级重试、账号切换和 WebSocket 恢复参数。
///
/// 参数在请求计划创建时冻结，避免管理员保存设置后改变已经运行中的请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestTuning {
    pub max_account_switches: u32,
    pub max_request_attempts: u32,
    pub websocket_max_retries: u32,
    pub websocket_http_fallback_enabled: bool,
    #[serde(default = "default_websocket_large_request_threshold_bytes")]
    pub websocket_large_request_threshold_bytes: u64,
    pub websocket_max_age_ms: u64,
    pub websocket_stream_idle_timeout_ms: u64,
    pub websocket_failure_threshold: u32,
    pub websocket_failure_window_ms: u64,
    pub websocket_failure_open_duration_ms: u64,
    pub rate_limit_cooldown_seconds: u64,
    #[serde(default)]
    pub openai_location_override_enabled: bool,
    #[serde(default)]
    pub max_waiting_per_key: u32,
    #[serde(default = "default_fallback_timeout_seconds")]
    pub key_concurrency_wait_timeout_seconds: u64,
    #[serde(default)]
    pub account_busy_wait_enabled: bool,
    #[serde(default = "default_sticky_max_waiting")]
    pub account_busy_wait_sticky_max_waiting: u32,
    #[serde(default = "default_sticky_timeout_seconds")]
    pub account_busy_wait_sticky_timeout_seconds: u64,
    #[serde(default = "default_fallback_max_waiting")]
    pub account_busy_wait_fallback_max_waiting: u32,
    #[serde(default = "default_fallback_timeout_seconds")]
    pub account_busy_wait_fallback_timeout_seconds: u64,
}

const fn default_websocket_large_request_threshold_bytes() -> u64 {
    15 * 1024 * 1024
}

const fn default_sticky_max_waiting() -> u32 {
    3
}

const fn default_sticky_timeout_seconds() -> u64 {
    120
}

const fn default_fallback_max_waiting() -> u32 {
    100
}

const fn default_fallback_timeout_seconds() -> u64 {
    30
}

impl Default for RequestTuning {
    fn default() -> Self {
        Self::defaults()
    }
}

impl RequestTuning {
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            max_account_switches: DEFAULT_MAX_REQUEST_ATTEMPTS - 1,
            max_request_attempts: DEFAULT_MAX_REQUEST_ATTEMPTS,
            websocket_max_retries: 5,
            websocket_http_fallback_enabled: true,
            websocket_large_request_threshold_bytes:
                default_websocket_large_request_threshold_bytes(),
            websocket_max_age_ms: 55 * 60 * 1_000,
            websocket_stream_idle_timeout_ms: 300_000,
            websocket_failure_threshold: 3,
            websocket_failure_window_ms: 30_000,
            websocket_failure_open_duration_ms: 30_000,
            rate_limit_cooldown_seconds: 60,
            openai_location_override_enabled: false,
            max_waiting_per_key: 0,
            key_concurrency_wait_timeout_seconds: 30,
            account_busy_wait_enabled: false,
            account_busy_wait_sticky_max_waiting: default_sticky_max_waiting(),
            account_busy_wait_sticky_timeout_seconds: default_sticky_timeout_seconds(),
            account_busy_wait_fallback_max_waiting: default_fallback_max_waiting(),
            account_busy_wait_fallback_timeout_seconds: default_fallback_timeout_seconds(),
        }
    }

    #[must_use]
    pub const fn max_attempts(self) -> NonZeroU32 {
        match NonZeroU32::new(self.max_request_attempts) {
            Some(value) => value,
            None => NonZeroU32::MIN,
        }
    }
}

/// 客户端请求中的模型名称。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicModelId(String);

impl PublicModelId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_text(&value, 256, true, None)?;
        Ok(Self(value))
    }

    /// 从客户端 OpenAI wire 读取模型名。
    ///
    /// 该值只作为路由查询键；具体模型字符串能否被上游接受由 Provider 决定，
    /// 因而不能把内部标识长度或控制字符规则当作入站 schema gate。
    ///
    /// # Errors
    ///
    /// 空模型名无法参与路由时返回错误。
    pub fn from_client_wire(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if value.is_empty() {
            return Err(IdentifierError::Empty);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PublicModelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Provider 实际接收的模型名称。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UpstreamModelId(String);

impl UpstreamModelId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_text(&value, 256, true, None)?;
        Ok(Self(value))
    }

    /// 从未命中显式映射的客户端模型名构造同名上游模型。
    ///
    /// 目录和配置仍使用 [`Self::new`] 的内部标识约束；客户端 wire 只要求非空，
    /// 具体模型字符串是否可用由绑定 Provider 决定。
    pub fn from_client_wire(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if value.is_empty() {
            return Err(IdentifierError::Empty);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UpstreamModelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// `runtime_settings.config_revision`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigRevision(NonZeroU64);

impl ConfigRevision {
    pub fn new(value: u64) -> Result<Self, RoutingError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(RoutingError::InvalidRevision)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Provider 实时目录报告的能力支持等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportLevel {
    Native,
    Emulated,
    Unsupported,
    Unknown,
}

/// Provider 实时模型目录中的能力事实；不落 PostgreSQL。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCapabilities {
    operations: BTreeSet<OperationKind>,
    features: BTreeMap<Feature, SupportLevel>,
    max_output_tokens: Option<u64>,
    upstream_validates_features: bool,
}

/// Provider 为客户端模型目录提供的展示与交互能力。
///
/// 该值不参与路由，也不把任一 Provider 的 wire 类型带入 Core。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelPresentation {
    display_name: Option<String>,
    description: Option<String>,
    default_reasoning_effort: Option<String>,
    supported_reasoning_efforts: Vec<String>,
    context_window_tokens: Option<u64>,
    max_context_window_tokens: Option<u64>,
    image_input: bool,
    agent_tools: bool,
    parallel_tool_calls: bool,
    search_tool: bool,
    image_detail_original: bool,
    verbosity: bool,
    service_tiers: Vec<ModelServiceTier>,
    hidden: bool,
}

/// Provider 声明给 Codex 客户端的服务档位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelServiceTier {
    id: String,
    name: String,
    description: String,
    speed_tier: Option<String>,
}

impl ModelServiceTier {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            speed_tier: None,
        }
    }

    #[must_use]
    pub fn with_speed_tier(mut self, speed_tier: impl Into<String>) -> Self {
        self.speed_tier = Some(speed_tier.into());
        self
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[must_use]
    pub fn speed_tier(&self) -> Option<&str> {
        self.speed_tier.as_deref()
    }
}

impl ModelPresentation {
    #[must_use]
    pub fn new(display_name: Option<String>, description: Option<String>) -> Self {
        Self {
            display_name,
            description,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_reasoning(
        mut self,
        default_effort: Option<String>,
        supported_efforts: Vec<String>,
    ) -> Self {
        self.default_reasoning_effort = default_effort;
        self.supported_reasoning_efforts = supported_efforts;
        self
    }

    #[must_use]
    pub const fn with_context_window_tokens(mut self, context_window_tokens: Option<u64>) -> Self {
        self.context_window_tokens = context_window_tokens;
        self
    }

    /// Provider 目录声明的最大上下文窗口；缺失时保持未知。
    #[must_use]
    pub const fn with_max_context_window_tokens(
        mut self,
        max_context_window_tokens: Option<u64>,
    ) -> Self {
        self.max_context_window_tokens = max_context_window_tokens;
        self
    }

    #[must_use]
    pub const fn with_image_input(mut self, image_input: bool) -> Self {
        self.image_input = image_input;
        self
    }

    #[must_use]
    pub const fn with_agent_tools(mut self, agent_tools: bool, parallel_tool_calls: bool) -> Self {
        self.agent_tools = agent_tools;
        self.parallel_tool_calls = parallel_tool_calls;
        self
    }

    #[must_use]
    pub const fn with_search_tool(mut self, search_tool: bool) -> Self {
        self.search_tool = search_tool;
        self
    }

    #[must_use]
    pub const fn with_image_detail_original(mut self, image_detail_original: bool) -> Self {
        self.image_detail_original = image_detail_original;
        self
    }

    #[must_use]
    pub const fn with_verbosity(mut self, verbosity: bool) -> Self {
        self.verbosity = verbosity;
        self
    }

    #[must_use]
    pub fn with_service_tiers(mut self, service_tiers: Vec<ModelServiceTier>) -> Self {
        self.service_tiers = service_tiers;
        self
    }

    #[must_use]
    pub const fn with_hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    #[must_use]
    pub fn default_reasoning_effort(&self) -> Option<&str> {
        self.default_reasoning_effort.as_deref()
    }

    #[must_use]
    pub fn supported_reasoning_efforts(&self) -> &[String] {
        &self.supported_reasoning_efforts
    }

    #[must_use]
    pub const fn context_window_tokens(&self) -> Option<u64> {
        self.context_window_tokens
    }

    #[must_use]
    pub const fn max_context_window_tokens(&self) -> Option<u64> {
        self.max_context_window_tokens
    }

    #[must_use]
    pub const fn image_input(&self) -> bool {
        self.image_input
    }

    #[must_use]
    pub const fn agent_tools(&self) -> bool {
        self.agent_tools
    }

    #[must_use]
    pub const fn parallel_tool_calls(&self) -> bool {
        self.parallel_tool_calls
    }

    #[must_use]
    pub const fn search_tool(&self) -> bool {
        self.search_tool
    }

    #[must_use]
    pub const fn image_detail_original(&self) -> bool {
        self.image_detail_original
    }

    #[must_use]
    pub const fn verbosity(&self) -> bool {
        self.verbosity
    }

    #[must_use]
    pub fn service_tiers(&self) -> &[ModelServiceTier] {
        &self.service_tiers
    }

    #[must_use]
    pub const fn hidden(&self) -> bool {
        self.hidden
    }
}

/// 一个公开模型及其 Provider 编译后的客户端画像。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicModelProfile {
    model: PublicModelId,
    presentation: ModelPresentation,
}

impl PublicModelProfile {
    #[must_use]
    pub const fn new(model: PublicModelId, presentation: ModelPresentation) -> Self {
        Self {
            model,
            presentation,
        }
    }

    #[must_use]
    pub const fn model(&self) -> &PublicModelId {
        &self.model
    }

    #[must_use]
    pub const fn presentation(&self) -> &ModelPresentation {
        &self.presentation
    }
}

impl ModelCapabilities {
    #[must_use]
    pub fn new(operations: BTreeSet<OperationKind>, max_output_tokens: Option<u64>) -> Self {
        Self {
            operations,
            features: BTreeMap::new(),
            max_output_tokens,
            upstream_validates_features: false,
        }
    }

    #[must_use]
    pub fn with_feature(mut self, feature: Feature, support: SupportLevel) -> Self {
        self.features.insert(feature, support);
        self
    }

    /// 将请求形态 feature 的最终合法性判断交给上游 wire API。
    #[must_use]
    pub const fn with_upstream_feature_validation(mut self) -> Self {
        self.upstream_validates_features = true;
        self
    }

    #[must_use]
    pub fn match_requirements(
        &self,
        requirements: &CapabilityRequirements,
    ) -> Option<BTreeSet<Feature>> {
        if !self.operations.contains(&requirements.operation())
            || requirements
                .requested_output_tokens()
                .is_some_and(|requested| {
                    self.max_output_tokens
                        .is_some_and(|maximum| requested > maximum)
                })
        {
            return None;
        }

        if self.upstream_validates_features {
            return Some(BTreeSet::new());
        }

        let mut emulated = BTreeSet::new();
        for feature in requirements.features() {
            match self
                .features
                .get(feature)
                .copied()
                .unwrap_or(SupportLevel::Unknown)
            {
                SupportLevel::Native => {}
                SupportLevel::Emulated => {
                    emulated.insert(*feature);
                }
                SupportLevel::Unsupported | SupportLevel::Unknown => return None,
            }
        }
        Some(emulated)
    }
}

/// 一个 Provider 实时发现的上游模型能力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModel {
    provider: ProviderKind,
    upstream_model: UpstreamModelId,
    capabilities: ModelCapabilities,
    presentation: Option<ModelPresentation>,
}

impl ProviderModel {
    #[must_use]
    pub const fn new(
        provider: ProviderKind,
        upstream_model: UpstreamModelId,
        capabilities: ModelCapabilities,
    ) -> Self {
        Self {
            provider,
            upstream_model,
            capabilities,
            presentation: None,
        }
    }

    #[must_use]
    pub fn with_presentation(mut self, presentation: ModelPresentation) -> Self {
        self.presentation = Some(presentation);
        self
    }

    #[must_use]
    pub const fn provider(&self) -> &ProviderKind {
        &self.provider
    }

    #[must_use]
    pub const fn upstream_model(&self) -> &UpstreamModelId {
        &self.upstream_model
    }

    #[must_use]
    pub const fn presentation(&self) -> Option<&ModelPresentation> {
        self.presentation.as_ref()
    }
}

/// 本次请求选择 Provider 时使用的动态过滤事实。
#[derive(Debug, Clone, Default)]
pub struct RoutingContext {
    /// 管理端 connection test 显式限制的 Provider；普通请求留空。
    pub required_provider: Option<ProviderKind>,
    pub blocked_providers: BTreeSet<ProviderKind>,
}

/// 已绑定 Provider 的请求候选；模型端点携带真实上游模型，原生端点不虚构模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCandidate {
    provider: ProviderKind,
    upstream_model: Option<UpstreamModelId>,
    emulated_features: BTreeSet<Feature>,
    account_scope: Arc<FrozenAccountScope>,
}

impl ProviderCandidate {
    #[must_use]
    pub const fn provider(&self) -> &ProviderKind {
        &self.provider
    }

    #[must_use]
    pub const fn upstream_model(&self) -> Option<&UpstreamModelId> {
        self.upstream_model.as_ref()
    }

    #[must_use]
    pub const fn emulated_features(&self) -> &BTreeSet<Feature> {
        &self.emulated_features
    }

    #[must_use]
    pub const fn account_scope(&self) -> &Arc<FrozenAccountScope> {
        &self.account_scope
    }
}

/// 一次请求冻结的 Provider 尝试顺序。
#[derive(Debug, Clone)]
pub struct RoutingPlan {
    disable_fast: bool,
    request_location: Option<crate::account::RequestLocation>,
    config_revision: ConfigRevision,
    account_selection_policy: AccountSelectionPolicy,
    operation: OperationKind,
    max_attempts: NonZeroU32,
    request_tuning: RequestTuning,
    account_scope: Arc<FrozenAccountScope>,
    candidates: Arc<[ProviderCandidate]>,
}

impl RoutingPlan {
    #[must_use]
    pub const fn disable_fast(&self) -> bool {
        self.disable_fast
    }

    #[must_use]
    pub fn request_location(&self) -> Option<&crate::account::RequestLocation> {
        self.request_location.as_ref()
    }

    #[must_use]
    pub const fn config_revision(&self) -> ConfigRevision {
        self.config_revision
    }

    #[must_use]
    pub const fn account_selection_policy(&self) -> AccountSelectionPolicy {
        self.account_selection_policy
    }

    #[must_use]
    pub const fn operation(&self) -> OperationKind {
        self.operation
    }

    #[must_use]
    pub const fn max_attempts(&self) -> NonZeroU32 {
        self.max_attempts
    }

    #[must_use]
    pub const fn request_tuning(&self) -> RequestTuning {
        self.request_tuning
    }

    #[must_use]
    pub const fn account_scope(&self) -> &Arc<FrozenAccountScope> {
        &self.account_scope
    }

    #[must_use]
    pub fn candidates(&self) -> &[ProviderCandidate] {
        &self.candidates
    }
}
