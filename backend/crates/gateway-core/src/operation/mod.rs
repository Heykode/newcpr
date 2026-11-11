//! 协议无关的业务 operation。
//!
//! 这里只保留网关需要解释、路由或结算的稳定语义。协议 adapter 无法写入
//! wire body 与连接级事实随 [`ProtocolPayload`] 或 [`RawJsonPayload`] 不透明传递，
//! 由对应 Provider 自己解释。

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use bytes::Bytes;
use gateway_protocol::request::{RequestFacts, request_facts};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::validation::{OperationError, validate_text};

pub mod quality_probe;

/// 网关支持的稳定 operation 分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum OperationKind {
    /// 文本、多模态和工具生成。
    Generate,
    /// 图像生成。
    GenerateImage,
    CountTokens,
    /// Provider 原生 standalone search。
    Search,
    /// 客户端显式请求的上下文压缩，不是自动生成步骤。
    Compact,
    /// Provider 自有的账号认证 HTTP 端点。
    ProviderHttp,
}

impl OperationKind {
    /// 返回注册表和持久化使用的稳定名称。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generate => "generate",
            Self::GenerateImage => "generate_image",
            Self::CountTokens => "count_tokens",
            Self::Search => "search",
            Self::Compact => "compact",
            Self::ProviderHttp => "provider_http",
        }
    }
}

/// Router 理解的稳定能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Feature {
    /// Tool calling。
    Tools,
    /// 图像输入。
    Vision,
    /// 推理控制或推理输出。
    Reasoning,
    /// JSON Schema 输出。
    JsonSchema,
    /// Provider 原生延续。
    NativeContinuation,
}

/// 从 operation 推导出的请求能力约束。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRequirements {
    operation: OperationKind,
    features: BTreeSet<Feature>,
    requested_output_tokens: Option<u64>,
}

impl CapabilityRequirements {
    /// 创建仅要求 operation 的能力约束。
    #[must_use]
    pub fn new(operation: OperationKind) -> Self {
        Self {
            operation,
            features: BTreeSet::new(),
            requested_output_tokens: None,
        }
    }

    /// 增加稳定能力要求。
    #[must_use]
    pub fn require(mut self, feature: Feature) -> Self {
        self.features.insert(feature);
        self
    }

    /// 设置请求的最大输出 token 数。
    #[must_use]
    pub const fn with_requested_output_tokens(mut self, tokens: Option<u64>) -> Self {
        self.requested_output_tokens = tokens;
        self
    }

    /// 返回 operation 分类。
    #[must_use]
    pub const fn operation(&self) -> OperationKind {
        self.operation
    }

    /// 返回全部稳定能力要求。
    #[must_use]
    pub fn features(&self) -> &BTreeSet<Feature> {
        &self.features
    }

    /// 返回请求的最大输出 token 数。
    #[must_use]
    pub const fn requested_output_tokens(&self) -> Option<u64> {
        self.requested_output_tokens
    }
}

/// 同一客户端连接内由 Provider 生成并解释的不透明会话状态。
///
/// Core 只在重试和路由过程中保持该值；协议层只能把 Provider 返回的状态原样带入
/// 下一轮，不能读取或改写正文。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderSessionState {
    provider: Box<str>,
    payload: Map<String, Value>,
    #[serde(skip)]
    reconnect_websocket: bool,
}

impl ProviderSessionState {
    /// 创建 Provider 私有会话状态。
    ///
    /// # Errors
    ///
    /// Provider 名称无效时返回错误。
    pub fn new(
        provider: impl Into<String>,
        payload: Map<String, Value>,
    ) -> Result<Self, OperationError> {
        let provider = provider.into();
        validate_text(&provider, 64, true, None).map_err(|_| OperationError::EmptyField {
            field: "provider_session_state provider",
        })?;
        Ok(Self {
            provider: provider.into_boxed_str(),
            payload,
            reconnect_websocket: false,
        })
    }

    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    #[must_use]
    pub const fn payload(&self) -> &Map<String, Value> {
        &self.payload
    }

    #[must_use]
    pub const fn with_websocket_reconnect(mut self) -> Self {
        self.reconnect_websocket = true;
        self
    }

    #[must_use]
    pub const fn prefers_websocket_reconnect(&self) -> bool {
        self.reconnect_websocket
    }
}

impl fmt::Debug for ProviderSessionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderSessionState")
            .field("provider", &self.provider)
            .field("payload", &"<not included in Debug>")
            .finish()
    }
}

/// 客户端协议交给 Provider 的不透明 JSON object。
///
/// Core 只负责在路由和重试过程中保持该值，不读取或改写正文。协议 adapter
/// 与对应 Provider 共同拥有其语义。
#[derive(Clone, PartialEq)]
pub struct ProtocolPayload {
    protocol: String,
    body: Map<String, Value>,
    context: Map<String, Value>,
}

impl ProtocolPayload {
    /// 创建协议不透明正文。
    ///
    /// # Errors
    ///
    /// 协议名称为空、过长或含控制字符时返回错误。
    pub fn json_object(
        protocol: impl Into<String>,
        body: Map<String, Value>,
    ) -> Result<Self, OperationError> {
        let protocol = protocol.into();
        validate_text(&protocol, 64, true, None).map_err(|_| OperationError::EmptyField {
            field: "protocol_payload protocol",
        })?;
        Ok(Self {
            protocol,
            body,
            context: Map::new(),
        })
    }

    /// 返回协议名称。
    #[must_use]
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    /// 返回仅供对应 Provider 解释的 JSON object。
    #[must_use]
    pub const fn body(&self) -> &Map<String, Value> {
        &self.body
    }

    /// 附着不写入 wire body 的协议连接上下文。
    ///
    /// Core 只保留该值；对应 Provider 可以读取已知键，未知键必须忽略。
    #[must_use]
    pub fn with_context(mut self, context: Map<String, Value>) -> Self {
        self.context = context;
        self
    }

    /// 返回仅供对应 Provider 解释的非 wire 上下文。
    #[must_use]
    pub const fn context(&self) -> &Map<String, Value> {
        &self.context
    }
}

impl fmt::Debug for ProtocolPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtocolPayload")
            .field("protocol", &self.protocol)
            .field("body", &"<not included in Debug>")
            .field("context", &"<not included in Debug>")
            .finish()
    }
}

/// 协议 adapter 与 Provider 之间不透明传递的 JSON 正文。
///
/// Core 保留字节与非 wire 上下文，不验证、解析或改写正文；
/// 请求载荷与原生模型目录均由协议 adapter 和对应 Provider 共同拥有语义。
#[derive(Clone, PartialEq, Eq)]
pub struct RawJsonPayload {
    protocol: String,
    body: Bytes,
    context: Map<String, Value>,
    translated: bool,
}

impl RawJsonPayload {
    /// 创建不透明的协议 JSON 正文。
    ///
    /// # Errors
    ///
    /// 协议名称为空、过长或含控制字符时返回错误。
    pub fn new(protocol: impl Into<String>, body: Bytes) -> Result<Self, OperationError> {
        let protocol = protocol.into();
        validate_text(&protocol, 64, true, None).map_err(|_| OperationError::EmptyField {
            field: "raw_json_payload protocol",
        })?;
        Ok(Self {
            protocol,
            translated: false,
            body,
            context: Map::new(),
        })
    }

    /// 返回协议名称。
    #[must_use]
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    /// 返回协议正文的 JSON 字节。
    #[must_use]
    pub const fn body(&self) -> &Bytes {
        &self.body
    }

    /// 附着不写入 wire body 的协议连接上下文。
    #[must_use]
    pub fn with_context(mut self, context: Map<String, Value>) -> Self {
        self.context = context;
        self
    }

    /// 返回仅供对应 Provider 解释的非 wire 上下文。
    #[must_use]
    pub const fn context(&self) -> &Map<String, Value> {
        &self.context
    }
}

impl fmt::Debug for RawJsonPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RawJsonPayload")
            .field("protocol", &self.protocol)
            .field("body", &"<not included in Debug>")
            .field("context", &"<not included in Debug>")
            .finish()
    }
}

/// 通用生成请求。
#[derive(Clone, PartialEq)]
pub struct GenerateRequest {
    payload: Arc<GeneratePayload>,
}

#[derive(Clone, PartialEq)]
struct GeneratePayload {
    source_image_generation: bool,
    wire_facts: RequestFacts,
    source_requirements: Option<CapabilityRequirements>,
    protocol_payload: ProtocolPayload,
    client_turn: Option<ClientTurn>,
    provider_session_state: Option<ProviderSessionState>,
    quality_probe: Option<quality_probe::QualityProbeStep>,
    codex_quota_overdraft_probe: bool,
    excel_recovery_revision: Option<u64>,
    excel_recovery_nonce: Option<String>,
}

/// Confirmed protocol identities, independent of account and connection routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTurn {
    thread_id: String,
    turn_id: String,
}

impl ClientTurn {
    pub fn new(thread_id: String, turn_id: String) -> Option<Self> {
        if [&thread_id, &turn_id].iter().any(|value| {
            value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control)
        }) {
            return None;
        }
        Some(Self { thread_id, turn_id })
    }
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }
    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }
}

impl GenerateRequest {
    #[must_use]
    pub fn with_client_turn(mut self, turn: Option<ClientTurn>) -> Self {
        Arc::make_mut(&mut self.payload).client_turn = turn;
        self
    }
    pub fn client_turn(&self) -> Option<&ClientTurn> {
        self.payload.client_turn.as_ref()
    }
    /// 创建携带协议不透明正文的生成请求。
    ///
    /// 完整请求只以原始 wire object 保存；Core 仅局部只读已知字段以推导路由能力，
    /// Provider 负责解释与转换正文。
    #[must_use]
    pub fn from_protocol_payload(protocol_payload: ProtocolPayload) -> Self {
        Self {
            payload: Arc::new(GeneratePayload {
                source_image_generation: false,
                wire_facts: request_facts(protocol_payload.protocol(), protocol_payload.body()),
                source_requirements: None,
                protocol_payload,
                client_turn: None,
                provider_session_state: None,
                quality_probe: None,
                codex_quota_overdraft_probe: false,
                excel_recovery_revision: None,
                excel_recovery_nonce: None,
            }),
        }
    }

    /// 附着同一客户端连接上一轮由 Provider 返回的不透明状态。
    #[must_use]
    pub fn with_provider_session_state(mut self, state: ProviderSessionState) -> Self {
        self.set_provider_session_state(state);
        self
    }

    /// 原地附着会话状态；payload 独占时不复制正文。
    pub fn set_provider_session_state(&mut self, state: ProviderSessionState) {
        Arc::make_mut(&mut self.payload).provider_session_state = Some(state);
    }

    #[must_use]
    pub fn with_quality_probe(mut self, step: quality_probe::QualityProbeStep) -> Self {
        Arc::make_mut(&mut self.payload).quality_probe = Some(step);
        self
    }

    #[must_use]
    pub fn quality_probe(&self) -> Option<&quality_probe::QualityProbeStep> {
        self.payload.quality_probe.as_ref()
    }

    #[must_use]
    pub fn with_codex_quota_overdraft_probe(mut self) -> Self {
        Arc::make_mut(&mut self.payload).codex_quota_overdraft_probe = true;
        self
    }

    #[must_use]
    pub fn codex_quota_overdraft_probe(&self) -> bool {
        self.payload.codex_quota_overdraft_probe
    }

    pub(crate) fn with_excel_recovery(mut self, revision: u64, nonce: String) -> Self {
        let payload = Arc::make_mut(&mut self.payload);
        payload.excel_recovery_revision = Some(revision);
        payload.excel_recovery_nonce = Some(nonce);
        self
    }

    #[must_use]
    pub fn excel_recovery_nonce(&self) -> Option<&str> {
        self.payload.excel_recovery_nonce.as_deref()
    }

    #[must_use]
    pub fn excel_recovery_revision(&self) -> Option<u64> {
        self.payload.excel_recovery_revision
    }

    /// 返回最大输出 token 数。
    #[must_use]
    pub fn max_output_tokens(&self) -> Option<u64> {
        self.payload.wire_facts.max_output_tokens
    }

    /// 返回客户端提供的 prompt cache 路由键。
    #[must_use]
    pub fn prompt_cache_key(&self) -> Option<&str> {
        self.body()
            .get("prompt_cache_key")
            .and_then(Value::as_str)
            .filter(|key| !key.trim().is_empty())
    }

    /// 返回原始请求是否要求 Provider 原生 continuation。
    #[must_use]
    pub fn native_continuation_requested(&self) -> bool {
        self.payload.wire_facts.native_continuation
    }

    /// 返回客户端是否请求了图片生成工具。
    #[must_use]
    pub fn image_generation_requested(&self) -> bool {
        self.payload.source_image_generation || self.payload.wire_facts.image_generation
    }

    /// 返回指定 Provider 的连接内会话状态。
    #[must_use]
    pub fn provider_session_state(&self, provider: &str) -> Option<&ProviderSessionState> {
        self.payload
            .provider_session_state
            .as_ref()
            .filter(|state| state.provider() == provider)
    }

    /// 返回协议不透明正文。
    #[must_use]
    pub fn protocol_payload(&self) -> &ProtocolPayload {
        &self.payload.protocol_payload
    }

    fn requirements(&self) -> CapabilityRequirements {
        if let Some(requirements) = &self.payload.source_requirements {
            return requirements.clone();
        }
        self.wire_capability_requirements()
    }

    /// 返回当前 wire 的能力要求，不混入转换前或中间件继承的要求
    #[must_use]
    pub fn wire_capability_requirements(&self) -> CapabilityRequirements {
        let facts = self.payload.wire_facts;
        let mut requirements = CapabilityRequirements::new(OperationKind::Generate)
            .with_requested_output_tokens(facts.max_output_tokens);
        for (requested, feature) in [
            (facts.tools, Feature::Tools),
            (facts.vision, Feature::Vision),
            (facts.reasoning, Feature::Reasoning),
            (facts.json_schema, Feature::JsonSchema),
            (facts.native_continuation, Feature::NativeContinuation),
        ] {
            if requested {
                requirements = requirements.require(feature);
            }
        }
        requirements
    }

    fn body(&self) -> &Map<String, Value> {
        self.protocol_payload().body()
    }
}

impl fmt::Debug for GenerateRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GenerateRequest")
            .field(
                "provider_session_state",
                &self
                    .payload
                    .provider_session_state
                    .as_ref()
                    .map(|_| "<present>"),
            )
            .field("protocol_payload", self.protocol_payload())
            .finish()
    }
}

/// 使用目标模型的 Provider 原生 Token 计数请求
#[derive(Clone, PartialEq)]
pub struct TokenCountRequest {
    payload: RawJsonPayload,
}

impl TokenCountRequest {
    #[must_use]
    pub const fn from_raw_json(payload: RawJsonPayload) -> Self {
        Self { payload }
    }

    #[must_use]
    pub const fn payload(&self) -> &RawJsonPayload {
        &self.payload
    }
}

impl fmt::Debug for TokenCountRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenCountRequest")
            .field("payload", &"<not included in Debug>")
            .finish()
    }
}

/// 图像 API 的稳定端点语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageRequestKind {
    /// 从文本生成图像。
    Generation,
    /// 使用输入图像执行编辑。
    Edit,
}

/// 图像生成或编辑请求。
#[derive(Clone, PartialEq)]
pub struct ImageRequest {
    kind: ImageRequestKind,
    payload: RawJsonPayload,
}

impl ImageRequest {
    /// 创建携带原始协议 JSON 正文的图像请求。
    #[must_use]
    pub const fn from_raw_json(kind: ImageRequestKind, payload: RawJsonPayload) -> Self {
        Self { kind, payload }
    }

    /// 返回图像 API 端点语义。
    #[must_use]
    pub const fn kind(&self) -> ImageRequestKind {
        self.kind
    }

    /// 返回未经重编码的协议 JSON 正文。
    #[must_use]
    pub const fn payload(&self) -> &RawJsonPayload {
        &self.payload
    }
}

impl fmt::Debug for ImageRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageRequest")
            .field("kind", &self.kind)
            .field("payload", &"<not included in Debug>")
            .finish()
    }
}

/// Provider 原生 standalone search 请求。
#[derive(Clone, PartialEq, Eq)]
pub struct StandaloneSearchRequest {
    payload: RawJsonPayload,
}

impl StandaloneSearchRequest {
    /// 创建携带原始协议 JSON 正文的 standalone search 请求。
    #[must_use]
    pub const fn from_raw_json(payload: RawJsonPayload) -> Self {
        Self { payload }
    }

    /// 返回未经重编码的协议 JSON 正文。
    #[must_use]
    pub const fn payload(&self) -> &RawJsonPayload {
        &self.payload
    }
}

impl fmt::Debug for StandaloneSearchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StandaloneSearchRequest")
            .field("payload", &"<not included in Debug>")
            .finish()
    }
}

/// 显式上下文压缩请求；必须通过模型路由，而非无模型 Provider 端点准入。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CompactRequest {
    payload: RawJsonPayload,
}

impl CompactRequest {
    #[must_use]
    pub const fn from_raw_json(payload: RawJsonPayload) -> Self {
        Self { payload }
    }

    #[must_use]
    pub const fn payload(&self) -> &RawJsonPayload {
        &self.payload
    }
}

/// 原生 HTTP 端点的原始正文；Core 不解释其内容。
#[derive(Clone, PartialEq, Eq)]
pub struct RawHttpPayload {
    protocol: String,
    body: Bytes,
    context: Map<String, Value>,
    translated: bool,
}

impl RawHttpPayload {
    /// 创建不透明 HTTP 正文
    ///
    /// # Errors
    ///
    /// 协议名称为空、过长或含控制字符时返回错误
    pub fn new(protocol: impl Into<String>, body: Bytes) -> Result<Self, OperationError> {
        let protocol = protocol.into();
        validate_text(&protocol, 64, true, None).map_err(|_| OperationError::EmptyField {
            field: "raw_http_payload protocol",
        })?;
        Ok(Self {
            protocol,
            body,
            context: Map::new(),
            translated: false,
        })
    }

    #[must_use]
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    #[must_use]
    pub const fn body(&self) -> &Bytes {
        &self.body
    }

    #[must_use]
    pub fn with_context(mut self, context: Map<String, Value>) -> Self {
        self.context = context;
        self
    }

    #[must_use]
    pub const fn context(&self) -> &Map<String, Value> {
        &self.context
    }
}

impl fmt::Debug for RawHttpPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RawHttpPayload")
            .field("protocol", &self.protocol)
            .field("body", &"<not included in Debug>")
            .field("context", &"<not included in Debug>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderHttpMethod {
    Get,
    Post,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderHttpHeader {
    name: String,
    value: Bytes,
}

impl ProviderHttpHeader {
    pub fn new(name: impl Into<String>, value: Bytes) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn value(&self) -> &Bytes {
        &self.value
    }
}

impl fmt::Debug for ProviderHttpHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderHttpHeader")
            .field("name", &self.name)
            .field("value", &"<not included in Debug>")
            .finish()
    }
}

/// Only a fixed Provider endpoint name, bounded query, and bounded protocol headers are accepted.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderHttpRequest {
    endpoint: String,
    method: ProviderHttpMethod,
    query: Option<String>,
    headers: Vec<ProviderHttpHeader>,
    payload: RawHttpPayload,
}

impl ProviderHttpRequest {
    pub fn new(
        endpoint: impl Into<String>,
        method: ProviderHttpMethod,
        query: Option<String>,
        headers: Vec<ProviderHttpHeader>,
        payload: RawHttpPayload,
    ) -> Result<Self, OperationError> {
        let endpoint = endpoint.into();
        let valid_endpoint = !endpoint.is_empty()
            && endpoint.len() <= 64
            && endpoint != "."
            && endpoint != ".."
            && endpoint
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
        let valid_query = query.as_ref().is_none_or(|value| {
            value.len() <= 8 * 1024 && !value.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | 0))
        });
        let valid_headers = headers.len() <= 64
            && headers
                .iter()
                .try_fold(0_usize, |total, header| {
                    let name = header.name.as_bytes();
                    let value = header.value.as_ref();
                    (!name.is_empty()
                        && name.len() <= 128
                        && http::HeaderName::from_bytes(name).is_ok()
                        && value.len() <= 8 * 1024
                        && !value.iter().any(|byte| matches!(byte, b'\r' | b'\n' | 0)))
                    .then_some(())
                    .and_then(|()| total.checked_add(name.len() + value.len()))
                    .filter(|total| *total <= 32 * 1024)
                })
                .is_some();
        if !valid_endpoint || !valid_query || !valid_headers {
            return Err(OperationError::EmptyField {
                field: "provider_http_request",
            });
        }
        Ok(Self {
            endpoint,
            method,
            query,
            headers,
            payload,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub const fn method(&self) -> ProviderHttpMethod {
        self.method
    }
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }
    pub fn headers(&self) -> &[ProviderHttpHeader] {
        &self.headers
    }
    pub const fn payload(&self) -> &RawHttpPayload {
        &self.payload
    }
}

impl fmt::Debug for ProviderHttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderHttpRequest")
            .field("endpoint", &self.endpoint)
            .field("method", &self.method)
            .field("query", &self.query.as_ref().map(|_| "<present>"))
            .field("header_count", &self.headers.len())
            .field("payload", &self.payload)
            .finish()
    }
}

/// 网关内部业务请求；不包含任何客户端 wire 或 Provider SDK 类型。
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub enum Operation {
    /// 生成。
    Generate(GenerateRequest),
    /// 图像生成。
    GenerateImage(ImageRequest),
    CountTokens(TokenCountRequest),
    /// Provider 原生 standalone search。
    Search(StandaloneSearchRequest),
    /// 显式上下文压缩。
    Compact(CompactRequest),
    /// Provider 原生 HTTP 端点。
    ProviderHttp(ProviderHttpRequest),
}

impl Operation {
    /// 替换协议正文和非 wire 上下文，同时保留 operation 类别、Provider 会话状态与
    /// 原协议名称
    /// `body=None` 表示只改上下文，避免无加工路径重新编码正文
    ///
    /// # Errors
    ///
    /// Generate 的替换正文不是 JSON object 时返回错误；原始 JSON 端点继续按字节保存
    pub fn replace_protocol_wire(
        self,
        body: Option<Bytes>,
        context: Map<String, Value>,
    ) -> Result<Self, OperationError> {
        self.replace_protocol_wire_inner(None, body, context)
    }

    /// 把一次已选择的协议转换结果写回 operation，同时保留非 wire 上下文和
    /// Provider 会话状态
    /// 该方法只验证公共载荷形状，不解释目标协议字段
    ///
    /// # Errors
    ///
    /// 目标协议名无效，或 Generate 的目标正文不是 JSON object 时返回错误
    pub fn translate_protocol_wire(
        self,
        target_protocol: impl Into<String>,
        body: Bytes,
    ) -> Result<Self, OperationError> {
        let target_protocol = target_protocol.into();
        validate_text(&target_protocol, 64, true, None).map_err(|_| {
            OperationError::EmptyField {
                field: "translated protocol",
            }
        })?;
        if self.protocol() == target_protocol || self.protocol_was_translated() {
            return Err(OperationError::EmptyField {
                field: "direct protocol translation",
            });
        }
        let context = match &self {
            Self::Generate(request) => request.protocol_payload().context().clone(),
            Self::GenerateImage(request) => request.payload().context().clone(),
            Self::Search(request) => request.payload().context().clone(),
            Self::Compact(request) => request.payload().context().clone(),
            Self::CountTokens(request) => request.payload().context().clone(),
            Self::ProviderHttp(request) => request.payload().context().clone(),
        };
        self.replace_protocol_wire_inner(Some(target_protocol), Some(body), context)
    }

    fn protocol_was_translated(&self) -> bool {
        match self {
            Self::Generate(request) => request.payload.source_requirements.is_some(),
            Self::GenerateImage(request) => request.payload().translated,
            Self::Search(request) => request.payload().translated,
            Self::Compact(request) => request.payload().translated,
            Self::CountTokens(request) => request.payload().translated,
            Self::ProviderHttp(request) => request.payload().translated,
        }
    }

    fn replace_protocol_wire_inner(
        self,
        target_protocol: Option<String>,
        body: Option<Bytes>,
        context: Map<String, Value>,
    ) -> Result<Self, OperationError> {
        let translating = target_protocol.is_some();
        match self {
            Self::Generate(request) => {
                let source_requirements = translating.then(|| request.requirements());
                let source_image_generation = translating && request.image_generation_requested();
                // 修改上下文不改变正文事实；只有新 wire 才需要重新扫描输入
                let mut payload = Arc::unwrap_or_clone(request.payload);
                if let Some(body) = body {
                    let body =
                        serde_json::from_slice::<Map<String, Value>>(&body).map_err(|_| {
                            OperationError::EmptyField {
                                field: "protocol_payload body",
                            }
                        })?;
                    if let Some(protocol) = target_protocol {
                        payload.protocol_payload.protocol = protocol;
                    }
                    payload.wire_facts = request_facts(payload.protocol_payload.protocol(), &body);
                    payload.protocol_payload.body = body;
                }
                payload.protocol_payload.context = context;
                // 跨协议转换不能抹去已识别的生图权限要求，也不解释目标私有协议
                payload.source_image_generation |= source_image_generation;
                if let Some(requirements) = source_requirements {
                    payload.source_requirements = Some(requirements);
                }
                Ok(Self::Generate(GenerateRequest {
                    payload: Arc::new(payload),
                }))
            }
            Self::GenerateImage(request) => {
                let payload = request.payload;
                Ok(Self::GenerateImage(ImageRequest {
                    kind: request.kind,
                    payload: RawJsonPayload {
                        protocol: target_protocol.unwrap_or(payload.protocol),
                        body: body.unwrap_or(payload.body),
                        context,
                        translated: payload.translated || translating,
                    },
                }))
            }
            Self::Search(request) => {
                let payload = request.payload;
                Ok(Self::Search(StandaloneSearchRequest {
                    payload: RawJsonPayload {
                        protocol: target_protocol.unwrap_or(payload.protocol),
                        body: body.unwrap_or(payload.body),
                        context,
                        translated: payload.translated || translating,
                    },
                }))
            }
            Self::Compact(request) => {
                let payload = request.payload;
                Ok(Self::Compact(CompactRequest {
                    payload: RawJsonPayload {
                        protocol: target_protocol.unwrap_or(payload.protocol),
                        body: body.unwrap_or(payload.body),
                        context,
                        translated: payload.translated || translating,
                    },
                }))
            }
            Self::CountTokens(request) => {
                let payload = request.payload;
                Ok(Self::CountTokens(TokenCountRequest {
                    payload: RawJsonPayload {
                        protocol: target_protocol.unwrap_or(payload.protocol),
                        body: body.unwrap_or(payload.body),
                        context,
                        translated: payload.translated || translating,
                    },
                }))
            }
            Self::ProviderHttp(request) => {
                let payload = request.payload;
                Ok(Self::ProviderHttp(ProviderHttpRequest {
                    endpoint: request.endpoint,
                    method: request.method,
                    query: request.query,
                    headers: request.headers,
                    payload: RawHttpPayload {
                        protocol: target_protocol.unwrap_or(payload.protocol),
                        body: body.unwrap_or(payload.body),
                        context,
                        translated: payload.translated || translating,
                    },
                }))
            }
        }
    }

    /// 返回协议 adapter 持有的不透明格式标识
    #[must_use]
    pub fn protocol(&self) -> &str {
        match self {
            Self::Generate(request) => request.protocol_payload().protocol(),
            Self::GenerateImage(request) => request.payload().protocol(),
            Self::Search(request) => request.payload().protocol(),
            Self::Compact(request) => request.payload().protocol(),
            Self::CountTokens(request) => request.payload().protocol(),
            Self::ProviderHttp(request) => request.payload().protocol(),
        }
    }

    /// 附着协议连接持有的 Provider 私有状态；非生成 operation 保持不变。
    #[must_use]
    pub fn with_provider_session_state(self, state: ProviderSessionState) -> Self {
        match self {
            Self::Generate(request) => Self::Generate(request.with_provider_session_state(state)),
            operation => operation,
        }
    }

    /// 原地附着 Provider 私有状态；payload 独占时不复制正文。
    pub fn set_provider_session_state(&mut self, state: ProviderSessionState) {
        if let Self::Generate(request) = self {
            request.set_provider_session_state(state);
        }
    }

    /// 返回稳定 operation 分类。
    #[must_use]
    pub const fn kind(&self) -> OperationKind {
        match self {
            Self::Generate(_) => OperationKind::Generate,
            Self::GenerateImage(_) => OperationKind::GenerateImage,
            Self::CountTokens(_) => OperationKind::CountTokens,
            Self::Search(_) => OperationKind::Search,
            Self::Compact(_) => OperationKind::Compact,
            Self::ProviderHttp(_) => OperationKind::ProviderHttp,
        }
    }

    /// 推导 Router 使用的能力要求。
    #[must_use]
    pub fn capability_requirements(&self) -> CapabilityRequirements {
        match self {
            Self::Generate(request) => request.requirements(),
            Self::GenerateImage(_) => CapabilityRequirements::new(OperationKind::GenerateImage),
            Self::CountTokens(_) => CapabilityRequirements::new(OperationKind::CountTokens),
            Self::Search(_) => CapabilityRequirements::new(OperationKind::Search),
            Self::Compact(_) => CapabilityRequirements::new(OperationKind::Compact),
            Self::ProviderHttp(_) => CapabilityRequirements::new(OperationKind::ProviderHttp),
        }
    }

    /// 返回该 operation 是否代表一次图片生成请求。
    #[must_use]
    pub fn image_generation_requested(&self) -> bool {
        match self {
            Self::Generate(request) => request.image_generation_requested(),
            Self::GenerateImage(_) => true,
            Self::Search(_) | Self::CountTokens(_) | Self::Compact(_) | Self::ProviderHttp(_) => {
                false
            }
        }
    }

    /// 返回当前 Provider 的连接内私有状态。
    #[must_use]
    pub fn provider_session_state(&self, provider: &str) -> Option<&ProviderSessionState> {
        match self {
            Self::Generate(request) => request.provider_session_state(provider),
            Self::GenerateImage(_)
            | Self::Search(_)
            | Self::CountTokens(_)
            | Self::Compact(_)
            | Self::ProviderHttp(_) => None,
        }
    }
}

impl fmt::Debug for Operation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Operation")
            .field("kind", &self.kind())
            .field("payload", &"<not included in Debug>")
            .finish()
    }
}
