//! Outbound-only correlation mapping. Local scheduling and opaque response/call IDs stay original.

use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::BTreeMap};

const TRACE_FIELDS: &[&str] = &[
    "traceparent",
    "tracestate",
    "ws_request_header_traceparent",
    "ws_request_header_tracestate",
];

#[derive(Clone, Copy)]
pub(crate) struct AccountCorrelation<'a> {
    key: &'a str,
    installation: &'a str,
    account: &'a str,
    originals: Option<&'a BTreeMap<String, String>>,
    mapped: Option<&'a RefCell<BTreeMap<String, String>>>,
}

impl<'a> AccountCorrelation<'a> {
    pub(crate) fn new(key: &'a str, installation: &'a str, account: &'a str) -> Self {
        Self {
            key,
            installation,
            account,
            originals: None,
            mapped: None,
        }
    }

    pub(super) fn with_originals(mut self, originals: &'a BTreeMap<String, String>) -> Self {
        self.originals = Some(originals);
        self
    }

    pub(super) fn recording(mut self, mapped: &'a RefCell<BTreeMap<String, String>>) -> Self {
        self.mapped = Some(mapped);
        self
    }

    // Keep the established session/thread algorithm, including its domain and UUID shape.
    pub(super) fn derive(self, kind: &str, anchor: &str) -> String {
        let anchor = self
            .originals
            .and_then(|originals| originals.get(anchor))
            .map_or(anchor, String::as_str);
        let mut hash = Sha256::new();
        for part in [
            "cpr-qx-account-identity-v1",
            kind,
            self.key,
            self.installation,
            self.account,
            anchor,
        ] {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part.as_bytes());
        }
        let mut bytes: [u8; 16] = hash.finalize()[..16].try_into().expect("SHA-256 prefix");
        bytes[6] = (bytes[6] & 0x0f) | 0x70;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let mapped = uuid::Uuid::from_bytes(bytes).to_string();
        if let Some(originals) = self.mapped {
            originals
                .borrow_mut()
                .insert(mapped.clone(), anchor.to_owned());
        }
        mapped
    }

    pub(crate) fn field(self, name: &str, value: &str) -> Option<String> {
        if value.is_empty() {
            return None;
        }
        let kind = match name {
            "session_id" | "session-id" | "sessionId" => "session",
            "thread_id"
            | "thread-id"
            | "threadId"
            | "conversation_id"
            | "conversationId"
            | "parent_thread_id"
            | "parentThreadId"
            | "x-codex-parent-thread-id"
            | "forked_from_thread_id"
            | "forkedFromThreadId"
            | "root_thread_id"
            | "rootThreadId"
            | "x-client-request-id"
            | "client_request_id"
            | "prompt_cache_key" => "thread",
            "window_id" | "context_window_id" | "contextWindowId" | "codexWindowId"
            | "x-codex-window-id" => {
                let (thread, generation) = value
                    .rsplit_once(':')
                    .and_then(|(thread, generation)| {
                        generation
                            .parse::<u64>()
                            .ok()
                            .map(|generation| (thread, generation))
                    })
                    .unwrap_or((value, 0));
                return Some(format!("{}:{generation}", self.derive("thread", thread)));
            }
            _ => return None,
        };
        Some(self.derive(kind, value))
    }

    pub(crate) fn is_trace(name: &str) -> bool {
        TRACE_FIELDS.contains(&name)
    }

    pub(crate) fn scope_fields(self, object: &mut Map<String, Value>) {
        object.retain(|key, _| !Self::is_trace(key));
        for (key, value) in object {
            if let Value::String(raw) = value
                && let Some(scoped) = self.field(key, raw)
            {
                *raw = scoped;
            }
        }
    }

    pub(crate) fn scope_metadata(self, raw: &str) -> Option<String> {
        super::request::scope_raw_turn_correlations(raw, self)
    }

    pub(crate) fn scope_turn_headers(self, headers: &mut HeaderMap) {
        // Turn IDs are protocol references, not account tracking identifiers.
        strip_trace_headers(headers);
    }

    fn scope_header(self, headers: &mut HeaderMap, name: &'static str) {
        let values: Vec<_> = headers
            .get_all(name)
            .iter()
            .map(|value| {
                value
                    .to_str()
                    .ok()
                    .and_then(|raw| self.field(name, raw))
                    .and_then(|raw| HeaderValue::from_str(&raw).ok())
                    .unwrap_or_else(|| value.clone())
            })
            .collect();
        headers.remove(name);
        for value in values {
            headers.append(name, value);
        }
    }

    pub(crate) fn scope_endpoint_headers(self, headers: &mut HeaderMap) {
        self.scope_turn_headers(headers);
        for name in [
            "session-id",
            "session_id",
            "thread-id",
            "x-client-request-id",
            "x-codex-window-id",
            "x-codex-parent-thread-id",
        ] {
            self.scope_header(headers, name);
        }
        if let Some(raw) = headers
            .get("x-codex-turn-metadata")
            .and_then(|v| v.to_str().ok())
            && let Some(scoped) = self.scope_metadata(raw)
            && let Ok(value) = HeaderValue::from_str(&scoped)
        {
            headers.insert("x-codex-turn-metadata", value);
        }
    }

    pub(super) fn scope_input(self, body: &mut Map<String, Value>) {
        let Some(Value::Array(items)) = body.get_mut("input") else {
            return;
        };
        for item in items {
            self.scope_git_context(item);
        }
    }

    fn scope_git_context(self, item: &mut Value) {
        if item.get("role").and_then(Value::as_str) != Some("user") {
            return;
        }
        let kinds = item
            .pointer("/internal_chat_message_metadata_passthrough/content_item_kinds")
            .cloned();
        let Some(Value::Array(parts)) = item.get_mut("content") else {
            return;
        };
        for (index, part) in parts.iter_mut().enumerate() {
            if kinds.as_ref().is_some_and(|kinds| {
                kinds.get(index).and_then(Value::as_str)
                    != Some("additional_content.codex_git_state")
            }) || part.get("type").and_then(Value::as_str) != Some("input_text")
            {
                continue;
            }
            let Some(Value::String(text)) = part.get_mut("text") else {
                continue;
            };
            if text.len() > 1024 * 1024
                || !text.trim().starts_with("<external_codex_git_state>")
                || !text.trim().ends_with("</external_codex_git_state>")
            {
                continue;
            }
            let _ = rewrite_git_context(text, self);
        }
    }
}

pub(super) fn strip_trace_headers(headers: &mut HeaderMap) {
    for name in TRACE_FIELDS {
        headers.remove(*name);
    }
}

// Upstream's bounded XML/JSON edit, restricted to the two Git correlation fields.
fn rewrite_git_context(text: &mut String, scope: AccountCorrelation<'_>) -> Option<()> {
    let document = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            nodes_limit: 10_000,
            ..Default::default()
        },
    )
    .ok()?;
    let root = document.root_element();
    if !root.has_tag_name("external_codex_git_state") {
        return None;
    }
    let mut nodes = root.descendants().filter(|node| {
        node.has_tag_name("git")
            && node
                .parent()
                .is_some_and(|parent| parent.has_tag_name("environment_context"))
    });
    let git = nodes.next()?;
    if nodes.next().is_some() || git.children().any(|node| !node.is_text()) {
        return None;
    }
    let raw: String = git.children().filter_map(|node| node.text()).collect();
    let Value::Object(mut value) = serde_json::from_str(&raw).ok()? else {
        return None;
    };
    let mut changed = false;
    for name in ["sessionId", "threadId"] {
        if let Some(Value::String(raw)) = value.get_mut(name)
            && let Some(mapped) = scope.field(name, raw)
        {
            *raw = mapped;
            changed = true;
        }
    }
    if !changed {
        return Some(());
    }
    let range = git.range();
    let start = git.first_child()?.range().start;
    let end = range.start + text[range].rfind("</")?;
    let encoded = serde_json::to_string(&value)
        .ok()?
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    if text.len() - (end - start) + encoded.len() > 1024 * 1024 {
        return None;
    }
    text.replace_range(start..end, &encoded);
    Some(())
}
