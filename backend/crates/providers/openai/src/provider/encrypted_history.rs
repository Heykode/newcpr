//! One-shot native recovery, adapted from Sub2API's invalid encrypted lineage.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::time::Instant;

use super::execution::{ColdResponse, cold_response_stream_once};
use super::*;

const RETENTION: Duration = Duration::from_secs(60 * 60);
const MAX_SESSIONS: usize = 1024;
const MAX_DIGESTS: usize = 64;
type DigestKey = [u8; 32];

#[derive(Clone, Default)]
pub(super) struct InvalidEncryptedHistory {
    entries: Arc<Mutex<HashMap<DigestKey, Entry>>>,
}

struct Entry {
    digests: HashSet<DigestKey>,
    updated: Instant,
}

impl InvalidEncryptedHistory {
    fn known(&self, scope: DigestKey) -> HashSet<DigestKey> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries
            .get(&scope)
            .is_some_and(|entry| entry.updated.elapsed() >= RETENTION)
        {
            entries.remove(&scope);
        }
        entries
            .get(&scope)
            .map(|entry| entry.digests.clone())
            .unwrap_or_default()
    }

    fn remember(&self, scope: DigestKey, digests: &[DigestKey]) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| entry.updated.elapsed() < RETENTION);
        if !entries.contains_key(&scope)
            && entries.len() >= MAX_SESSIONS
            && let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.updated)
                .map(|(key, _)| *key)
        {
            entries.remove(&oldest);
        }
        let entry = entries.entry(scope).or_insert_with(|| Entry {
            digests: HashSet::new(),
            updated: Instant::now(),
        });
        for digest in digests.iter().take(MAX_DIGESTS) {
            if entry.digests.len() >= MAX_DIGESTS {
                break;
            }
            entry.digests.insert(*digest);
        }
        entry.updated = Instant::now();
    }
}

fn scope(response: &ColdResponse) -> Option<DigestKey> {
    let session = response
        .request
        .local_conversation_id
        .as_deref()
        .filter(|value| !value.is_empty())?;
    Some(scope_key(
        response.lease.account_id().as_str(),
        response.context.client_api_key_ref().as_str(),
        session,
    ))
}

fn scope_key(account: &str, client: &str, session: &str) -> DigestKey {
    let mut digest = Sha256::new();
    for part in [account, client, session] {
        digest.update(part.len().to_le_bytes());
        digest.update(part.as_bytes());
    }
    digest.finalize().into()
}

fn encrypted_reasoning(item: &Value) -> Option<&str> {
    (item.get("type").and_then(Value::as_str) == Some("reasoning"))
        .then(|| item.get("encrypted_content").and_then(Value::as_str))
        .flatten()
        .filter(|value| !value.is_empty())
}

fn eligible(request: &CodexResponsesRequest) -> bool {
    request.excel.is_none()
        && request.quality_probe.is_none()
        && request.generate()
        && !matches!(
            transport_requirement(request),
            TransportRequirement::ExactWebSocketContinuation
                | TransportRequirement::ExternalUnknown
        )
        && !request.input().iter().any(|item| {
            let required_cipher =
                encrypted_reasoning(item).is_none() && item.get("encrypted_content").is_some();
            required_cipher
                || item.get("type").and_then(Value::as_str) == Some("item_reference")
                || item.get("encrypted_function_args").is_some()
                || ["content", "output"].iter().any(|field| {
                    item.get(field)
                        .and_then(Value::as_array)
                        .is_some_and(|parts| {
                            parts.iter().any(|part| {
                                part.get("encrypted_content").is_some()
                                    || part.get("type").and_then(Value::as_str)
                                        == Some("encrypted_content")
                            })
                        })
                })
        })
}

fn strip(
    request: &mut CodexResponsesRequest,
    known: Option<&HashSet<DigestKey>>,
) -> Vec<DigestKey> {
    let Some(input) = request
        .body_mut()
        .get_mut("input")
        .and_then(Value::as_array_mut)
    else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    input.retain_mut(|item| {
        let Some(encrypted) = encrypted_reasoning(item) else {
            return true;
        };
        let digest: DigestKey = Sha256::digest(encrypted.as_bytes()).into();
        if known.is_some_and(|known| !known.contains(&digest)) {
            return true;
        }
        if removed.len() < MAX_DIGESTS {
            removed.push(digest);
        }
        let object = item.as_object_mut().expect("reasoning object");
        object.shift_remove("encrypted_content");
        if object.get("content").is_some_and(Value::is_null) {
            object.shift_remove("content");
        }
        // A stateless ID without replayable content would cause a second missing-item error.
        let has_plaintext = ["summary", "content"].iter().any(|field| {
            object
                .get(*field)
                .and_then(Value::as_array)
                .is_some_and(|parts| !parts.is_empty())
        });
        if has_plaintext {
            object.shift_remove("id");
        }
        has_plaintext
    });
    removed
}

fn retry_request(
    request: &CodexResponsesRequest,
) -> Option<(CodexResponsesRequest, Vec<DigestKey>)> {
    if !eligible(request) {
        return None;
    }
    let mut retry = request.clone();
    let removed = strip(&mut retry, None);
    if removed.is_empty() || retry.input().is_empty() {
        return None;
    }
    // A persisted anchor can contain history absent from input. Removing it is not
    // proof of complete replay, even without a tool output. Keep its ownership.
    Some((retry, removed))
}

pub(super) fn recovering_stream(mut response: ColdResponse) -> EventStream {
    if !eligible(&response.request)
        || !matches!(
            response.lease.authentication(),
            crate::credential::CodexRuntimeAuthentication::OAuth(_)
        )
        || !response
            .request
            .input()
            .iter()
            .any(|item| encrypted_reasoning(item).is_some())
    {
        return cold_response_stream_once(response);
    }
    response.client = response.client.with_same_attempt_recovery();
    let cache = response.encrypted_history.clone();
    let scope = scope(&response);
    if let Some(scope) = scope {
        let known = cache.known(scope);
        if !known.is_empty() {
            let mut cleaned = (*response.request).clone();
            let removed = strip(&mut cleaned, Some(&known));
            if !removed.is_empty() && !cleaned.input().is_empty() {
                response.context.trace().record(
                    "native.encrypted_history",
                    json!({
                        "phase":"known_invalid_removed", "items":removed.len(),
                    }),
                );
                response.request = Arc::new(cleaned);
            }
        }
    }
    Box::pin(async_stream::try_stream! {
        let mut retry = response.clone();
        let mut stream = cold_response_stream_once(response);
        let mut recovery_used = false;
        let mut client_committed = false;
        let mut pending_digests: Option<Vec<DigestKey>> = None;
        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    client_committed |= event.has_client_event();
                    let completed = event
                        .canonical_facts()
                        .iter()
                        .any(|event| matches!(event, GatewayEvent::Completed(meta)
                            if matches!(meta.finish_reason(), Some(FinishReason::Stop | FinishReason::ToolCall))))
                        && !super::observation::terminal_response_is_incomplete(std::slice::from_ref(&event));
                    if completed && let (Some(scope), Some(digests)) = (scope, pending_digests.take()) {
                        cache.remember(scope, &digests);
                    }
                    yield event;
                }
                Err(error) => {
                    let explicit_rejection = error.upstream_code().map(OpaqueUpstreamValue::as_str)
                        == Some("invalid_encrypted_content");
                    if !client_committed && !recovery_used && explicit_rejection && !retry.context.deadline().is_elapsed()
                        && !retry.context.cancellation().is_cancelled()
                        && let Some((request, digests)) = retry_request(&retry.request)
                    {
                        drop(stream);
                        retry.context.trace().record("native.encrypted_history", json!({
                            "phase":"same_account_retry", "reason":"invalid_encrypted_content",
                            "attempt":2, "items":digests.len(),
                        }));
                        recovery_used = true;
                        retry.request = Arc::new(request);
                        pending_digests = Some(digests);
                        tokio::task::yield_now().await;
                        stream = cold_response_stream_once(retry.clone());
                    } else {
                        Err(error)?;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CodexResponsesRequest {
        CodexResponsesRequest::from_body(json!({"model":"gpt-5.4","input":[
            {"type":"reasoning","id":"rs_fixture","encrypted_content":"cipher-fixture","summary":[]},
            {"role":"user","content":"keep user history"}
        ]}).as_object().unwrap().clone())
    }

    #[test]
    fn native_encrypted_recovery_preserves_required_history_and_unknown_owners() {
        for item in [
            json!({"type":"compaction","encrypted_content":"required-history"}),
            json!({"type":"compaction_summary","encrypted_content":"required-history"}),
            json!({"type":"function_call","encrypted_function_args":["required-arguments"]}),
            json!({"type":"message","content":[{"type":"encrypted_content","encrypted_content":"required-body"}]}),
            json!({"type":"item_reference","id":"rs_fixture"}),
        ] {
            let mut req = request();
            req.body_mut()["input"].as_array_mut().unwrap().push(item);
            assert!(retry_request(&req).is_none());
        }
        for scope in [
            None,
            Some(PreviousResponseScope::ConnectionLocal),
            Some(PreviousResponseScope::ExternalUnknown),
        ] {
            let mut req = request();
            req.set_previous_response_id(Some("resp_previous".into()));
            req.previous_response_scope = scope;
            assert!(retry_request(&req).is_none());
        }
        let mut req = request();
        req.body_mut().insert("generate".into(), json!(false));
        assert!(retry_request(&req).is_none());
    }

    #[test]
    fn native_encrypted_recovery_keeps_tool_outputs_attached_and_preserves_plaintext() {
        for tool in [
            None,
            Some("function_call_output"),
            Some("custom_tool_call_output"),
            Some("tool_search_output"),
        ] {
            let mut req = request();
            req.set_previous_response_id(Some("resp_previous".into()));
            req.previous_response_scope = Some(PreviousResponseScope::Persisted);
            req.body_mut()["input"][0]["summary"] =
                json!([{"type":"summary_text","text":"keep summary"}]);
            if let Some(tool) = tool {
                req.body_mut()["input"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"type":tool,"call_id":"call_fixture","output":"keep"}));
            }
            let before = req.body().clone();
            let (retry, _) = retry_request(&req).unwrap();
            assert_eq!(retry.previous_response_id(), Some("resp_previous"));
            assert_eq!(retry.input()[0]["summary"], before["input"][0]["summary"]);
            assert!(retry.input()[0].get("encrypted_content").is_none());
            assert_eq!(req.body(), &before);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn native_encrypted_lineage_is_scoped_bounded_and_expires() {
        let cache = InvalidEncryptedHistory::default();
        let key = scope_key("account-a", "client-a", "session-a");
        let digests = vec![[7; 32]];
        cache.remember(key, &digests);
        assert_eq!(cache.known(key).len(), 1);
        for (account, client, session) in [
            ("account-b", "client-a", "session-a"),
            ("account-a", "client-b", "session-a"),
            ("account-a", "client-a", "session-b"),
        ] {
            assert!(cache.known(scope_key(account, client, session)).is_empty());
        }
        let mut req = request();
        assert!(strip(&mut req, Some(&cache.known(key))).is_empty());
        tokio::time::advance(RETENTION).await;
        assert!(cache.known(key).is_empty());
        for i in 0..MAX_SESSIONS + 2 {
            cache.remember(scope_key("a", "b", &i.to_string()), &digests);
        }
        assert_eq!(cache.entries.lock().unwrap().len(), MAX_SESSIONS);
    }
}
