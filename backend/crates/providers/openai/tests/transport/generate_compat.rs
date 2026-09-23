//! Real loopback encoding checks; no live upstream capabilities are asserted.

use gateway_core::operation::{GenerateRequest, ProtocolPayload};
use provider_openai::encode_generate_request;

use super::*;

fn cases() -> Vec<(Value, Value)> {
    let mut cases: Vec<_> = [
        (json!("fast"), None),
        (json!("priority"), Some(json!(true))),
        (json!("default"), Some(json!(false))),
        (json!("auto"), Some(Value::Null)),
        (json!("flex"), None),
        (json!("FAST"), None),
        (json!("fast "), None),
        (json!("future-tier"), None),
        (Value::Null, None),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (tier, store))| {
        let text = if index == 2 {
            ""
        } else {
            "  hello\n\u{4f60}\u{597d}"
        };
        let input = if index == 1 {
            json!([{"role":"user","content":text,"future_input":{"preserve":true}}])
        } else {
            json!(text)
        };
        let mut original = json!({
            "model": "client-model",
            "input": input,
            "instructions": "preserve instructions",
            "stream": false,
            "service_tier": tier,
            "max_output_tokens": 100,
            "temperature": 0.5,
            "max_completion_tokens": 200,
            "top_p": 0.8,
            "frequency_penalty": 0.2,
            "presence_penalty": 0.3,
            "prompt_cache_retention": "24h",
            "context_management": [{"type":"compaction","compact_threshold":20000}],
            "parallel_tool_calls": false,
            "include": ["reasoning.encrypted_content", "future.include"],
            "truncation": "auto",
            "tools": [{"type":"web_search"}, {"type":"image_generation","quality":"low"}],
            "future_field": {"keep": ["all", "values"]}
        });
        if let Some(store) = store {
            original["store"] = store;
        }
        let mut expected = original.clone();
        expected["model"] = json!("gpt-test");
        expected["stream"] = json!(true);
        expected
            .as_object_mut()
            .unwrap()
            .entry("store")
            .or_insert(json!(false));
        expected
            .as_object_mut()
            .unwrap()
            .remove("max_output_tokens");
        expected.as_object_mut().unwrap().remove("temperature");
        expected
            .as_object_mut()
            .unwrap()
            .remove("prompt_cache_retention");
        expected["tools"][0]["user_location"] = json!({
            "type": "approximate",
            "country": "US",
            "region": "Ohio",
            "city": "Piketon",
            "timezone": "America/New_York"
        });
        if index != 1 {
            expected["input"] = json!([{
                "type":"message","role":"user",
                "content":[{"type":"input_text","text":text}]
            }]);
        }
        if index == 0 {
            expected["service_tier"] = json!("priority");
        }
        (original, expected)
    })
    .collect();
    let original = json!({
        "model": "client-model",
        "stream": false,
        "instructions": "Existing instructions",
        "input": [
            {"type":"message","role":"system","content":"Keep this instruction","future_item":{"preserve":true}},
            {"type":"message","role":"system","content":[{"type":"input_text","text":"Keep nested roles","role":"system"}]},
            {"type":"message","role":"developer","content":"Existing developer instruction"},
            {"type":"message","role":"user","content":"Hello"},
            {"role":"system","content":"Preserve shorthand"},
            {"type":"future_item","role":"system","content":"Opaque content"},
            "opaque-input"
        ],
        "future_field":{"preserve":true}
    });
    let mut expected = original.clone();
    expected["model"] = json!("gpt-test");
    expected["stream"] = json!(true);
    expected["store"] = json!(false);
    expected["input"][0]["role"] = json!("developer");
    expected["input"][1]["role"] = json!("developer");
    cases.push((original, expected));
    cases
}

fn encoded_request(body: &Value, websocket: bool) -> CodexResponsesRequest {
    let payload = ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
        .expect("OpenAI payload")
        .with_context(Map::from_iter([(
            "use_websocket".to_owned(),
            json!(websocket),
        )]));
    let generate = GenerateRequest::from_protocol_payload(payload);
    let mut encoded =
        encode_generate_request(&generate, "gpt-test", &Default::default()).expect("encode");
    // The existing WS-only fixture context disables optional transport fallback.
    if websocket {
        encoded.downstream_websocket_connection_id = Some("compat-downstream".to_owned());
    }
    assert_eq!(
        generate.protocol_payload().body(),
        body.as_object().unwrap()
    );
    assert_eq!(encoded.body().get("service_tier"), body.get("service_tier"));
    encoded
}

#[test]
fn encoder_normalizes_explicit_system_roles_before_transport_and_preserves_original_input() {
    for (original, expected) in cases() {
        for websocket in [false, true] {
            let encoded = encoded_request(&original, websocket);
            assert_eq!(encoded.body().get("input"), expected.get("input"));
            assert_eq!(
                encoded.body().get("instructions"),
                original.get("instructions")
            );
        }
    }
}

fn assert_identity_header(name: &str, value: Option<&str>) {
    let profile = test_wire_profile().snapshot();
    let user_agent = profile.user_agent();
    let expected = match name {
        "authorization" => Some("Bearer access-token"),
        "chatgpt-account-id" => Some("acct-compat"),
        "session-id" => Some("session-compat"),
        "user-agent" => Some(user_agent.as_str()),
        "x-codex-installation-id" => None,
        _ => panic!("unexpected identity header"),
    };
    assert_eq!(value, expected, "{name}");
}

fn assert_routing_hint(value: Option<&str>, expected_body: &Value) {
    let expected = match expected_body["service_tier"].as_str() {
        Some(tier) => format!("model=gpt-test;tier={tier}"),
        None => "model=gpt-test".to_owned(),
    };
    // HTTP parsers may remove trailing optional whitespace from header values.
    assert_eq!(value.map(str::trim_end), Some(expected.trim_end()));
}

const IDENTITY_HEADERS: &[&str] = &[
    "authorization",
    "chatgpt-account-id",
    "session-id",
    "user-agent",
    "x-codex-installation-id",
];

fn compat_context() -> CodexRequestContext<'static> {
    CodexRequestContext {
        installation_id: Some("installation-compat"),
        session_id: Some("session-compat"),
        ..request_context("req_compat", Some("acct-compat"))
    }
}

#[tokio::test]
async fn http_generate_compat_preserves_the_encoded_request_when_sending() {
    for (original, expected) in cases() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let raw = read_http_request_with_body(&mut stream).await;
            write_completed_sse_response(&mut stream).await;
            raw
        });
        let request = encoded_request(&original, false);
        let local_body = request.body().clone();
        let backend = CodexBackendClient::new(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            format!("http://{address}"),
            test_wire_profile(),
        );
        let response = timeout(
            Duration::from_secs(5),
            backend.create_response(&request, compat_context()),
        )
        .await
        .expect("bounded HTTP response")
        .expect("HTTP completion");
        assert_eq!(response.transport, CodexBackendTransport::HttpSse);
        let raw = server.await.unwrap();
        let separator = raw
            .windows(4)
            .position(|value| value == b"\r\n\r\n")
            .unwrap();
        let head = std::str::from_utf8(&raw[..separator]).unwrap();
        for name in IDENTITY_HEADERS {
            assert_identity_header(name, read_header_value(head, name));
        }
        assert_routing_hint(read_header_value(head, "x-codex-routing-hint"), &expected);
        let expected_len = serde_json::to_vec(&expected).unwrap().len();
        assert_eq!(
            read_header_value(head, "content-encoding"),
            (expected_len >= 1024).then_some("zstd")
        );
        let body = if expected_len >= 1024 {
            zstd::stream::decode_all(&raw[separator + 4..]).unwrap()
        } else {
            raw[separator + 4..].to_vec()
        };
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body, expected);
        assert_eq!(request.body(), &local_body);
        assert!(!request.stream());
        assert!(request.force_http_sse);
    }
}

#[tokio::test]
async fn websocket_generate_compat_preserves_explicit_store_and_local_transport_intent() {
    for (mut original, mut expected) in cases() {
        // External continuation uses WS without the optional new-chain opening budget.
        original["previous_response_id"] = json!("resp_previous");
        expected["previous_response_id"] = json!("resp_previous");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut headers = None;
            let mut websocket = accept_codex_test_websocket_with(stream, |request, _| {
                headers = Some(request.headers().clone());
            })
            .await;
            let Some(Ok(Message::Text(payload))) = websocket.next().await else {
                panic!("response.create frame");
            };
            websocket
                .send(Message::Text(
                    json!({
                        "type":"response.completed",
                        "response":{"id":"resp_compat","status":"completed","output":[]}
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            (payload, headers.unwrap())
        });
        let request = encoded_request(&original, true);
        let local_body = request.body().clone();
        let backend = CodexBackendClient::new(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            format!("http://{address}"),
            test_wire_profile(),
        );
        let response = timeout(
            Duration::from_secs(5),
            backend.create_response(&request, compat_context()),
        )
        .await
        .expect("bounded WS response")
        .expect("WS completion");
        assert_eq!(response.transport, CodexBackendTransport::WebSocket);
        let (payload, headers) = server.await.unwrap();
        for name in IDENTITY_HEADERS {
            assert_identity_header(
                name,
                headers.get(*name).and_then(|value| value.to_str().ok()),
            );
        }
        assert_routing_hint(
            headers
                .get("x-codex-routing-hint")
                .and_then(|value| value.to_str().ok()),
            &expected,
        );
        expected["type"] = json!("response.create");
        let mut body: Value = serde_json::from_str(&payload).unwrap();
        let metadata = body
            .as_object_mut()
            .unwrap()
            .remove("client_metadata")
            .unwrap();
        assert_eq!(metadata.as_object().unwrap().len(), 1);
        assert!(
            metadata["x-codex-ws-stream-request-start-ms"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .is_ok()
        );
        assert_eq!(body, expected);
        assert_eq!(request.body(), &local_body);
        assert!(!request.stream());
        assert!(request.use_websocket);
        assert!(!request.force_http_sse);
        assert_eq!(
            request.downstream_websocket_connection_id.as_deref(),
            Some("compat-downstream")
        );
    }
}
