//! 核心 Generate operation 到 Codex Responses wire request 的严格编码。

mod item_ids;
mod raw_json;

pub use raw_json::scope_raw_json_to_account;
pub(crate) use raw_json::scope_raw_turn_metadata;

use std::io;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use gateway_core::operation::GenerateRequest;
use gateway_protocol::openai::{
    WS_REQUEST_HEADER_RESPONSES_LITE_CLIENT_METADATA_KEY, is_transport_managed_request_header,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use roxmltree::Document;
use serde::Serialize as _;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::transport::profile::CodexRequestLocation;
use crate::transport::protocol::responses::{
    CodexResponsesRequest, X_CODEX_TURN_STATE_CLIENT_METADATA_KEY,
};

const PASSTHROUGH_HEADERS_CONTEXT_KEY: &str = "opaque_request_headers";
const TURN_ID_CLIENT_METADATA_KEY: &str = "turn_id";
const THREAD_SPAWN_SUBAGENT_KIND: &str = "thread_spawn";
const THREAD_SPAWN_CONVERSATION_PREFIX: &str = "thread-spawn:";
const ENVIRONMENT_CONTEXT_CONTENT_KIND: &str = "environments.environment_context";
const UNSUPPORTED_CODEX_RESPONSES_FIELDS: &[&str] = &[
    "max_output_tokens",
    "max_completion_tokens",
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
    "prompt_cache_retention",
];

const CROSS_ACCOUNT_IDENTITY_KEYS: &[&str] = &[
    "authorization",
    "Authorization",
    "cookie",
    "Cookie",
    "chatgpt-account-id",
    "chatgpt_account_id",
    "chatgptAccountId",
    "account_id",
    "accountId",
    "user_id",
    "userId",
    "chatgpt_user_id",
    "chatgptUserId",
    "access_token",
    "accessToken",
    "session_token",
    "sessionToken",
    "refresh_token",
    "refreshToken",
    "id_token",
    "idToken",
    "token",
    "cookies",
    "cookie_header",
    "cookieHeader",
    "cf_clearance",
];

const ACCOUNT_BOUND_STATE_KEYS: &[&str] = &[
    "turnState",
    "turn_state",
    "x-codex-turn-state",
    "previous_response_id",
    "previousResponseId",
    "response_id",
    "responseId",
    "conversation",
];

const TURN_METADATA_KEYS: &[&str] = &["turnMetadata", "turn_metadata", "x-codex-turn-metadata"];

const INSTALLATION_ID_KEYS: &[&str] = &[
    "installation_id",
    "installationId",
    "x-codex-installation-id",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestAccountScope {
    Same,
    Different,
    Unknown,
}

impl RequestAccountScope {
    pub(crate) const fn can_reuse_account_state(self) -> bool {
        matches!(self, Self::Same)
    }
}

/// Provider 专属编码错误；不保存 prompt、schema 或 option 值。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodexRequestEncodeError {
    #[error("Codex request is missing its OpenAI protocol payload")]
    InvalidProtocolPayload,
}

pub fn encode_generate_request(
    request: &GenerateRequest,
    upstream_model: &str,
    location: &CodexRequestLocation,
) -> Result<CodexResponsesRequest, CodexRequestEncodeError> {
    encode_generate_request_with_location(request, upstream_model, Some(location))
}

pub fn encode_generate_request_with_location(
    request: &GenerateRequest,
    upstream_model: &str,
    location: Option<&CodexRequestLocation>,
) -> Result<CodexResponsesRequest, CodexRequestEncodeError> {
    let payload = request.protocol_payload();
    if payload.protocol() != "openai" {
        return Err(CodexRequestEncodeError::InvalidProtocolPayload);
    }
    let mut encoded = encode_responses_body(payload.body().clone(), upstream_model, location);
    apply_protocol_context(&mut encoded, payload.context());
    Ok(encoded)
}

pub(crate) fn encode_responses_body(
    mut body: Map<String, Value>,
    upstream_model: &str,
    location: Option<&CodexRequestLocation>,
) -> CodexResponsesRequest {
    adapt_codex_responses_body(&mut body, upstream_model, location);
    let mut encoded = CodexResponsesRequest::from_body(body);
    encoded.explicit_prompt_cache_key = encoded.prompt_cache_key().is_some();
    extract_request_context(&mut encoded);
    encoded
}

fn adapt_codex_responses_body(
    body: &mut Map<String, Value>,
    upstream_model: &str,
    location: Option<&CodexRequestLocation>,
) {
    body.insert("model".to_owned(), Value::String(upstream_model.to_owned()));
    // Store is a provider capability, not just a wire default: session capture and
    // transport selection must observe the same value that Codex will receive.
    if body
        .get("store")
        .is_some_and(|value| value != &Value::Bool(false))
    {
        tracing::debug!(field = "store", "Codex Generate storage option normalized");
    }
    body.insert("store".to_owned(), Value::Bool(false));
    for field in UNSUPPORTED_CODEX_RESPONSES_FIELDS {
        if body.remove(*field).is_some() {
            tracing::debug!(field = *field, "Unsupported Codex Generate option omitted");
        }
    }
    if let Some(location) = location {
        align_structured_location_fields(body, Utc::now(), location);
    }
}

pub(crate) fn align_structured_location_fields(
    body: &mut Map<String, Value>,
    now: DateTime<Utc>,
    location: &CodexRequestLocation,
) {
    // 只改写官方生成的环境上下文和 Web Search 地区；用户正文和 epoch 时间戳保持原样。
    let current_date = now
        .with_timezone(&location.timezone)
        .format("%Y-%m-%d")
        .to_string();
    if let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) {
        for item in input {
            align_environment_context(item, &current_date, location.timezone.name());
        }
    }
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            align_web_search_location(tool, location);
        }
    }
}

fn align_environment_context(item: &mut Value, current_date: &str, timezone: &str) {
    let Some(item) = item.as_object_mut() else {
        return;
    };
    if item.get("role").and_then(Value::as_str) != Some("user") {
        return;
    }
    let content_kinds = item
        .get("internal_chat_message_metadata_passthrough")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("content_item_kinds"))
        .and_then(Value::as_array)
        .map(|kinds| kinds.iter().filter_map(Value::as_str).collect::<Vec<_>>());
    if !content_kinds
        .as_deref()
        .is_some_and(|kinds| kinds.contains(&ENVIRONMENT_CONTEXT_CONTENT_KIND))
    {
        return;
    }
    let Some(parts) = item.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    for part in parts {
        let Some(part) = part.as_object_mut() else {
            continue;
        };
        if part.get("type").and_then(Value::as_str) != Some("input_text") {
            continue;
        }
        let Some(Value::String(text)) = part.get_mut("text") else {
            continue;
        };
        if let Some(aligned) = aligned_environment_context(text, current_date, timezone) {
            *text = aligned;
        }
    }
}

fn aligned_environment_context(text: &str, current_date: &str, timezone: &str) -> Option<String> {
    let trimmed = text.trim();
    if !trimmed.starts_with("<environment_context>") || !trimmed.ends_with("</environment_context>")
    {
        return None;
    }
    let document = Document::parse(text).ok()?;
    let root = document.root_element();
    if !root.has_tag_name("environment_context") {
        return None;
    }
    let mut replacements = root
        .children()
        .filter(|node| node.is_element())
        .filter_map(|node| {
            let replacement = match node.tag_name().name() {
                "current_date" => format!("<current_date>{current_date}</current_date>"),
                "timezone" => format!("<timezone>{timezone}</timezone>"),
                _ => return None,
            };
            Some((node.range(), replacement))
        })
        .collect::<Vec<_>>();
    if replacements.is_empty() {
        return None;
    }
    replacements.sort_unstable_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut aligned = text.to_owned();
    for (range, replacement) in replacements {
        aligned.replace_range(range, &replacement);
    }
    Some(aligned)
}

fn align_web_search_location(tool: &mut Value, location: &CodexRequestLocation) {
    let Some(tool) = tool.as_object_mut() else {
        return;
    };
    let Some(tool_type) = tool.get("type").and_then(Value::as_str) else {
        return;
    };
    if tool_type != "web_search" && !tool_type.starts_with("web_search_") {
        return;
    }
    tool.insert(
        "user_location".to_owned(),
        json!({
            "type": "approximate",
            "country": location.country,
            "region": location.region,
            "city": location.city,
            "timezone": location.timezone.name(),
        }),
    );
}

/// Normalize only the outbound copy, after account and session identity selection.
pub(crate) fn normalize_generate_upstream_body(body: &mut Map<String, Value>) {
    item_ids::normalize_standalone_item_ids(body);
    if let Some(input @ Value::String(_)) = body.get_mut("input") {
        let text = std::mem::take(input);
        *input = serde_json::json!([{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": text}]
        }]);
    }
    if let Some(Value::String(tier)) = body.get_mut("service_tier") {
        let upstream_tier = generate_upstream_service_tier(tier);
        if upstream_tier != tier.as_str() {
            *tier = upstream_tier.to_owned();
        }
    }
}

pub(crate) fn generate_upstream_service_tier(tier: &str) -> &str {
    match tier {
        "fast" => "priority",
        _ => tier,
    }
}

fn extract_request_context(request: &mut CodexResponsesRequest) {
    let context = ExtractedRequestContext::from_body(request.body());
    request.turn_state = context.turn_state;
    request.turn_metadata = context.turn_metadata;
    request.beta_features = context.beta_features;
    request.version = context.version;
    request.include_timing_metrics = context.include_timing_metrics;
    request.codex_window_id = context.codex_window_id;
    request.parent_thread_id = context.parent_thread_id;
    request.client_conversation_id = context.conversation_id;
    request.client_request_id = context.client_request_id;
    request.client_turn_id = context.turn_id;
    request.responses_lite = context.responses_lite;
    request.memgen_request = context.memgen_request;
}

struct ExtractedRequestContext {
    turn_state: Option<String>,
    turn_metadata: Option<String>,
    beta_features: Option<String>,
    version: Option<String>,
    include_timing_metrics: Option<String>,
    codex_window_id: Option<String>,
    parent_thread_id: Option<String>,
    conversation_id: Option<String>,
    client_request_id: Option<String>,
    turn_id: Option<String>,
    responses_lite: Option<String>,
    memgen_request: Option<String>,
}

impl ExtractedRequestContext {
    fn from_body(body: &Map<String, Value>) -> Self {
        let client_metadata = body.get("client_metadata").and_then(Value::as_object);
        Self {
            // 官方 downstream WebSocket 无法逐帧更新 HTTP header，因此把
            // response.metadata 返回的 turn state 放回下一帧 client_metadata。
            // Provider 仍会在账号/turn 归属确定后决定是否允许复用该状态。
            turn_state: body_string(body, "turnState").or_else(|| {
                client_metadata.and_then(|metadata| {
                    string_value(metadata.get(X_CODEX_TURN_STATE_CLIENT_METADATA_KEY))
                })
            }),
            turn_metadata: body_string(body, "turnMetadata"),
            beta_features: body_string(body, "betaFeatures"),
            version: body_string(body, "version"),
            include_timing_metrics: body_string(body, "includeTimingMetrics"),
            codex_window_id: body_string(body, "codexWindowId"),
            parent_thread_id: body_string(body, "parentThreadId"),
            conversation_id: body_string(body, "conversation_id"),
            client_request_id: body_string(body, "x-client-request-id"),
            turn_id: body_string(body, "turn_id").or_else(|| {
                client_metadata
                    .and_then(|metadata| string_value(metadata.get(TURN_ID_CLIENT_METADATA_KEY)))
            }),
            // Responses Lite 在 WebSocket body 中的这个键是官方 header 投影，
            // 不是普通上下文字段的 metadata 回退。
            responses_lite: client_metadata.and_then(|metadata| {
                string_value(metadata.get(WS_REQUEST_HEADER_RESPONSES_LITE_CLIENT_METADATA_KEY))
            }),
            // Memory consolidation 只由官方请求头提供；body 中同名字段不参与
            // transport 事实提取。
            memgen_request: None,
        }
    }
}

fn body_string(body: &Map<String, Value>, key: &str) -> Option<String> {
    string_value(body.get(key))
}

fn string_value(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(ToOwned::to_owned)
}

pub(crate) fn derive_conversation_anchor(
    request: &CodexResponsesRequest,
) -> Option<(&'static str, String)> {
    thread_spawn_conversation_anchor(request).or_else(|| {
        request
            .client_session_id
            .as_deref()
            .map(|value| ("session", value.to_owned()))
            .or_else(|| {
                request
                    .client_conversation_id
                    .as_deref()
                    .map(|value| ("conversation", value.to_owned()))
            })
            .or_else(|| {
                request
                    .client_thread_id
                    .as_deref()
                    .map(|value| ("thread", value.to_owned()))
            })
            .or_else(|| {
                request
                    .prompt_cache_key()
                    .map(|value| ("prompt-cache", value.to_owned()))
            })
            .or_else(|| derive_stable_conversation_key(request).map(|value| ("request", value)))
    })
}

/// `thread_spawn` 与父任务共享根会话，但它本身是独立的子任务执行。
/// 因此子线程必须拥有独立的 continuation/WebSocket 传输身份；账号亲和另行派生。
fn thread_spawn_conversation_anchor(
    request: &CodexResponsesRequest,
) -> Option<(&'static str, String)> {
    if request.subagent_kind().as_deref() != Some(THREAD_SPAWN_SUBAGENT_KIND) {
        return None;
    }

    request
        .client_thread_id
        .as_deref()
        .map(str::trim)
        .filter(|thread_id| !thread_id.is_empty())
        .map(|thread_id| {
            (
                "subagent-thread",
                format!("{THREAD_SPAWN_CONVERSATION_PREFIX}{thread_id}"),
            )
        })
}

const LEADING_SYSTEM_REMINDER_OPEN: &str = "<system-reminder>";
const LEADING_SYSTEM_REMINDER_CLOSE: &str = "</system-reminder>";

pub(crate) fn derive_stable_conversation_key(request: &CodexResponsesRequest) -> Option<String> {
    let instructions = request
        .instructions()
        .chars()
        .take(2_000)
        .collect::<String>();
    let content = first_user_content(request.body().get("input"));
    let first_user_text = content.map(first_user_text).unwrap_or_default();
    let normalized = normalize_conversation_anchor_text(&first_user_text);
    let first_user_text = if normalized.is_empty() {
        first_user_text
    } else {
        normalized
    };
    let nontext = content
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) != Some("input_text"))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if instructions.is_empty() && first_user_text.is_empty() && nontext.is_empty() {
        return None;
    }

    let mut hasher = Sha256::new();
    hasher.update(request.model().as_bytes());
    hasher.update(b"\0");
    hasher.update(instructions.as_bytes());
    hasher.update(b"\0");
    hasher.update(first_user_text.as_bytes());
    // Preserve established text-only keys, while distinguishing tools and non-text input.
    for (name, value) in [
        ("tools", request.body().get("tools").cloned()),
        ("functions", request.body().get("functions").cloned()),
        ("nontext", Some(Value::Array(nontext))),
    ] {
        if let Some(value) = value.filter(|value| {
            !value.is_null() && value.as_array().is_none_or(|array| !array.is_empty())
        }) {
            hasher.update(b"\0");
            hasher.update(name.as_bytes());
            hash_canonical_json(&mut hasher, &value);
        }
    }
    let digest = hex::encode(hasher.finalize());
    Some(format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    ))
}

fn first_user_content(input: Option<&Value>) -> Option<&Value> {
    let input = input?;
    if input.is_string() {
        return Some(input);
    }
    if input.get("role").and_then(Value::as_str) == Some("user") {
        return input.get("content");
    }
    input
        .as_array()?
        .iter()
        .find(|item| item.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|item| item.get("content"))
}

fn first_user_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("input_text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect()
}

fn hash_canonical_json(hasher: &mut Sha256, value: &Value) {
    match value {
        Value::Object(fields) => {
            hasher.update(b"{");
            let sorted: std::collections::BTreeMap<_, _> = fields.iter().collect();
            for (key, value) in sorted {
                hasher.update((key.len() as u64).to_be_bytes());
                hasher.update(key.as_bytes());
                hash_canonical_json(hasher, value);
            }
            hasher.update(b"}");
        }
        Value::Array(items) => {
            hasher.update(b"[");
            for item in items {
                hash_canonical_json(hasher, item);
            }
            hasher.update(b"]");
        }
        value => {
            let encoded = value.to_string();
            hasher.update((encoded.len() as u64).to_be_bytes());
            hasher.update(encoded.as_bytes());
        }
    }
}

fn normalize_conversation_anchor_text(text: &str) -> String {
    let mut rest = text.trim_start();
    loop {
        let lower = rest.to_ascii_lowercase();
        if !lower.starts_with(LEADING_SYSTEM_REMINDER_OPEN) {
            break;
        }
        let Some(close_start) = lower.find(LEADING_SYSTEM_REMINDER_CLOSE) else {
            break;
        };
        rest = rest[close_start + LEADING_SYSTEM_REMINDER_CLOSE.len()..].trim_start();
    }
    rest.to_owned()
}

/// Apply the same turn decision to every known mirror before account scoping and passthrough.
pub(crate) fn clear_turn_state(request: &mut CodexResponsesRequest) {
    request.turn_state = None;
    request.managed_turn_state_version = None;
    request.managed_turn_state_expires_at = None;
    request.passthrough_headers.remove("x-codex-turn-state");
    request.turn_metadata = request.turn_metadata.take().map(clear_embedded_turn_state);
    clear_turn_state_fields(request.body_mut());
    if let Some(metadata) = request
        .body_mut()
        .get_mut("client_metadata")
        .and_then(Value::as_object_mut)
    {
        clear_turn_state_fields(metadata);
    }
    if let Some(raw) = request
        .passthrough_headers
        .get("x-codex-turn-metadata")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        && let Ok(value) = HeaderValue::from_str(&clear_embedded_turn_state(raw))
    {
        request
            .passthrough_headers
            .insert("x-codex-turn-metadata", value);
    }
}

/// Apply a provider-managed state after account scoping has established ownership.
pub(crate) fn apply_managed_turn_state(
    request: &mut CodexResponsesRequest,
    state: String,
    version: u64,
    expires_at: std::time::SystemTime,
) {
    request.turn_state = Some(state.clone());
    request.managed_turn_state_version = Some(version);
    request.managed_turn_state_expires_at = Some(expires_at);
    request.passthrough_headers.remove("x-codex-turn-state");
    let mut metadata = request
        .client_metadata()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    metadata.insert("x-codex-turn-state".to_owned(), Value::String(state));
    request.set_client_metadata(Some(Value::Object(metadata)));
}

fn clear_turn_state_fields(fields: &mut Map<String, Value>) {
    for key in ["turnState", "turn_state", "x-codex-turn-state"] {
        fields.remove(key);
    }
    for key in TURN_METADATA_KEYS {
        if let Some(raw) = fields.get(*key).and_then(Value::as_str).map(str::to_owned) {
            fields.insert(
                (*key).to_owned(),
                Value::String(clear_embedded_turn_state(raw)),
            );
        }
    }
}

fn clear_embedded_turn_state(raw: String) -> String {
    let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(&raw) else {
        return raw;
    };
    for key in ["turnState", "turn_state", "x-codex-turn-state"] {
        fields.remove(key);
    }
    encode_turn_metadata(&fields).unwrap_or(raw)
}

/// 把客户端正文收敛到当前 lease 的账号身份边界。
///
/// 真实 account ID 由随后构造的 `CodexRequestContext` 注入请求头；installation ID
/// 统一写入 Core client_metadata，并替换原有兼容字段，绝不接受客户端
/// 提供的 token、cookie 或账号身份。`input` 是 Responses 的可回放会话正文，
/// item ID、encrypted content 和 compaction 都必须原样保留。
pub(crate) fn scope_request_to_account(
    request: &mut CodexResponsesRequest,
    installation_id: &str,
    account_scope: RequestAccountScope,
) {
    let reset_account_state = !account_scope.can_reuse_account_state();
    let client_metadata_turn_state = metadata_string(request, "x-codex-turn-state");
    let preserve_turn_state = !reset_account_state
        && (request.turn_state.is_some() || client_metadata_turn_state.is_some());
    let turn_state = preserve_turn_state
        .then(|| request.turn_state.clone())
        .flatten();
    let client_metadata_turn_state = if preserve_turn_state {
        client_metadata_turn_state
    } else {
        None
    };
    let turn_metadata = request
        .turn_metadata
        .as_deref()
        .and_then(|metadata| scope_turn_metadata(metadata, installation_id, reset_account_state));
    let client_metadata_turn_metadata = metadata_string(request, "x-codex-turn-metadata")
        .and_then(|metadata| scope_turn_metadata(&metadata, installation_id, reset_account_state));

    if reset_account_state {
        request.passthrough_headers.remove("x-codex-turn-state");
        request.passthrough_headers.remove("x-codex-turn-metadata");
        for key in CROSS_ACCOUNT_IDENTITY_KEYS
            .iter()
            .chain(ACCOUNT_BOUND_STATE_KEYS)
        {
            request.body_mut().remove(*key);
        }
    } else {
        // Opaque headers are applied last. Scope that mirror too, or it can
        // overwrite the selected device with a downstream installation alias.
        let values: Vec<_> = request
            .passthrough_headers
            .get_all("x-codex-turn-metadata")
            .iter()
            .map(|value| {
                value
                    .to_str()
                    .ok()
                    .and_then(|raw| scope_turn_metadata(raw, installation_id, false))
                    .and_then(|raw| HeaderValue::from_str(&raw).ok())
                    .unwrap_or_else(|| value.clone())
            })
            .collect();
        request.passthrough_headers.remove("x-codex-turn-metadata");
        for value in values {
            request
                .passthrough_headers
                .append("x-codex-turn-metadata", value);
        }
    }

    for key in INSTALLATION_ID_KEYS {
        request.replace_existing_identity_field(key, Some(installation_id));
    }
    for key in TURN_METADATA_KEYS {
        let scoped = request
            .body()
            .get(*key)
            .and_then(Value::as_str)
            .and_then(|value| scope_turn_metadata(value, installation_id, reset_account_state));
        replace_existing_body_string(request, key, scoped.as_deref());
    }

    let client_metadata = request
        .client_metadata()
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let scoped = match client_metadata {
        Value::Object(mut metadata) => {
            let scoped_turn_metadata = ["turnMetadata", "turn_metadata", "x-codex-turn-metadata"]
                .map(|key| {
                    (
                        key,
                        metadata.get(key).and_then(Value::as_str).and_then(|value| {
                            scope_turn_metadata(value, installation_id, reset_account_state)
                        }),
                    )
                });
            if reset_account_state {
                for key in CROSS_ACCOUNT_IDENTITY_KEYS
                    .iter()
                    .chain(ACCOUNT_BOUND_STATE_KEYS)
                {
                    metadata.remove(*key);
                }
            }
            metadata.insert(
                "installation_id".to_owned(),
                Value::String(installation_id.to_owned()),
            );
            metadata.insert(
                "x-codex-installation-id".to_owned(),
                Value::String(installation_id.to_owned()),
            );
            replace_existing_metadata_field(&mut metadata, "installationId", Some(installation_id));
            replace_metadata_field(
                &mut metadata,
                "x-codex-turn-state",
                client_metadata_turn_state.as_deref(),
            );
            replace_metadata_field(
                &mut metadata,
                "x-codex-turn-metadata",
                client_metadata_turn_metadata.as_deref(),
            );
            for (key, value) in scoped_turn_metadata {
                replace_existing_metadata_field(&mut metadata, key, value.as_deref());
            }
            (!metadata.is_empty()).then_some(Value::Object(metadata))
        }
        value => Some(value),
    };
    request.set_client_metadata(scoped);

    request.turn_state = turn_state;
    request.turn_metadata = turn_metadata;
}

fn metadata_string(request: &CodexResponsesRequest, key: &str) -> Option<String> {
    request
        .client_metadata()?
        .as_object()?
        .get(key)?
        .as_str()
        .map(ToOwned::to_owned)
}

pub(crate) fn scope_turn_metadata(
    raw: &str,
    installation_id: &str,
    cross_account: bool,
) -> Option<String> {
    let Ok(Value::Object(mut metadata)) = serde_json::from_str::<Value>(raw) else {
        return (!cross_account).then(|| raw.to_owned());
    };
    let mut changed = false;
    if cross_account {
        for key in CROSS_ACCOUNT_IDENTITY_KEYS
            .iter()
            .chain(ACCOUNT_BOUND_STATE_KEYS)
            .chain(TURN_METADATA_KEYS)
        {
            changed |= metadata.remove(*key).is_some();
        }
    }
    if raw.is_ascii()
        && !cross_account
        && !changed
        && !INSTALLATION_ID_KEYS
            .iter()
            .any(|key| metadata.contains_key(*key))
    {
        return Some(raw.to_owned());
    }
    for key in INSTALLATION_ID_KEYS {
        if metadata.contains_key(*key) {
            metadata.insert((*key).to_owned(), Value::String(installation_id.to_owned()));
        }
    }
    // Codex 的 turn metadata 同时承载于 HTTP header 与 WS client_metadata。
    // 改写安装 ID 后仍须保持官方 to_ascii_json_string 的编码合同；普通
    // to_string 会把中文工作区路径还原成 UTF-8，触发上游 WS metadata 后 Close 1000。
    encode_turn_metadata(&metadata)
}

pub(crate) fn encode_turn_metadata(metadata: &Map<String, Value>) -> Option<String> {
    let mut bytes = Vec::new();
    metadata
        .serialize(&mut serde_json::Serializer::with_formatter(
            &mut bytes,
            AsciiTurnMetadataFormatter,
        ))
        .ok()?;
    String::from_utf8(bytes).ok()
}

struct AsciiTurnMetadataFormatter;

impl serde_json::ser::Formatter for AsciiTurnMetadataFormatter {
    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        let mut start = 0;
        for (index, ch) in fragment.char_indices() {
            if ch.is_ascii() {
                continue;
            }
            writer.write_all(&fragment.as_bytes()[start..index])?;
            for unit in ch.encode_utf16(&mut [0; 2]) {
                write!(writer, "\\u{unit:04x}")?;
            }
            start = index + ch.len_utf8();
        }
        writer.write_all(&fragment.as_bytes()[start..])
    }
}

fn replace_existing_body_string(
    request: &mut CodexResponsesRequest,
    key: &str,
    value: Option<&str>,
) {
    if request.body().get(key).is_some_and(Value::is_string)
        && let Some(value) = value
    {
        request
            .body_mut()
            .insert(key.to_owned(), Value::String(value.to_owned()));
    }
}

fn replace_existing_metadata_field(
    metadata: &mut Map<String, Value>,
    key: &str,
    value: Option<&str>,
) {
    if metadata.contains_key(key)
        && let Some(value) = value
    {
        metadata.insert(key.to_owned(), Value::String(value.to_owned()));
    }
}

fn replace_metadata_field(metadata: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value
        && metadata.get(key).is_none_or(Value::is_string)
    {
        metadata.insert(key.to_owned(), Value::String(value.to_owned()));
    }
}

// 迁移契约是 header-authoritative:连接边界解析出的协议上下文(官方请求头)
// 优先;body 顶层别名只在 header 缺失时兜底(例如 WebSocket 帧无法逐轮携带
// 请求头的场景)。
fn apply_protocol_context(request: &mut CodexResponsesRequest, context: &Map<String, Value>) {
    request.scheduling_session_hint =
        gateway_protocol::openai::OpenAiSchedulingSessionHint::from_context(context);
    request.passthrough_headers = decode_passthrough_headers(context);
    request.turn_state =
        context_string(context, "turn_state").or_else(|| request.turn_state.take());
    request.turn_metadata =
        context_string(context, "turn_metadata").or_else(|| request.turn_metadata.take());
    request.beta_features =
        context_string(context, "beta_features").or_else(|| request.beta_features.take());
    request.version = context_string(context, "version").or_else(|| request.version.take());
    request.include_timing_metrics = context_string(context, "include_timing_metrics")
        .or_else(|| request.include_timing_metrics.take());
    request.codex_window_id =
        context_string(context, "codex_window_id").or_else(|| request.codex_window_id.take());
    request.downstream_websocket_connection_id =
        context_string(context, "downstream_websocket_connection_id")
            .or_else(|| request.downstream_websocket_connection_id.take());
    request.parent_thread_id =
        context_string(context, "parent_thread_id").or_else(|| request.parent_thread_id.take());
    let prompt_cache_key = request.prompt_cache_key().map(ToOwned::to_owned);
    request.client_conversation_id = context_string(context, "conversation_id")
        .or_else(|| request.client_conversation_id.take())
        .or(prompt_cache_key);
    request.client_session_id = gateway_protocol::openai::codex_session_id(request.body(), context);
    request.client_thread_id = gateway_protocol::openai::codex_thread_id(request.body(), context);
    request.client_request_id =
        context_string(context, "client_request_id").or_else(|| request.client_request_id.take());
    request.client_turn_id =
        context_string(context, "turn_id").or_else(|| request.client_turn_id.take());
    request.responses_lite =
        context_string(context, "responses_lite").or_else(|| request.responses_lite.take());
    request.memgen_request =
        context_string(context, "memgen_request").or_else(|| request.memgen_request.take());
    match context.get("use_websocket").and_then(Value::as_bool) {
        Some(true) => {
            request.use_websocket = true;
            request.force_http_sse = false;
        }
        Some(false) => {
            request.use_websocket = false;
            request.force_http_sse = true;
        }
        None => {}
    }
}

fn decode_passthrough_headers(context: &Map<String, Value>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let Some(entries) = context
        .get(PASSTHROUGH_HEADERS_CONTEXT_KEY)
        .and_then(Value::as_array)
    else {
        return headers;
    };

    for entry in entries {
        let Some(entry) = entry.as_array().filter(|entry| entry.len() == 2) else {
            continue;
        };
        let Some(name) = entry.first().and_then(Value::as_str) else {
            continue;
        };
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        if provider_managed_header(name.as_str()) {
            continue;
        }
        let Some(encoded) = entry.get(1).and_then(Value::as_str) else {
            continue;
        };
        let Ok(bytes) = STANDARD.decode(encoded) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_bytes(&bytes) else {
            continue;
        };
        headers.append(name, value);
    }
    headers
}

fn provider_managed_header(name: &str) -> bool {
    is_transport_managed_request_header(name)
        || name.starts_with("x-grok-")
        || name.starts_with("x-xai-")
        || name.starts_with("x-stainless-")
        || name.starts_with("sec-ch-ua")
        || name.starts_with("sec-fetch-")
        || matches!(
            name,
            "authorization"
                | "x-api-key"
                | "x-openai-actor-authorization"
                | "cookie"
                | "cookie2"
                | "chatgpt-account-id"
                | "chatgpt-organization-id"
                | "chatgpt-org-id"
                | "chatgpt-project-id"
                | "openai-organization"
                | "openai-project"
                | "x-openai-organization"
                | "x-openai-project"
                | "x-codex-installation-id"
                | "origin"
                | "referer"
                // The semantic alias is already decoded; the lease projects upstream IDs.
                | "session_id"
        )
}

fn context_string(context: &Map<String, Value>, field: &str) -> Option<String> {
    context
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}
