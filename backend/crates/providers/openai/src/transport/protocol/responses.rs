use std::fmt;

use gateway_protocol::openai::{
    CodexResponsesRequestSemantics as CodexRequestSemantics, OpenAiSchedulingSessionHint,
    codex_responses_request_semantics_with_turn_metadata, events,
};
use reqwest::header::HeaderMap;
use serde::Serialize;
use serde_json::{Map, Value};

/// One precommit grace window shared by WS transport and Provider buffering.
pub(crate) const STREAM_REPLAY_GRACE: std::time::Duration = std::time::Duration::from_millis(2_500);

/// 官方 Codex 客户端据此触发完整历史重放的稳定错误码。
pub(crate) const PREVIOUS_RESPONSE_NOT_FOUND_CODE: &str = "previous_response_not_found";
/// Responses WebSocket 用于回传同一 turn 不透明状态的官方 client metadata 键。
pub(crate) const X_CODEX_TURN_STATE_CLIENT_METADATA_KEY: &str = "x-codex-turn-state";
/// 本地生成 history unavailable 错误时使用的官方提示文本。
pub(crate) const PREVIOUS_RESPONSE_NOT_FOUND_MESSAGE: &str =
    "Previous response was not found. Retrying the full request.";
/// Codex Responses 上游请求体。
///
/// 发往上游的 Responses 请求。`body` 持有客户端原始 JSON object，逐字段（含顺序、
/// 含未知字段）透传上游，是上游请求体的唯一来源；`use_websocket`/`force_http_sse`
/// 仅用于本地传输选择，不写入 body。常用字段通过访问器方法读写。
/// 普通客户端请求不修改 body；模型路由只写入明确受控字段。
///
/// 其余字段是代理控制状态，不进上游 body（原 `#[serde(skip)]` 字段）。
#[derive(Clone)]
pub struct CodexResponsesRequest {
    pub(crate) quality_probe: Option<gateway_core::operation::quality_probe::QualityProbeStep>,
    /// Trusted recovery expectation, never read from or serialized into client JSON.
    pub(crate) excel_recovery_nonce: Option<String>,
    pub(crate) excel: Option<crate::transport::excel::ExcelPreparedRequest>,
    /// 上游请求体（唯一真相源）。
    body: Map<String, Value>,
    /// API 边界保存、Provider 逐条恢复的普通客户端请求头。
    pub(crate) passthrough_headers: HeaderMap,
    /// 是否由客户端显式提供了 prompt cache key。
    pub explicit_prompt_cache_key: bool,
    /// 客户端会话 ID。
    pub client_conversation_id: Option<String>,
    /// 客户端 session ID，仅保留在受控本地上下文。
    pub client_session_id: Option<String>,
    /// 客户端 thread ID，仅保留在受控本地上下文。
    pub client_thread_id: Option<String>,
    /// Local account-selection hint; never part of upstream or connection identity.
    pub(crate) scheduling_session_hint: Option<OpenAiSchedulingSessionHint>,
    /// 客户端 request ID，仅保留在受控本地上下文。
    pub client_request_id: Option<String>,
    /// 客户端 turn ID，仅保留在受控本地上下文。
    pub client_turn_id: Option<String>,
    /// 连接池和 affinity 使用的本地会话身份，不发送上游。
    pub local_conversation_id: Option<String>,
    /// 变体身份键。
    pub variant_identity: Option<String>,
    /// 代理侧识别的客户端 IP，仅用于管理端使用记录展示。
    pub client_ip: Option<String>,
    /// 客户端 User-Agent，仅用于管理端使用记录展示。
    pub client_user_agent: Option<String>,
    /// 已鉴权客户端 API key 的稳定 ID，仅用于事实归因。
    pub client_api_key_id: Option<String>,
    /// Original protocol anchors retained across wire projections and owned continuations.
    pub(crate) identity_seed: Option<crate::transport::qx_application::CodexIdentitySeed>,
    /// 是否偏好 WebSocket 传输。
    pub use_websocket: bool,
    /// 是否强制 HTTP SSE。
    pub force_http_sse: bool,
    /// turn state 透传头。
    pub turn_state: Option<String>,
    /// 代理托管 state 的版本；只参与本地 WS 新链连接隔离，不发送上游。
    /// turn metadata 透传头。
    pub turn_metadata: Option<String>,
    /// beta features 透传头。
    pub beta_features: Option<String>,
    /// 下游 `version` 扩展头；存在时上游值由 Desktop 版本画像统一生成。
    pub version: Option<String>,
    /// timing metrics 透传头。
    pub include_timing_metrics: Option<String>,
    /// Responses Lite 请求语义；HTTP 使用 header，WebSocket 使用 client metadata 投影。
    pub responses_lite: Option<String>,
    /// Memory consolidation 请求语义；HTTP 与 WebSocket opening 均使用 header。
    pub memgen_request: Option<String>,
    /// Codex window ID。
    pub codex_window_id: Option<String>,
    /// 代理分配的下游 WebSocket 连接 ID，仅用于隔离本地连接池通道。
    pub downstream_websocket_connection_id: Option<String>,
    /// 父线程 ID。
    pub parent_thread_id: Option<String>,
    /// 已知 previous response 的持久化范围，仅用于本地 transport 校验。
    pub previous_response_scope: Option<PreviousResponseScope>,
}

impl fmt::Debug for CodexResponsesRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexResponsesRequest")
            .field("body", &"<not included in Debug>")
            .field("explicit_prompt_cache_key", &self.explicit_prompt_cache_key)
            .field(
                "has_local_conversation_id",
                &self.local_conversation_id.is_some(),
            )
            .field("use_websocket", &self.use_websocket)
            .field("force_http_sse", &self.force_http_sse)
            .finish()
    }
}

/// previous response 在上游的可续接范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviousResponseScope {
    Persisted,
    ConnectionLocal,
    ExternalUnknown,
}

impl Serialize for CodexResponsesRequest {
    /// 上游 body 序列化即原始 `body` map（HTTP SSE 直发；WebSocket 在外层前置 `type`）。
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.body.serialize(serializer)
    }
}

/// Codex Responses 请求对上游传输的显式要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportRequirement {
    /// 客户端显式要求 HTTP。
    HttpRequired,
    /// `generate=false + store=false` 预热必须保留在同一条 WebSocket。
    ExplicitWebSocketWarmup,
    /// 客户端 WebSocket 的非持久化新链，必须在池化连接上建立后续续接状态。
    WebSocketNewChain,
    /// 只能使用持有指定 connection-local response 的精确 WebSocket。
    ExactWebSocketContinuation,
    /// previous response 已持久化，允许 WebSocket 或 HTTP/2。
    PersistedContinuation,
    /// previous response 的所有权未知，只允许当前选定账号原样尝试。
    ExternalUnknown,
    /// 没有 previous response 的普通新链。
    NewChain,
}

impl TransportRequirement {
    /// 是否必须使用 WebSocket，且禁止 HTTP fallback。
    pub fn requires_websocket(self) -> bool {
        matches!(
            self,
            Self::ExplicitWebSocketWarmup
                | Self::WebSocketNewChain
                | Self::ExactWebSocketContinuation
        )
    }

    /// WebSocket 尚未发送 payload 时失败，是否允许切到同账号 HTTP/2。
    pub fn allows_pre_send_http_fallback(self) -> bool {
        matches!(
            self,
            Self::PersistedContinuation | Self::ExternalUnknown | Self::NewChain
        )
    }

    /// 无续接依赖的新请求可以在明确的连接级拒绝后重新建连。
    pub fn allows_connection_restart(self) -> bool {
        matches!(self, Self::NewChain | Self::WebSocketNewChain)
    }

    /// 用于审计与遥测的稳定名称。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HttpRequired => "http_required",
            Self::ExplicitWebSocketWarmup => "explicit_websocket_warmup",
            Self::WebSocketNewChain => "websocket_new_chain",
            Self::ExactWebSocketContinuation => "exact_websocket_continuation",
            Self::PersistedContinuation => "persisted_continuation",
            Self::ExternalUnknown => "external_unknown",
            Self::NewChain => "new_chain",
        }
    }
}

/// 将已完成 history preparation 的请求规范化为唯一 transport requirement。
pub fn transport_requirement(request: &CodexResponsesRequest) -> TransportRequirement {
    if request.excel.is_some() {
        return TransportRequirement::HttpRequired;
    }
    if !request.generate() && !request.store() {
        return TransportRequirement::ExplicitWebSocketWarmup;
    }
    if request.force_http_sse {
        return TransportRequirement::HttpRequired;
    }
    match request.previous_response_id() {
        Some(_) => match request.previous_response_scope {
            Some(PreviousResponseScope::Persisted) => TransportRequirement::PersistedContinuation,
            Some(PreviousResponseScope::ConnectionLocal) => {
                TransportRequirement::ExactWebSocketContinuation
            }
            Some(PreviousResponseScope::ExternalUnknown) | None => {
                TransportRequirement::ExternalUnknown
            }
        },
        // 客户端会在下一轮提交 response ID；HTTP store=false 的成功响应无法
        // 在池化 WebSocket 上续接，所以首轮也不能按普通快路径降级到 HTTP。
        None if request.downstream_websocket_connection_id.is_some() && !request.store() => {
            TransportRequirement::WebSocketNewChain
        }
        None => TransportRequirement::NewChain,
    }
}

/// 单个 Responses 事件对计时系统提供的稳定语义信号。
///
/// `output_start` observes the first non-preamble event. Semantic output still
/// requires content and remains the retry/delivery boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResponseEventSignals {
    pub protocol_progress: bool,
    pub output_start: bool,
    pub semantic_output: bool,
    pub reasoning_output: bool,
    pub text_output: bool,
}

/// 从已解析的 Responses 事件提取计时语义。
///
/// First-response timing includes structural output events, but not preamble,
/// heartbeat, quota or failure events. Content timing remains separate.
pub fn response_event_signals(event_type: Option<&str>, value: &Value) -> ResponseEventSignals {
    let mut signals = ResponseEventSignals {
        protocol_progress: !matches!(event_type, Some("response.failed" | "error")),
        output_start: event_type.is_some_and(|kind| {
            !matches!(
                kind,
                "" | "response.created"
                    | "response.in_progress"
                    | "keepalive"
                    | "codex.rate_limits"
                    | "response.failed"
                    | "error"
            )
        }),
        ..ResponseEventSignals::default()
    };
    match event_type {
        Some("response.output_text.delta") => {
            signals.text_output = non_empty_string(value.get("delta"));
            signals.semantic_output = signals.text_output;
        }
        Some("response.output_text.done") => {
            signals.text_output = non_empty_string(value.get("text"));
            signals.semantic_output = signals.text_output;
        }
        Some("response.refusal.delta") => {
            signals.text_output = non_empty_string(value.get("delta"));
            signals.semantic_output = signals.text_output;
        }
        Some("response.refusal.done") => {
            signals.text_output = non_empty_string(value.get("refusal"));
            signals.semantic_output = signals.text_output;
        }
        Some("response.reasoning_summary_text.delta" | "response.reasoning_text.delta") => {
            signals.reasoning_output = non_empty_string(value.get("delta"));
            signals.semantic_output = signals.reasoning_output;
        }
        Some("response.reasoning_summary_text.done" | "response.reasoning_text.done") => {
            signals.reasoning_output =
                non_empty_string(value.get("text").or_else(|| value.get("summary")));
            signals.semantic_output = signals.reasoning_output;
        }
        Some(
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta",
        ) => {
            signals.semantic_output = non_empty_string(value.get("delta"));
        }
        Some("response.function_call_arguments.done") => {
            signals.semantic_output = non_empty_string(value.get("arguments"));
        }
        Some("response.custom_tool_call_input.done") => {
            signals.semantic_output = non_empty_string(value.get("input"));
        }
        Some("response.image_generation_call.partial_image") => {
            signals.semantic_output = non_empty_string(
                value
                    .get("partial_image_b64")
                    .or_else(|| value.get("partial_image")),
            );
        }
        Some("response.output_item.done") => {
            if let Some(item) = value.get("item") {
                merge_output_signals(&mut signals, output_item_signals(item));
            }
        }
        Some("response.content_part.done") => {
            if let Some(part) = value.get("part") {
                merge_output_signals(&mut signals, output_item_signals(part));
            }
        }
        Some("response.completed" | "response.incomplete") => {
            if let Some(items) = value.pointer("/response/output").and_then(Value::as_array) {
                merge_output_signals(&mut signals, output_items_signals(items));
            }
        }
        Some(event_type) if event_type.ends_with(".delta") => {
            signals.semantic_output = non_empty_semantic_value(value.get("delta"));
        }
        Some(event_type) if event_type.ends_with(".done") => {
            signals.semantic_output = [
                "text",
                "refusal",
                "arguments",
                "input",
                "transcript",
                "data",
                "output",
                "result",
            ]
            .into_iter()
            .any(|field| non_empty_semantic_value(value.get(field)));
        }
        Some(event_type) if is_tool_execution_event(event_type) => {
            signals.semantic_output = true;
        }
        _ => {}
    }
    signals
}

fn merge_output_signals(target: &mut ResponseEventSignals, source: ResponseEventSignals) {
    target.semantic_output |= source.semantic_output;
    target.reasoning_output |= source.reasoning_output;
    target.text_output |= source.text_output;
}

fn output_items_signals(items: &[Value]) -> ResponseEventSignals {
    items
        .iter()
        .fold(ResponseEventSignals::default(), |mut signals, item| {
            merge_output_signals(&mut signals, output_item_signals(item));
            signals
        })
}

fn output_item_signals(item: &Value) -> ResponseEventSignals {
    let mut signals = ResponseEventSignals::default();
    match item.get("type").and_then(Value::as_str) {
        Some("output_text" | "text") => {
            signals.text_output = non_empty_string(item.get("text"));
            signals.semantic_output = signals.text_output;
        }
        Some("reasoning") => {
            signals.reasoning_output = non_empty_semantic_value(item.get("text"))
                || non_empty_semantic_value(item.get("summary"));
            signals.semantic_output = signals.reasoning_output;
        }
        Some("refusal") => {
            signals.text_output =
                non_empty_string(item.get("refusal")) || non_empty_string(item.get("text"));
            signals.semantic_output = signals.text_output;
        }
        Some(item_type) if item_type.ends_with("_call") => {
            // done 的工具调用本身已进入不可安全重试的语义边界；不依赖每种工具的字段表。
            signals.semantic_output = true;
        }
        _ => {
            if let Some(items) = item.get("content").and_then(Value::as_array) {
                merge_output_signals(&mut signals, output_items_signals(items));
            }
        }
    }
    signals
}

fn non_empty_string(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

fn non_empty_semantic_value(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(value)) => !value.is_empty(),
        Some(Value::Object(value)) => !value.is_empty(),
        Some(Value::Number(_) | Value::Bool(_)) => true,
        Some(Value::Null) | None => false,
    }
}

fn is_tool_execution_event(event_type: &str) -> bool {
    let Some((call_type, phase)) = event_type
        .strip_prefix("response.")
        .and_then(|event| event.rsplit_once('.'))
    else {
        return false;
    };
    call_type.ends_with("_call")
        && matches!(
            phase,
            "in_progress" | "searching" | "interpreting" | "completed" | "failed"
        )
}

/// Codex Responses SSE 失败事件。
#[derive(Clone, PartialEq, Eq)]
pub struct ResponsesSseFailure {
    /// SSE event 名称。
    pub event: String,
    /// 上游错误消息。
    pub message: String,
    /// 上游错误码。
    pub upstream_code: Option<String>,
    /// 上游显式错误类型；不从业务码推导。
    pub upstream_type: Option<String>,
    /// 上游显式状态码；不从业务码或错误类型推导。
    pub explicit_status_code: Option<u16>,
    /// 上游显式重试间隔，或从官方限流消息中解析出的重试间隔。
    pub retry_after_seconds: Option<u64>,
    /// 当前错误事件自身携带的请求 ID；不是连接 opening ID。
    pub(crate) request_id: Option<String>,
    /// 上游错误事件的原始 JSON data；只应在明确的失败审计边界读取。
    raw_body: String,
}

impl fmt::Debug for ResponsesSseFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResponsesSseFailure")
            .field("event", &self.event)
            .field("message", &"<redacted>")
            .field("has_upstream_code", &self.upstream_code.is_some())
            .field("has_upstream_type", &self.upstream_type.is_some())
            .field("explicit_status_code", &self.explicit_status_code)
            .field("retry_after_seconds", &self.retry_after_seconds)
            .finish()
    }
}

impl ResponsesSseFailure {
    pub fn from_event(event: &str, value: &Value) -> Self {
        Self::from_raw_event(event, &value.to_string(), value)
    }

    pub(crate) fn from_raw_event(event: &str, raw_body: &str, value: &Value) -> Self {
        Self {
            event: event.to_string(),
            message: failure_message(value).unwrap_or_else(|| "Codex upstream SSE failed".into()),
            upstream_code: failure_code(value),
            upstream_type: failure_type(value),
            explicit_status_code: failure_explicit_status_code(value),
            retry_after_seconds: events::retry_after_seconds_from_value(value),
            request_id:
                crate::transport::diagnostics::CodexUpstreamDiagnostics::error_event_request_id(
                    value,
                ),
            raw_body: raw_body.to_owned(),
        }
    }

    /// 返回上游错误事件未经重编码的 JSON data。
    #[must_use]
    pub fn raw_body(&self) -> &str {
        &self.raw_body
    }
}

fn failure_message(value: &Value) -> Option<String> {
    value
        .pointer("/response/error/message")
        .or_else(|| value.pointer("/error/message"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn failure_code(value: &Value) -> Option<String> {
    value
        .pointer("/response/error/code")
        .or_else(|| value.pointer("/error/code"))
        .or_else(|| value.get("code"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn failure_type(value: &Value) -> Option<String> {
    failure_error(value)
        .and_then(|error| error.get("type"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn failure_explicit_status_code(value: &Value) -> Option<u16> {
    value
        .get("status")
        .or_else(|| value.get("status_code"))
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
}

fn failure_error(value: &Value) -> Option<&Value> {
    value
        .pointer("/response/error")
        .or_else(|| value.get("error"))
}

impl CodexResponsesRequest {
    /// 从客户端原始 Responses JSON object 构造上游请求。
    ///
    /// 客户端提供的字段（含未知字段）原样保留在 `body` 中透传上游。
    /// 协议默认值仅由类型化访问器在本地解释，不写回上游正文。
    pub fn from_body(body: Map<String, Value>) -> Self {
        Self {
            excel: None,
            quality_probe: None,
            excel_recovery_nonce: None,
            body,
            passthrough_headers: HeaderMap::new(),
            explicit_prompt_cache_key: false,
            client_conversation_id: None,
            client_session_id: None,
            client_thread_id: None,
            scheduling_session_hint: None,
            client_request_id: None,
            client_turn_id: None,
            local_conversation_id: None,
            variant_identity: None,
            client_ip: None,
            client_user_agent: None,
            client_api_key_id: None,
            identity_seed: None,
            use_websocket: false,
            force_http_sse: false,
            turn_state: None,
            turn_metadata: None,
            beta_features: None,
            version: None,
            include_timing_metrics: None,
            responses_lite: None,
            memgen_request: None,
            codex_window_id: None,
            downstream_websocket_connection_id: None,
            parent_thread_id: None,
            previous_response_scope: None,
        }
    }

    /// 上游 body 的只读视图。
    pub fn body(&self) -> &Map<String, Value> {
        &self.body
    }

    /// Provider adapter 编码阶段写入已经白名单校验的上游字段。
    pub(crate) fn body_mut(&mut self) -> &mut Map<String, Value> {
        &mut self.body
    }

    // --- body 字段类型化访问器（上游语义字段，透传不重写）---

    /// 模型名。
    pub fn model(&self) -> &str {
        self.body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// 指令文本（缺省空串）。
    pub fn instructions(&self) -> &str {
        self.body
            .get("instructions")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// 输入条目切片（非数组时为空）。
    pub fn input(&self) -> &[Value] {
        self.body
            .get("input")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    /// 是否流式返回。
    pub fn stream(&self) -> bool {
        self.body
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    /// 是否要求上游存储响应。
    pub fn store(&self) -> bool {
        self.body
            .get("store")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// 是否实际生成模型响应；官方预热请求会显式传入 `false`。
    pub fn generate(&self) -> bool {
        self.body
            .get("generate")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    /// reasoning 配置（透传，不规整）。
    pub fn reasoning(&self) -> Option<&Value> {
        self.body.get("reasoning")
    }

    /// 工具定义数组（非数组或空时 None）。
    pub fn tools(&self) -> Option<&[Value]> {
        self.body
            .get("tools")
            .and_then(Value::as_array)
            .filter(|tools| !tools.is_empty())
            .map(Vec::as_slice)
    }

    /// include 列表（透传原值）。
    pub fn include(&self) -> Option<&Value> {
        self.body.get("include")
    }

    /// service tier（透传原值）。
    pub fn service_tier(&self) -> Option<&str> {
        self.body.get("service_tier").and_then(Value::as_str)
    }

    /// Only change the top-level tier; enabling Fast requires catalog evidence.
    pub(crate) fn apply_fast_policy(
        &mut self,
        mode: gateway_core::account::FastMode,
        supports_priority: bool,
    ) -> bool {
        use gateway_core::account::FastMode;
        let tier = self.service_tier().map(str::trim);
        let target = match mode {
            FastMode::Disabled
                if tier.is_some_and(|tier| {
                    tier.eq_ignore_ascii_case("priority") || tier.eq_ignore_ascii_case("fast")
                }) =>
            {
                "default"
            }
            FastMode::Enabled
                if supports_priority
                    && (matches!(self.body.get("service_tier"), None | Some(Value::Null))
                        || tier.is_some_and(|tier| tier.eq_ignore_ascii_case("default"))) =>
            {
                "priority"
            }
            _ => return false,
        };
        self.body
            .insert("service_tier".to_owned(), Value::String(target.to_owned()));
        true
    }
    /// 前一个 response ID。
    pub fn previous_response_id(&self) -> Option<&str> {
        self.body
            .get("previous_response_id")
            .and_then(Value::as_str)
    }

    /// 设置 / 清除前一个 response ID。
    pub fn set_previous_response_id(&mut self, previous_response_id: Option<String>) {
        match previous_response_id {
            Some(value) => {
                self.body
                    .insert("previous_response_id".to_string(), Value::String(value));
            }
            None => {
                self.body.remove("previous_response_id");
                self.previous_response_scope = None;
            }
        }
    }

    /// 提示缓存键。
    pub fn prompt_cache_key(&self) -> Option<&str> {
        self.body.get("prompt_cache_key").and_then(Value::as_str)
    }

    /// client metadata（透传原值）。
    pub fn client_metadata(&self) -> Option<&Value> {
        self.body.get("client_metadata")
    }

    /// 提取 Codex 请求类型、子代理类型、推理预设与压缩语义。
    pub fn semantics(&self) -> CodexRequestSemantics {
        let mut semantics = codex_responses_request_semantics_with_turn_metadata(
            self.body(),
            self.turn_metadata.as_deref(),
        );
        // 官方 generate=false 只准备连接与上下文，不是模型推理。
        // 预热分类参与用量筛选，不能仅凭客户端的 request_kind 提示排除真实推理。
        if !self.generate() {
            semantics.request_kind = Some("prewarm".to_owned());
        } else if semantics.request_kind.as_deref() == Some("prewarm") {
            semantics.request_kind = None;
        }
        semantics
    }

    /// 返回请求语义与 child transport 隔离所用的子代理区分值。
    ///
    /// Codex 原生请求在 turn metadata 中声明 `subagent_kind`；兼容客户端也可通过
    /// `client_metadata.x-openai-subagent` 声明同一语义。它不拆分根会话的账号首选项；
    /// `thread_spawn` 仍用它派生独立 WebSocket/continuation transport identity。
    pub fn subagent_kind(&self) -> Option<String> {
        self.semantics()
            .subagent_kind
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                self.client_metadata()
                    .and_then(Value::as_object)
                    .and_then(|metadata| metadata.get("x-openai-subagent"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            })
    }

    /// 仅识别协议完整且无冲突的 Guardian/review 子请求。
    ///
    /// 该值只用于本地调度偏好。任何元数据冲突、空值、非目标模型或未明确声明
    /// Guardian/review 的请求都会退回既有调度，不会扩大账号亲和范围。
    pub(crate) fn guardian_parent_thread_id(&self) -> Option<String> {
        if self.model() != "codex-auto-review" {
            return None;
        }

        let mut subagent_kinds = Vec::new();
        let mut parent_thread_ids = Vec::new();
        let mut metadata = Vec::new();

        Self::append_turn_metadata(&mut metadata, self.turn_metadata.as_deref())?;
        for key in ["turn_metadata", "turnMetadata", "x-codex-turn-metadata"] {
            if let Some(value) = self.body.get(key) {
                Self::append_body_turn_metadata(&mut metadata, value)?;
            }
        }
        let client_metadata = match self.client_metadata() {
            Some(value) => Some(value.as_object()?),
            None => None,
        };
        if let Some(client_metadata) = client_metadata {
            for key in ["turn_metadata", "turnMetadata", "x-codex-turn-metadata"] {
                if let Some(value) = client_metadata.get(key) {
                    Self::append_body_turn_metadata(&mut metadata, value)?;
                }
            }
        }
        for metadata in &metadata {
            let object = metadata.as_object()?;
            Self::append_required_string(&mut subagent_kinds, object.get("subagent_kind"))?;
            for key in ["parent_thread_id", "parentThreadId"] {
                Self::append_required_string(&mut parent_thread_ids, object.get(key))?;
            }
        }

        Self::append_required_string(
            &mut subagent_kinds,
            client_metadata.and_then(|metadata| metadata.get("x-openai-subagent")),
        )?;
        for key in ["parent_thread_id", "parentThreadId"] {
            Self::append_required_string(
                &mut parent_thread_ids,
                client_metadata.and_then(|metadata| metadata.get(key)),
            )?;
            Self::append_required_string(&mut parent_thread_ids, self.body.get(key))?;
        }
        Self::append_optional_string(&mut parent_thread_ids, self.parent_thread_id.as_deref())?;
        let subagent_kind = Self::consistent_value(&subagent_kinds)?;
        if !matches!(subagent_kind.as_str(), "guardian" | "review") {
            return None;
        }
        Self::consistent_value(&parent_thread_ids)
    }

    /// 设置 / 合并 client metadata。
    pub fn set_client_metadata(&mut self, client_metadata: Option<Value>) {
        match client_metadata {
            Some(value) => {
                self.body.insert("client_metadata".to_string(), value);
            }
            None => {
                self.body.remove("client_metadata");
            }
        }
    }

    fn append_turn_metadata(values: &mut Vec<Value>, raw: Option<&str>) -> Option<()> {
        let Some(raw) = raw else {
            return Some(());
        };
        let value: Value = serde_json::from_str(raw).ok()?;
        value.as_object()?;
        values.push(value);
        Some(())
    }

    fn append_body_turn_metadata(values: &mut Vec<Value>, raw: &Value) -> Option<()> {
        let value = match raw {
            Value::String(raw) => serde_json::from_str(raw).ok()?,
            Value::Object(_) => raw.clone(),
            _ => return None,
        };
        value.as_object()?;
        values.push(value);
        Some(())
    }

    fn append_required_string(values: &mut Vec<String>, value: Option<&Value>) -> Option<()> {
        let Some(value) = value else {
            return Some(());
        };
        Self::append_optional_string(values, Some(value.as_str()?))
    }

    fn append_optional_string(values: &mut Vec<String>, value: Option<&str>) -> Option<()> {
        let Some(value) = value else {
            return Some(());
        };
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        values.push(value.to_owned());
        Some(())
    }

    fn consistent_value(values: &[String]) -> Option<String> {
        let value = values.first()?.clone();
        values
            .iter()
            .all(|candidate| candidate == &value)
            .then_some(value)
    }

    /// 替换客户端原本提供的账号身份字段；无法安全重建时删除该字段。
    pub fn replace_existing_identity_field(&mut self, key: &str, value: Option<&str>) {
        if !self.body.contains_key(key) {
            return;
        }
        match value {
            Some(value) => {
                self.body
                    .insert(key.to_string(), Value::String(value.to_string()));
            }
            None => {
                self.body.remove(key);
            }
        }
    }
}
