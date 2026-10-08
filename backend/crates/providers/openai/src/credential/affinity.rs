//! OpenAI 会话及子线程到 Store 不透明账号亲和键及诊断上下文的单向派生。

use std::time::Duration;

use gateway_core::account::AccountAffinity;
use gateway_core::operation::RawJsonPayload;
use gateway_core::policy::ClientApiKeyId;
use gateway_core::provider_ports::{ProviderSessionAffinityKey, ProviderSessionAlias};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::transport::protocol::responses::CodexResponsesRequest;
use crate::transport::request::derive_conversation_anchor;

const AFFINITY_KEY_HASH_LENGTH: usize = 12;
pub(crate) const CODEX_ROOT_SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// 一次请求派生出的账号亲和键及其结构化日志上下文。
#[derive(Clone)]
pub(crate) struct CodexSessionAffinity {
    mode: AccountAffinity,
    resolved_alias: Option<ProviderSessionAlias>,
    key: Option<ProviderSessionAffinityKey>,
    root_key: Option<ProviderSessionAffinityKey>,
    migration_key: Option<ProviderSessionAffinityKey>,
    guardian_parent_preference_key: Option<ProviderSessionAffinityKey>,
    guardian_parent_record_key: Option<ProviderSessionAffinityKey>,
    turn_alias_key: Option<ProviderSessionAffinityKey>,
    turn_alias_required: bool,
    follow_only: bool,
    key_hash: String,
    anchor_source: &'static str,
    anchor: String,
    session_id: Option<String>,
}

impl CodexSessionAffinity {
    fn turn_alias_only(turn_alias_key: ProviderSessionAffinityKey) -> Self {
        Self {
            mode: AccountAffinity::Strict,
            resolved_alias: None,
            key: None,
            root_key: None,
            migration_key: None,
            guardian_parent_preference_key: None,
            guardian_parent_record_key: None,
            key_hash: short_key_hash(&turn_alias_key),
            turn_alias_key: Some(turn_alias_key),
            turn_alias_required: true,
            follow_only: false,
            anchor_source: "turn-session",
            anchor: String::new(),
            session_id: None,
        }
    }

    pub(crate) fn with_resolved_turn_alias(mut self, alias: &ProviderSessionAlias) -> Self {
        if self.binding_key().is_none() {
            self.key = Some(alias.session_key().clone());
            self.key_hash = short_key_hash(alias.session_key());
        }
        self.resolved_alias = Some(alias.clone());
        self
    }

    pub(crate) fn with_policy(mut self, mode: AccountAffinity) -> Self {
        self.mode = mode;
        self
    }

    fn alias_binding_key<'a>(
        &self,
        alias: &'a ProviderSessionAlias,
    ) -> &'a ProviderSessionAffinityKey {
        if self.mode != AccountAffinity::Relaxed {
            alias.root_session_key().unwrap_or(alias.session_key())
        } else {
            alias.session_key()
        }
    }

    pub(crate) fn accepts_turn_alias(&self, alias: &ProviderSessionAlias) -> bool {
        if self
            .root_key
            .as_ref()
            .zip(alias.root_session_key())
            .is_some_and(|(root, alias_root)| root != alias_root)
        {
            return false;
        }
        self.binding_key().is_none_or(|key| key == self.alias_binding_key(alias))
            // Some endpoint calls carry only the root plus a child turn ID.
            || (self.root_key.is_none() && self.key.as_ref().is_some_and(|key| Some(key) == alias.root_session_key()))
            // Legacy strict child aliases have only a root target. Keep that turn
            // on its original root even when new child turns use relaxed affinity.
            || (alias.follow_only()
                && alias.root_session_key().is_none()
                && self.root_key.as_ref() == Some(alias.session_key()))
    }

    pub(crate) fn alias_record(&self) -> Option<ProviderSessionAlias> {
        self.resolved_alias.clone().or_else(|| {
            let root = (self.mode == AccountAffinity::Relaxed)
                .then(|| self.root_key.clone())
                .flatten();
            Some(
                ProviderSessionAlias::new(self.binding_key()?.clone(), self.follow_only())
                    .with_root_session_key(root),
            )
        })
    }

    #[must_use]
    pub(crate) fn key(&self) -> Option<&ProviderSessionAffinityKey> {
        self.binding_key()
    }

    pub(crate) fn transport_key(&self) -> Option<&ProviderSessionAffinityKey> {
        self.key.as_ref()
    }

    pub(crate) fn root_key(&self) -> Option<&ProviderSessionAffinityKey> {
        self.resolved_alias
            .as_ref()
            .and_then(ProviderSessionAlias::root_session_key)
            .or(self.root_key.as_ref())
    }

    pub(crate) fn turn_alias_key(&self) -> Option<&ProviderSessionAffinityKey> {
        self.turn_alias_key.as_ref()
    }

    pub(crate) fn with_advisory_turn_alias(mut self) -> Self {
        self.turn_alias_required = false;
        self
    }

    pub(crate) const fn turn_alias_required(&self) -> bool {
        self.turn_alias_required
    }

    pub(crate) fn follow_only(&self) -> bool {
        self.resolved_alias.as_ref().is_some_and(|alias| {
            (self.mode != AccountAffinity::Preferred && alias.follow_only())
                || (self.mode == AccountAffinity::Strict && alias.root_session_key().is_some())
        }) || (self.mode == AccountAffinity::Strict && self.follow_only)
    }

    pub(crate) fn binding_key(&self) -> Option<&ProviderSessionAffinityKey> {
        if let Some(alias) = self.resolved_alias.as_ref() {
            Some(self.alias_binding_key(alias))
        } else if self.mode != AccountAffinity::Relaxed {
            self.root_key.as_ref().or(self.key.as_ref())
        } else {
            self.key.as_ref().or(self.root_key.as_ref())
        }
    }

    pub(crate) fn migration_key(&self) -> Option<&ProviderSessionAffinityKey> {
        self.migration_key.as_ref()
    }

    pub(crate) fn guardian_parent_preference_key(&self) -> Option<&ProviderSessionAffinityKey> {
        self.guardian_parent_preference_key.as_ref()
    }

    pub(crate) fn guardian_parent_record_key(&self) -> Option<&ProviderSessionAffinityKey> {
        self.guardian_parent_record_key.as_ref()
    }

    /// Keep both identities; the frozen policy selects the account-binding key.
    fn with_thread(mut self, thread_id: Option<&str>) -> Option<Self> {
        if let Some(thread_id) = non_empty(thread_id)
            && self
                .session_id
                .as_deref()
                .is_some_and(|root| root != thread_id)
        {
            let parent_key = self.key.as_ref()?;
            let child_key = opaque_affinity_key(
                "child-thread",
                &format!("{}\0{thread_id}", parent_key.expose_to_store()),
            )?;
            self.root_key = Some(self.key.replace(child_key)?);
            self.key_hash = short_key_hash(self.key.as_ref()?);
            self.follow_only = true;
            self.anchor_source = "child-thread";
            self.anchor = thread_id.to_owned();
        }
        Some(self)
    }

    fn with_turn_alias(mut self, turn_alias_key: Option<ProviderSessionAffinityKey>) -> Self {
        self.turn_alias_key = turn_alias_key;
        self
    }

    #[must_use]
    pub(crate) fn key_hash(&self) -> &str {
        &self.key_hash
    }

    /// 返回可持久化的客户端作用域不透明会话关联值。
    #[must_use]
    pub(crate) fn persistence_hash(&self) -> Option<&str> {
        self.key().map(ProviderSessionAffinityKey::expose_to_store)
    }

    #[must_use]
    pub(crate) const fn anchor_source(&self) -> &'static str {
        self.anchor_source
    }

    #[must_use]
    pub(crate) fn anchor(&self) -> &str {
        &self.anchor
    }

    #[must_use]
    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    #[must_use]
    pub(crate) const fn session_id_present(&self) -> bool {
        self.session_id.is_some()
    }

    #[must_use]
    pub(crate) fn into_key(self) -> Option<ProviderSessionAffinityKey> {
        self.binding_key().cloned()
    }
}

/// 将原始 response ID 投影为客户端作用域的不可逆关联值。
#[must_use]
pub(crate) fn derive_previous_response_id_hash(
    previous_response_id: &str,
    client_api_key_id: &ClientApiKeyId,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-previous-response-observation-v1\0");
    hasher.update(client_api_key_id.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(previous_response_id.as_bytes());
    hex::encode(hasher.finalize())
}

pub(crate) fn derive_codex_session_affinity(
    request: &CodexResponsesRequest,
    client_api_key_id: &ClientApiKeyId,
) -> Option<CodexSessionAffinity> {
    let session_id = non_empty(request.client_session_id.as_deref()).map(str::to_owned);
    let guardian_parent_thread_id = request.guardian_parent_thread_id();
    let guardian_request = guardian_parent_thread_id.is_some();
    let guardian_parent_preference_key = guardian_parent_thread_id
        .as_deref()
        .and_then(|parent| guardian_parent_affinity_key(parent, client_api_key_id));
    let guardian_parent_record_key = (!guardian_request)
        .then(|| non_empty(request.client_thread_id.as_deref()))
        .flatten()
        .and_then(|thread_id| guardian_parent_affinity_key(thread_id, client_api_key_id));
    let legacy = derive_account_affinity_anchor(request).and_then(|(source, anchor)| {
        session_affinity(source, anchor, session_id, client_api_key_id)?
            .with_thread(request.client_thread_id.as_deref())
    });
    let turn_alias_key = request
        .client_turn_id
        .as_deref()
        .and_then(|turn_id| derive_turn_alias(turn_id, client_api_key_id));
    let legacy = legacy.map(|affinity| {
        affinity
            .with_guardian(
                guardian_parent_preference_key.clone(),
                guardian_parent_record_key.clone(),
            )
            .with_turn_alias(turn_alias_key.clone())
    });
    let guardian_only = || {
        guardian_parent_preference_key
            .clone()
            .zip(guardian_parent_thread_id.clone())
            .map(|(preference_key, parent_thread_id)| {
                guardian_only_affinity(preference_key, parent_thread_id)
                    .with_turn_alias(turn_alias_key.clone())
            })
    };
    if non_empty(request.client_session_id.as_deref()).is_some()
        || non_empty(request.client_conversation_id.as_deref()).is_some()
        || non_empty(request.client_thread_id.as_deref()).is_some()
        || (request.explicit_prompt_cache_key && non_empty(request.prompt_cache_key()).is_some())
        || request.previous_response_id().is_some()
    {
        return legacy.or_else(guardian_only);
    }
    let Some(hint) = request.scheduling_session_hint.as_ref() else {
        return legacy.or_else(guardian_only);
    };
    let mut affinity = scheduling_hint_affinity(hint, client_api_key_id)?;
    affinity.migration_key = legacy.and_then(CodexSessionAffinity::into_key);
    affinity = affinity.with_guardian(guardian_parent_preference_key, guardian_parent_record_key);
    affinity = affinity.with_turn_alias(turn_alias_key);
    Some(affinity)
}

/// 原始 JSON 端点只读取会话身份，发送时仍保留原始字节。Search 的 `id` 是官方
/// 根 session_id，必须与 Responses 共用命名空间，不能另建一份账号亲和。
pub(crate) fn derive_codex_endpoint_session_affinity(
    payload: &RawJsonPayload,
    client_api_key_id: &ClientApiKeyId,
    body_session_field: &str,
) -> Option<CodexSessionAffinity> {
    let body = serde_json::from_slice::<Map<String, Value>>(payload.body()).unwrap_or_default();
    let turn_alias_key = endpoint_turn_id(&body, payload.context())
        .and_then(|turn_id| derive_turn_alias(&turn_id, client_api_key_id));
    endpoint_explicit_session_affinity(
        &body,
        payload.context(),
        client_api_key_id,
        body_session_field,
    )
    .map(|affinity| affinity.with_turn_alias(turn_alias_key.clone()))
    .or_else(|| {
        endpoint_scheduling_hint_affinity(&body, payload.context(), client_api_key_id)
            .map(|affinity| affinity.with_turn_alias(turn_alias_key.clone()))
    })
    .or_else(|| turn_alias_key.map(CodexSessionAffinity::turn_alias_only))
}

pub(crate) fn derive_codex_compact_session_affinity(
    payload: &RawJsonPayload,
    client_api_key_id: &ClientApiKeyId,
) -> Option<CodexSessionAffinity> {
    let body = serde_json::from_slice::<Map<String, Value>>(payload.body()).unwrap_or_default();
    endpoint_explicit_session_affinity(&body, payload.context(), client_api_key_id, "session_id")
        .or_else(|| {
            let cache_key = non_empty(body.get("prompt_cache_key").and_then(Value::as_str))?;
            session_affinity(
                "root-prompt-cache",
                cache_key.to_owned(),
                None,
                client_api_key_id,
            )
        })
        .or_else(|| endpoint_scheduling_hint_affinity(&body, payload.context(), client_api_key_id))
}

pub(crate) fn derive_live_session_affinity(
    request: &gateway_core::operation::ProviderHttpRequest,
    client_api_key_id: &ClientApiKeyId,
) -> Option<CodexSessionAffinity> {
    let mut context = Map::new();
    for (header, field) in [
        ("session-id", "session_id"),
        ("thread-id", "thread_id"),
        ("x-codex-turn-metadata", "turn_metadata"),
        ("x-client-turn-id", "turn_id"),
    ] {
        if let Some(value) = request
            .headers()
            .iter()
            .find(|item| item.name().eq_ignore_ascii_case(header))
            .and_then(|item| std::str::from_utf8(item.value()).ok())
        {
            context.insert(field.to_owned(), Value::String(value.to_owned()));
        }
    }
    // x-session-id identifies the realtime call, not the root conversation.
    let body = Map::new();
    let turn_alias_key = endpoint_turn_id(&body, &context)
        .and_then(|turn| derive_turn_alias(&turn, client_api_key_id));
    endpoint_explicit_session_affinity(&body, &context, client_api_key_id, "session_id")
        .map(|affinity| affinity.with_turn_alias(turn_alias_key.clone()))
        .or_else(|| turn_alias_key.map(CodexSessionAffinity::turn_alias_only))
}

fn endpoint_explicit_session_affinity(
    body: &Map<String, Value>,
    context: &Map<String, Value>,
    client_api_key_id: &ClientApiKeyId,
    body_session_field: &str,
) -> Option<CodexSessionAffinity> {
    let session_id = gateway_protocol::openai::codex_session_id(body, context).or_else(|| {
        non_empty(body.get(body_session_field).and_then(Value::as_str)).map(str::to_owned)
    })?;
    session_affinity(
        "root-session",
        session_id.clone(),
        Some(session_id),
        client_api_key_id,
    )?
    .with_thread(gateway_protocol::openai::codex_thread_id(body, context).as_deref())
}

fn endpoint_scheduling_hint_affinity(
    body: &Map<String, Value>,
    context: &Map<String, Value>,
    client_api_key_id: &ClientApiKeyId,
) -> Option<CodexSessionAffinity> {
    // A standalone thread did not establish endpoint affinity before this feature.
    if gateway_protocol::openai::codex_thread_id(body, context).is_some() {
        return None;
    }
    let hint = gateway_protocol::openai::OpenAiSchedulingSessionHint::from_context(context)?;
    scheduling_hint_affinity(&hint, client_api_key_id)
}

fn endpoint_turn_id(body: &Map<String, Value>, context: &Map<String, Value>) -> Option<String> {
    [
        body.get("image_turn_id"),
        body.get("turn_id"),
        context.get("image_turn_id"),
        context.get("turn_id"),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| non_empty(value.as_str()).map(str::to_owned))
    .or_else(|| {
        [
            body.get("turn_metadata"),
            body.get("turnMetadata"),
            context.get("turn_metadata"),
        ]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find_map(|raw| {
            serde_json::from_str::<Map<String, Value>>(raw)
                .ok()
                .and_then(|metadata| {
                    metadata
                        .get("turn_id")
                        .and_then(Value::as_str)
                        .and_then(|value| non_empty(Some(value)).map(str::to_owned))
                })
        })
    })
}

fn scheduling_hint_affinity(
    hint: &gateway_protocol::openai::OpenAiSchedulingSessionHint,
    client_api_key_id: &ClientApiKeyId,
) -> Option<CodexSessionAffinity> {
    let mut affinity = session_affinity(
        "supplementary-session",
        format!("{}\0{}", hint.source(), hint.id()),
        None,
        client_api_key_id,
    )?;
    // Local hints are not upstream identities and must not expose new raw IDs in logs.
    affinity.anchor = affinity.key_hash.clone();
    Some(affinity)
}

fn session_affinity(
    anchor_source: &'static str,
    anchor: String,
    session_id: Option<String>,
    client_api_key_id: &ClientApiKeyId,
) -> Option<CodexSessionAffinity> {
    let session_key = opaque_affinity_key(anchor_source, &anchor)?;
    let key = opaque_affinity_key(
        "client-session",
        &format!(
            "{}\0{}",
            client_api_key_id.as_str(),
            session_key.expose_to_store()
        ),
    )?;
    let key_hash = short_key_hash(&key);
    Some(CodexSessionAffinity {
        mode: AccountAffinity::Strict,
        resolved_alias: None,
        key: Some(key),
        root_key: None,
        migration_key: None,
        guardian_parent_preference_key: None,
        guardian_parent_record_key: None,
        turn_alias_key: None,
        turn_alias_required: true,
        follow_only: false,
        key_hash,
        anchor_source,
        anchor,
        session_id,
    })
}

impl CodexSessionAffinity {
    fn with_guardian(
        mut self,
        guardian_parent_preference_key: Option<ProviderSessionAffinityKey>,
        guardian_parent_record_key: Option<ProviderSessionAffinityKey>,
    ) -> Self {
        self.guardian_parent_preference_key = guardian_parent_preference_key;
        self.guardian_parent_record_key = guardian_parent_record_key;
        self
    }
}

fn guardian_only_affinity(
    preference_key: ProviderSessionAffinityKey,
    parent_thread_id: String,
) -> CodexSessionAffinity {
    CodexSessionAffinity {
        mode: AccountAffinity::Strict,
        resolved_alias: None,
        key: None,
        root_key: None,
        migration_key: None,
        guardian_parent_preference_key: Some(preference_key.clone()),
        guardian_parent_record_key: None,
        turn_alias_key: None,
        turn_alias_required: true,
        follow_only: false,
        key_hash: short_key_hash(&preference_key),
        anchor_source: "guardian-parent-thread",
        anchor: parent_thread_id,
        session_id: None,
    }
}

fn guardian_parent_affinity_key(
    parent_thread_id: &str,
    client_api_key_id: &ClientApiKeyId,
) -> Option<ProviderSessionAffinityKey> {
    opaque_affinity_key(
        "guardian-parent-thread",
        &format!("{}\0{parent_thread_id}", client_api_key_id.as_str()),
    )
}

fn derive_turn_alias(
    turn_id: &str,
    client_api_key_id: &ClientApiKeyId,
) -> Option<ProviderSessionAffinityKey> {
    let turn_id = non_empty(Some(turn_id))?;
    opaque_affinity_key(
        "client-turn",
        &format!("{}\0{turn_id}", client_api_key_id.as_str()),
    )
}

fn short_key_hash(key: &ProviderSessionAffinityKey) -> String {
    // 亲和键本身已经是 SHA-256；日志沿用 WebSocket 诊断的 12 位短哈希长度。
    key.expose_to_store()
        .chars()
        .take(AFFINITY_KEY_HASH_LENGTH)
        .collect()
}

/// 先确定根会话锚点；子线程仅派生独立的传输恢复键，账号绑定仍共用根键。
fn derive_account_affinity_anchor(
    request: &CodexResponsesRequest,
) -> Option<(&'static str, String)> {
    non_empty(request.client_session_id.as_deref())
        .map(|value| ("root-session", value.to_owned()))
        .or_else(|| {
            non_empty(request.client_conversation_id.as_deref())
                .map(|value| ("root-conversation", value.to_owned()))
        })
        .or_else(|| {
            request
                .explicit_prompt_cache_key
                .then(|| request.prompt_cache_key())
                .flatten()
                .and_then(|value| non_empty(Some(value)))
                .map(|value| ("root-prompt-cache", value.to_owned()))
        })
        .or_else(|| {
            non_empty(request.local_conversation_id.as_deref())
                .map(|value| ("local-conversation", value.to_owned()))
        })
        .or_else(|| derive_conversation_anchor(request))
}

fn opaque_affinity_key(domain: &str, value: &str) -> Option<ProviderSessionAffinityKey> {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-session-affinity-v1\0");
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    hasher.update(value.as_bytes());
    ProviderSessionAffinityKey::try_new(hex::encode(hasher.finalize())).ok()
}

/// 恢复旧版 `cyber_policy` 的会话隔离键。
///
/// 它只接受显式 session/conversation 或客户端明确给出的 prompt cache key，避免将
/// 请求内容哈希误当成长会话；`previous_response_id` 续写不参与该策略。
pub(crate) fn derive_codex_cyber_policy_session_key(
    request: &CodexResponsesRequest,
    client_api_key_id: &ClientApiKeyId,
) -> Option<ProviderSessionAffinityKey> {
    if request.previous_response_id().is_some() {
        return None;
    }
    let session_id = non_empty(request.client_session_id.as_deref())
        .or_else(|| non_empty(request.client_conversation_id.as_deref()))
        .or_else(|| {
            request
                .explicit_prompt_cache_key
                .then(|| request.prompt_cache_key())
                .flatten()
                .and_then(|value| non_empty(Some(value)))
        })?;
    let mut hasher = Sha256::new();
    hasher.update(b"cyber-policy-session\0");
    hasher.update(client_api_key_id.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(session_id.as_bytes());
    ProviderSessionAffinityKey::try_new(hex::encode(hasher.finalize())).ok()
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
