use provider_openai::transport::{
    CodexRequestContext,
    protocol::responses::CodexResponsesRequest,
    qx_application::{apply_response_headers, project_response_request},
};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::json;

const INSTALLATION: &str = "f20f428a-d175-4e39-a53d-a2a18828053b";

fn request() -> CodexResponsesRequest {
    CodexResponsesRequest::from_body(
        json!({
            "model": "gpt-test",
            "input": "hello",
            "prompt_cache_key": "explicit-cache-key",
            "previous_response_id": "resp-original",
            "client_metadata": {
                "installation_id": INSTALLATION,
                "x-codex-installation-id": INSTALLATION,
                "x-codex-turn-state": "opaque-state",
                "x-codex-turn-metadata": "{\"turn_id\":\"original-turn\"}",
                "x-openai-subagent": "review",
                "ws_request_header_x-openai-internal-codex-responses-lite": "true",
                "unknown": {"preserve": true}
            }
        })
        .as_object()
        .unwrap()
        .clone(),
    )
}

fn context() -> CodexRequestContext<'static> {
    CodexRequestContext::auxiliary(
        "Bearer synthetic-token",
        Some("account-test"),
        "proxy-request",
        Some(INSTALLATION),
    )
}

fn assert_header(headers: &HeaderMap, name: &str, expected: &str) {
    assert_eq!(headers[name], expected, "{name}");
    assert_eq!(headers.get_all(name).iter().count(), 1, "{name}");
}

#[test]
fn resolved_ids_override_passthrough_without_mutating_body_or_continuation() {
    let mut request = request();
    request.explicit_prompt_cache_key = true;
    request.client_session_id = Some("request-session".to_owned());
    request.client_thread_id = Some("request-thread".to_owned());
    request.client_conversation_id = Some("client-conversation".to_owned());
    request.local_conversation_id = Some("lc_local_only".to_owned());
    request.client_turn_id = Some("original-turn".to_owned());
    request.turn_state = Some("opaque-state".to_owned());
    request.responses_lite = Some("true".to_owned());
    request.memgen_request = Some("memory".to_owned());
    let body_before = request.body().clone();
    let mut context = context();
    context.session_id = Some("cpr-session");
    context.thread_id = Some("cpr-thread");
    context.client_request_id = Some("old-request");
    context.codex_window_id = Some("foreign:prefix:00042");
    let mut headers = HeaderMap::new();
    for name in [
        "x-codex-installation-id",
        "session-id",
        "session_id",
        "thread-id",
        "x-client-request-id",
        "conversation_id",
        "x-codex-window-id",
    ] {
        headers.append(name, HeaderValue::from_static("passthrough-a"));
        headers.append(name, HeaderValue::from_static("passthrough-b"));
    }
    for (name, value) in [
        ("x-codex-turn-state", "opaque-state"),
        ("x-codex-turn-metadata", "{\"turn_id\":\"original-turn\"}"),
        ("x-openai-subagent", "review"),
        ("x-openai-internal-codex-responses-lite", "true"),
        ("x-openai-memgen-request", "memory"),
        ("x-codex-parent-thread-id", "parent"),
        ("x-codex-turn-id", "original-turn"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    apply_response_headers(&mut headers, &request, context).unwrap();
    assert!(!headers.contains_key("x-codex-installation-id"));
    for (name, expected) in [
        ("session-id", "cpr-session"),
        ("session_id", "cpr-session"),
        ("thread-id", "cpr-thread"),
        ("x-client-request-id", "cpr-thread"),
        ("conversation_id", "cpr-thread"),
        ("x-codex-window-id", "cpr-thread:42"),
        ("x-codex-beta-features", "remote_compaction_v2"),
        ("x-codex-turn-state", "opaque-state"),
        ("x-codex-turn-metadata", "{\"turn_id\":\"original-turn\"}"),
        ("x-openai-subagent", "review"),
        ("x-openai-internal-codex-responses-lite", "true"),
        ("x-openai-memgen-request", "memory"),
        ("x-codex-parent-thread-id", "parent"),
        ("x-codex-turn-id", "original-turn"),
    ] {
        assert_header(&headers, name, expected);
    }
    let once = headers.clone();
    apply_response_headers(&mut headers, &request, context).unwrap();
    assert_eq!(headers, once);
    assert_eq!(request.body(), &body_before);
    assert!(request.explicit_prompt_cache_key);
    assert_eq!(request.prompt_cache_key(), Some("explicit-cache-key"));
    assert_eq!(
        request.local_conversation_id.as_deref(),
        Some("lc_local_only")
    );
    assert_eq!(request.client_turn_id.as_deref(), Some("original-turn"));
    assert_eq!(request.turn_state.as_deref(), Some("opaque-state"));
    assert_eq!(request.responses_lite.as_deref(), Some("true"));
    assert_eq!(request.memgen_request.as_deref(), Some("memory"));
}

#[test]
fn request_resolved_ids_and_conversation_are_ordered_fallbacks() {
    for (session, thread, conversation, expected_session, expected_thread) in [
        (Some("s"), Some("t"), Some("c"), Some("s"), Some("t")),
        (None, None, Some("c"), Some("c"), Some("c")),
        (Some("s"), None, None, Some("s"), Some("s")),
        (None, Some("t"), None, None, Some("t")),
    ] {
        let mut request = request();
        request.client_session_id = session.map(str::to_owned);
        request.client_thread_id = thread.map(str::to_owned);
        request.client_conversation_id = conversation.map(str::to_owned);
        let mut headers = HeaderMap::new();
        apply_response_headers(&mut headers, &request, context()).unwrap();
        for name in ["session-id", "session_id"] {
            assert_eq!(
                headers.get(name).map(|value| value.to_str().unwrap()),
                expected_session
            );
        }
        assert_eq!(
            headers
                .get("thread-id")
                .map(|value| value.to_str().unwrap()),
            expected_thread
        );
        assert_eq!(
            headers
                .get("conversation_id")
                .map(|value| value.to_str().unwrap()),
            conversation.and(expected_thread)
        );
    }
}

#[test]
fn absent_authority_removes_passthrough_ids_without_exporting_local_anchor() {
    let mut request = request();
    request.local_conversation_id = Some("lc_not_a_wire_id".to_owned());
    let mut context = context();
    context.installation_id = None;
    let mut headers = HeaderMap::new();
    let managed = [
        "x-codex-installation-id",
        "session-id",
        "session_id",
        "thread-id",
        "conversation_id",
        "x-codex-window-id",
    ];
    for name in managed {
        headers.insert(name, HeaderValue::from_static("unresolved-passthrough"));
    }
    apply_response_headers(&mut headers, &request, context).unwrap();
    for name in managed {
        assert!(!headers.contains_key(name), "{name}");
    }
    assert_header(&headers, "x-client-request-id", "proxy-request");
    assert_eq!(request.prompt_cache_key(), Some("explicit-cache-key"));
}

#[test]
fn invalid_protocol_ids_fall_back_without_rejecting_the_body() {
    let mut request = request();
    request.client_session_id = Some("resolved-session".to_owned());
    request.client_thread_id = Some("resolved-thread".to_owned());
    let mut context = context();
    context.session_id = Some("bad\r\nsession");
    context.thread_id = Some("   ");
    let mut headers = HeaderMap::new();
    apply_response_headers(&mut headers, &request, context).unwrap();
    assert_header(&headers, "session-id", "resolved-session");
    assert_header(&headers, "thread-id", "resolved-thread");
    assert!(!headers.contains_key("conversation_id"));
}

#[test]
fn window_generation_uses_resolved_context_then_request_and_u64_bounds() {
    for (context_window, request_window, expected) in [
        (Some("other:0009"), Some("other:7"), "thread:9"),
        (Some("bad"), Some("old:0007"), "thread:7"),
        (Some("old:-1"), None, "thread:0"),
        (Some("old:18446744073709551616"), None, "thread:0"),
        (
            Some("old:18446744073709551615"),
            None,
            "thread:18446744073709551615",
        ),
        (Some("old:"), None, "thread:0"),
        (None, None, "thread:0"),
    ] {
        let mut request = request();
        request.codex_window_id = request_window.map(str::to_owned);
        let mut context = context();
        context.thread_id = Some("thread");
        context.codex_window_id = context_window;
        let mut headers = HeaderMap::new();
        apply_response_headers(&mut headers, &request, context).unwrap();
        assert_header(&headers, "x-codex-window-id", expected);
    }
}

#[test]
fn beta_features_preserve_existing_empty_and_multivalue_declarations() {
    let request = request();
    for values in [vec![""], vec!["custom"], vec!["custom", "other"]] {
        let mut headers = HeaderMap::new();
        for value in &values {
            headers.append(
                "x-codex-beta-features",
                HeaderValue::from_str(value).unwrap(),
            );
        }
        apply_response_headers(&mut headers, &request, context()).unwrap();
        assert_eq!(
            headers
                .get_all("x-codex-beta-features")
                .iter()
                .map(|value| value.to_str().unwrap())
                .collect::<Vec<_>>(),
            values
        );
    }
    let mut context = context();
    context.beta_features = Some("context-feature");
    let mut headers = HeaderMap::new();
    apply_response_headers(&mut headers, &request, context).unwrap();
    assert_header(&headers, "x-codex-beta-features", "context-feature");
    context.beta_features = Some("");
    headers.clear();
    apply_response_headers(&mut headers, &request, context).unwrap();
    assert_header(&headers, "x-codex-beta-features", "");
}

#[test]
fn installation_identity_is_not_interpreted_as_a_header_value() {
    let request = request();
    let mut context = context();
    context.installation_id = Some("invalid\r\ninstallation");
    let mut headers = HeaderMap::new();
    headers.insert("x-custom", HeaderValue::from_static("preserve"));
    headers.insert(
        "x-codex-installation-id",
        HeaderValue::from_static("downstream-device"),
    );
    let body_before = request.body().clone();
    apply_response_headers(&mut headers, &request, context).unwrap();
    assert!(!headers.contains_key("x-codex-installation-id"));
    assert_eq!(headers["x-custom"], "preserve");
    assert_eq!(request.body(), &body_before);
}

fn scoped_request(key: &str) -> CodexResponsesRequest {
    let mut request = CodexResponsesRequest::from_body(
        json!({
            "model":"gpt-test", "instructions":"rules",
            "input":[{"role":"user","content":"first"}],
            "prompt_cache_key":"client-shared-cache",
            "client_metadata": {
                "session_id":"original-session", "thread_id":"original-thread",
                "x-codex-window-id":"original-thread:4", "other":{"keep":true}
            }
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    request.client_api_key_id = Some(key.to_owned());
    request.client_session_id = Some("original-session".to_owned());
    request.client_thread_id = Some("original-thread".to_owned());
    request.codex_window_id = Some("original-thread:4".to_owned());
    request.local_conversation_id = Some("lc_unchanged".to_owned());
    request
}

#[test]
fn account_scoped_projection_is_stable_and_does_not_feed_back_into_local_identity() {
    let request = scoped_request("key-a");
    let before = request.body().clone();
    let projected = project_response_request(&request, context());
    let mut headers = HeaderMap::new();
    apply_response_headers(&mut headers, &projected, context()).unwrap();
    let thread = headers["thread-id"].to_str().unwrap();
    let session = headers["session-id"].to_str().unwrap();
    assert_ne!(thread, "original-thread");
    assert_ne!(session, thread);
    assert_eq!(uuid::Uuid::parse_str(thread).unwrap().get_version_num(), 7);
    assert_eq!(projected.prompt_cache_key(), Some(thread));
    assert_eq!(projected.body()["client_metadata"]["thread_id"], thread);
    assert_eq!(projected.body()["client_metadata"]["session_id"], session);
    assert_eq!(headers["x-client-request-id"], thread);
    assert_eq!(headers["x-codex-window-id"], format!("{thread}:4"));
    assert_eq!(projected.body()["input"], before["input"]);
    assert_eq!(
        projected.body()["client_metadata"]["other"],
        before["client_metadata"]["other"]
    );
    assert_eq!(
        projected.local_conversation_id,
        request.local_conversation_id
    );
    assert_eq!(request.body(), &before);
    let twice = project_response_request(&projected, context());
    assert_eq!(twice.body(), projected.body());
}

#[test]
fn scoped_ids_separate_client_keys_accounts_and_installations() {
    let request = scoped_request("key-a");
    let original = project_response_request(&request, context());
    assert_ne!(
        original.prompt_cache_key(),
        project_response_request(&scoped_request("key-b"), context()).prompt_cache_key()
    );
    let mut other_account = context();
    other_account.account_id = Some("other-workspace");
    assert_ne!(
        original.prompt_cache_key(),
        project_response_request(&request, other_account).prompt_cache_key()
    );
    let mut other_installation = context();
    other_installation.installation_id = Some("844efdf0-bd69-49b1-a13a-cb101c4d0162");
    assert_ne!(
        original.prompt_cache_key(),
        project_response_request(&request, other_installation).prompt_cache_key()
    );
    // Reprojecting an outbound copy for a different account still uses the original seed.
    assert_eq!(
        project_response_request(&original, other_account).body(),
        project_response_request(&request, other_account).body(),
    );
}

#[test]
fn cache_only_clients_already_get_stable_scoped_session_headers() {
    let make = |cache: &str| {
        let mut request = CodexResponsesRequest::from_body(
            json!({"model":"gpt-test","input":"hello","prompt_cache_key":cache})
                .as_object()
                .unwrap()
                .clone(),
        );
        request.client_api_key_id = Some("key-cache-only".to_owned());
        request
    };
    let request = make("shared-cache");
    let mut first_context = context();
    first_context.session_id = None;
    first_context.thread_id = None;
    first_context.client_request_id = None;
    let mut second_context = first_context;
    second_context.request_id = "another-request";
    let mut first = HeaderMap::new();
    let mut second = HeaderMap::new();
    apply_response_headers(&mut first, &request, first_context).unwrap();
    apply_response_headers(&mut second, &request, second_context).unwrap();
    for name in ["session-id", "thread-id", "x-client-request-id"] {
        assert_eq!(first[name], second[name], "{name}");
        assert_ne!(first[name], "shared-cache");
    }
    assert_eq!(
        project_response_request(&request, first_context).prompt_cache_key(),
        Some(first["thread-id"].to_str().unwrap()),
    );
    let mut changed = HeaderMap::new();
    apply_response_headers(&mut changed, &make("other-cache"), second_context).unwrap();
    assert_ne!(first["session-id"], changed["session-id"]);
    second_context.account_id = Some("another-account");
    apply_response_headers(&mut changed, &request, second_context).unwrap();
    assert_ne!(first["session-id"], changed["session-id"]);
    assert_eq!(request.prompt_cache_key(), Some("shared-cache"));
}

#[test]
fn explicit_thread_wins_over_cache_and_changing_turns() {
    let first = scoped_request("key-a");
    let mut body = first.body().clone();
    body.insert(
        "input".to_owned(),
        json!([{"role":"user","content":"different"}]),
    );
    body.insert("prompt_cache_key".to_owned(), json!("another-cache"));
    let mut next = CodexResponsesRequest::from_body(body);
    next.client_api_key_id = first.client_api_key_id.clone();
    next.client_session_id = first.client_session_id.clone();
    next.client_thread_id = first.client_thread_id.clone();
    assert_eq!(
        project_response_request(&first, context()).prompt_cache_key(),
        project_response_request(&next, context()).prompt_cache_key(),
    );
}

#[test]
fn weak_content_seed_handles_equivalent_input_but_does_not_merge_empty_requests() {
    let make = |input| {
        let mut request = CodexResponsesRequest::from_body(
            json!({"model":"gpt-test","input":input})
                .as_object()
                .unwrap()
                .clone(),
        );
        request.client_api_key_id = Some("key-a".to_owned());
        request
    };
    let string = project_response_request(&make(json!("hello")), context());
    let array = project_response_request(
        &make(json!([{"role":"user","content":[{"type":"input_text","text":"hello"}]}])),
        context(),
    );
    assert_eq!(string.prompt_cache_key(), array.prompt_cache_key());
    assert_ne!(
        string.prompt_cache_key(),
        project_response_request(&make(json!("other")), context()).prompt_cache_key()
    );
    let empty = make(json!([]));
    let mut next = context();
    next.request_id = "different-request";
    assert_ne!(
        project_response_request(&empty, context()).prompt_cache_key(),
        project_response_request(&empty, next).prompt_cache_key()
    );
}

#[test]
fn parent_and_child_use_the_same_account_scoped_thread_namespace() {
    let parent = scoped_request("key-a");
    let mut child = scoped_request("key-a");
    child.client_thread_id = Some("child-thread".to_owned());
    child.parent_thread_id = Some("original-thread".to_owned());
    let mut parent_headers = HeaderMap::new();
    let mut child_headers = HeaderMap::new();
    apply_response_headers(&mut parent_headers, &parent, context()).unwrap();
    apply_response_headers(&mut child_headers, &child, context()).unwrap();
    assert_eq!(
        child_headers["x-codex-parent-thread-id"],
        parent_headers["thread-id"]
    );
    assert_ne!(child_headers["thread-id"], parent_headers["thread-id"]);
    assert_eq!(child_headers["session-id"], parent_headers["session-id"]);
}

#[test]
fn qx_projection_preserves_ascii_turn_metadata_without_losing_unicode() {
    let raw = r#"{"session_id":"original-session","thread_id":"original-thread","workspaces":{"/tmp/\u4e2d\u6587/\ud83d\ude80":{"literal":"\\u4e2d","label":"caf\u00e9"}}}"#;
    let original = scoped_request("key-a");
    let mut body = original.body().clone();
    body.insert("turnMetadata".to_owned(), json!(raw));
    body.get_mut("client_metadata").unwrap()["x-codex-turn-metadata"] = json!(raw);
    let mut request = CodexResponsesRequest::from_body(body);
    request.client_api_key_id = original.client_api_key_id;
    request.client_session_id = original.client_session_id;
    request.client_thread_id = original.client_thread_id;
    let projected = project_response_request(&request, context());
    let mut headers = HeaderMap::new();
    headers.insert("x-codex-turn-metadata", HeaderValue::from_str(raw).unwrap());
    apply_response_headers(&mut headers, &projected, context()).unwrap();
    let mut expected: serde_json::Value = serde_json::from_str(raw).unwrap();
    expected["session_id"] = json!(headers["session-id"].to_str().unwrap());
    expected["thread_id"] = json!(headers["thread-id"].to_str().unwrap());
    for encoded in [
        projected.body()["turnMetadata"]
            .as_str()
            .unwrap()
            .as_bytes(),
        projected.body()["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .unwrap()
            .as_bytes(),
        headers["x-codex-turn-metadata"].as_bytes(),
    ] {
        assert!(
            encoded.is_ascii(),
            "metadata must remain header-safe ASCII JSON"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(encoded).unwrap(),
            expected
        );
    }
    assert_eq!(projected.body()["input"], request.body()["input"]);
}
