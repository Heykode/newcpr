//! Unified account-scoped QX application identity, independent from UA syntax.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{CodexClientResult, CodexRequestContext, protocol::responses::CodexResponsesRequest};

/// Only original opaque anchors are saved, never a transcript or authentication material.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CodexIdentitySeed {
    session: String,
    thread: String,
}

pub(crate) fn identity_seed(
    request: &CodexResponsesRequest,
    request_id: &str,
) -> CodexIdentitySeed {
    let session = [
        request.client_session_id.as_deref(),
        request.client_conversation_id.as_deref(),
        request.prompt_cache_key(),
        request
            .client_metadata()
            .and_then(|value| value.get("session_id"))
            .and_then(Value::as_str),
    ]
    .into_iter()
    .find_map(nonblank)
    .map(str::to_owned)
    .or_else(|| super::request::derive_stable_conversation_key(request))
    .unwrap_or_else(|| format!("request:{request_id}"));
    let thread = [
        request.client_thread_id.as_deref(),
        request.client_conversation_id.as_deref(),
    ]
    .into_iter()
    .find_map(nonblank)
    .unwrap_or(&session)
    .to_owned();
    CodexIdentitySeed { session, thread }
}

pub(crate) fn has_explicit_identity(request: &CodexResponsesRequest) -> bool {
    [
        request.client_session_id.as_deref(),
        request.client_conversation_id.as_deref(),
        request.client_thread_id.as_deref(),
        request.prompt_cache_key(),
    ]
    .into_iter()
    .any(|value| nonblank(value).is_some())
}

struct WireIdentity {
    session: String,
    thread: String,
    window: String,
    parent: Option<String>,
}

fn wire_identity(
    request: &CodexResponsesRequest,
    context: CodexRequestContext<'_>,
) -> Option<WireIdentity> {
    let key = nonblank(request.client_api_key_id.as_deref())?;
    // The durable per-principal installation is independent from token rotation and local row IDs.
    let installation = nonblank(context.installation_id)?;
    let seed = request
        .identity_seed
        .clone()
        .unwrap_or_else(|| identity_seed(request, context.request_id));
    let account = context.account_id.unwrap_or_default();
    let derive = |kind: &str, anchor: &str| {
        let mut hash = Sha256::new();
        for part in [
            "cpr-qx-account-identity-v1",
            kind,
            key,
            installation,
            account,
            anchor,
        ] {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part.as_bytes());
        }
        let mut bytes: [u8; 16] = hash.finalize()[..16].try_into().expect("SHA-256 prefix");
        // Deterministic UUID-shaped identifier, not a timestamp or upstream-issued credential.
        bytes[6] = (bytes[6] & 0x0f) | 0x70;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        uuid::Uuid::from_bytes(bytes).to_string()
    };
    let session = derive("session", &seed.session);
    let thread = derive("thread", &seed.thread);
    let generation = [context.codex_window_id, request.codex_window_id.as_deref()]
        .into_iter()
        .flatten()
        .find_map(window_generation)
        .unwrap_or(0);
    let parent = [
        context.parent_thread_id,
        request.parent_thread_id.as_deref(),
    ]
    .into_iter()
    .find_map(nonblank)
    .map(|parent| derive("thread", parent));
    Some(WireIdentity {
        session,
        window: format!("{thread}:{generation}"),
        thread,
        parent,
    })
}

/// Project only the outbound copy; account selection and continuation anchors remain original.
pub fn project_response_request(
    request: &CodexResponsesRequest,
    context: CodexRequestContext<'_>,
) -> CodexResponsesRequest {
    let mut projected = request.clone();
    projected
        .identity_seed
        .get_or_insert_with(|| identity_seed(request, context.request_id));
    project_response_body(projected.body_mut(), request, context);
    projected
}

pub(crate) fn project_response_body(
    body: &mut Map<String, Value>,
    request: &CodexResponsesRequest,
    context: CodexRequestContext<'_>,
) {
    let Some(ids) = wire_identity(request, context) else {
        return;
    };
    body.insert(
        "prompt_cache_key".to_owned(),
        Value::String(ids.thread.clone()),
    );
    project_identity_fields(body, &ids);
    if let Some(metadata) = body
        .get_mut("client_metadata")
        .and_then(Value::as_object_mut)
    {
        project_identity_fields(metadata, &ids);
    }
}

fn project_identity_fields(fields: &mut Map<String, Value>, ids: &WireIdentity) {
    for (names, value) in [
        (
            &["session_id", "session-id", "sessionId"][..],
            Some(ids.session.as_str()),
        ),
        (
            &[
                "thread_id",
                "thread-id",
                "threadId",
                "conversation_id",
                "conversationId",
            ][..],
            Some(ids.thread.as_str()),
        ),
        (
            &["x-client-request-id", "client_request_id"][..],
            Some(ids.thread.as_str()),
        ),
        (
            &["x-codex-window-id", "window_id"][..],
            Some(ids.window.as_str()),
        ),
        (
            &["x-codex-parent-thread-id", "parent_thread_id"][..],
            ids.parent.as_deref(),
        ),
    ] {
        for name in names {
            if fields.contains_key(*name) {
                if let Some(value) = value {
                    fields.insert((*name).to_owned(), Value::String(value.to_owned()));
                } else {
                    fields.remove(*name);
                }
            }
        }
    }
    for key in ["turn_metadata", "turnMetadata", "x-codex-turn-metadata"] {
        if let Some(raw) = fields.get(key).and_then(Value::as_str)
            && let Ok(Value::Object(mut metadata)) = serde_json::from_str::<Value>(raw)
        {
            // Embedded turn metadata is a flat identity projection, not arbitrary recursive content.
            for (name, value) in [
                ("session_id", ids.session.as_str()),
                ("thread_id", ids.thread.as_str()),
                ("window_id", ids.window.as_str()),
                ("x-client-request-id", ids.thread.as_str()),
            ] {
                if metadata.contains_key(name) {
                    metadata.insert(name.to_owned(), Value::String(value.to_owned()));
                }
            }
            if let Some(raw) = super::request::encode_turn_metadata(&metadata) {
                fields.insert(key.to_owned(), Value::String(raw));
            }
        }
    }
}

fn nonblank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Apply after passthrough using the frozen response identity.
///
/// Account/key scoped identities require authority from the Provider. Low-level calls without
/// that authority retain the original header projection and cannot invent an account identity.
pub fn apply_response_headers(
    headers: &mut HeaderMap,
    request: &CodexResponsesRequest,
    context: CodexRequestContext<'_>,
) -> CodexClientResult<()> {
    // Installation identity belongs in account-scoped body metadata, not a standalone header.
    headers.remove("x-codex-installation-id");
    let conversation = protocol_value(request.client_conversation_id.as_deref());
    let ids = wire_identity(request, context);
    let session = ids
        .as_ref()
        .map(|ids| HeaderValue::from_str(&ids.session).expect("UUID header"))
        .or_else(|| {
            [
                context.session_id,
                request.client_session_id.as_deref(),
                request.client_conversation_id.as_deref(),
            ]
            .into_iter()
            .find_map(protocol_value)
        });
    let thread = ids
        .as_ref()
        .map(|ids| HeaderValue::from_str(&ids.thread).expect("UUID header"))
        .or_else(|| {
            [
                context.thread_id,
                request.client_thread_id.as_deref(),
                request.client_conversation_id.as_deref(),
            ]
            .into_iter()
            .find_map(protocol_value)
            .or_else(|| session.clone())
        });
    let request_id = thread.clone().or_else(|| {
        [
            context.client_request_id,
            request.client_request_id.as_deref(),
            Some(context.request_id),
        ]
        .into_iter()
        .find_map(protocol_value)
    });
    let window = thread
        .as_ref()
        .and_then(|thread| thread.to_str().ok())
        .map(|thread| {
            let generation = [context.codex_window_id, request.codex_window_id.as_deref()]
                .into_iter()
                .flatten()
                .find_map(window_generation)
                .unwrap_or(0);
            HeaderValue::from_str(&format!("{thread}:{generation}"))
        })
        .transpose()?;
    for (name, value) in [
        ("session-id", session.clone()),
        ("session_id", session),
        ("thread-id", thread.clone()),
        ("x-client-request-id", request_id),
        ("conversation_id", conversation.and(thread)),
        ("x-codex-window-id", window),
    ] {
        headers.remove(name);
        if let Some(value) = value {
            headers.insert(HeaderName::from_static(name), value);
        }
    }
    if let Some(ids) = ids {
        headers.remove("x-codex-parent-thread-id");
        if let Some(parent) = &ids.parent {
            headers.insert("x-codex-parent-thread-id", HeaderValue::from_str(parent)?);
        }
        // Do not leave original session aliases inside a JSON turn-metadata header.
        if let Some(raw) = headers
            .get("x-codex-turn-metadata")
            .and_then(|value| value.to_str().ok())
            && let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(raw)
        {
            project_identity_fields(&mut fields, &ids);
            if let Some(raw) = super::request::encode_turn_metadata(&fields) {
                headers.insert("x-codex-turn-metadata", HeaderValue::from_str(&raw)?);
            }
        }
    }
    if !headers.contains_key("x-codex-beta-features") {
        let features = [context.beta_features, request.beta_features.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|value| HeaderValue::from_str(value).ok())
            .unwrap_or_else(|| HeaderValue::from_static("remote_compaction_v2"));
        headers.insert(HeaderName::from_static("x-codex-beta-features"), features);
    }
    Ok(())
}

fn protocol_value(value: Option<&str>) -> Option<HeaderValue> {
    let value = value.filter(|value| !value.trim().is_empty())?;
    let header = HeaderValue::from_str(value).ok()?;
    header.to_str().ok()?;
    Some(header)
}

fn window_generation(value: &str) -> Option<u64> {
    let (_, generation) = value.trim().rsplit_once(':')?;
    generation.trim().parse().ok()
}
