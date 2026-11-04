//! 验证 HTTP 与 WebSocket 的账号头部隔离及中间件覆盖

use super::*;

const SELECTED_ACCOUNT: &str = "acct_scope_same";
const CLIENT_ACCOUNT_HEADERS: &[(&str, &[&str])] = &[
    (
        "traceparent",
        &["00-11111111111111111111111111111111-2222222222222222-01"],
    ),
    ("tracestate", &["vendor=client"]),
    ("sentry-trace", &["client-trace"]),
    ("baggage", &["environment=synthetic", "release=synthetic"]),
    ("x-openai-fedramp", &["true", "false"]),
];

fn client_account_metadata() -> Map<String, Value> {
    let mut metadata = Map::new();
    for (name, values) in CLIENT_ACCOUNT_HEADERS {
        metadata.insert((*name).to_owned(), json!(values[0]));
        metadata.insert(format!("ws_request_header_{name}"), json!(values[0]));
    }
    metadata
}

pub(super) fn isolation_history() -> Value {
    json!([
        {"role":"user", "content":"hello"},
        {"type":"function_call", "id":"fc_history", "call_id":"call_history", "name":"lookup", "arguments":"{}"},
        {
            "type":"function_call_output", "call_id":"call_history", "output":"tool result",
            "internal_chat_message_metadata_passthrough": {
                "executed_tool_calls": [{
                    "name":"lookup", "arguments":{},
                    "tool_result_metadata": {
                        "openai/resource_access":{"resource_coverage":"complete", "resources":[]},
                        "parent_response_id":"tool-business-extension"
                    }
                }]
            }
        },
        {"role":"user", "content":"continue"}
    ])
}

fn isolation_payload() -> ProtocolPayload {
    let mut turn_metadata = client_account_metadata();
    turn_metadata.insert("parent_response_id".to_owned(), json!("client-correlation"));
    turn_metadata.insert("workspace".to_owned(), json!("/工作区"));
    turn_metadata.insert("future".to_owned(), json!({"traceparent":"business-value"}));
    let mut body = json!({
        "model": "gpt-5.4",
        "input": isolation_history(),
        "client_metadata": {
            "parent_response_id": "resp_parent_old",
            "guardian_credits_requested": "true",
            "x-codex-turn-metadata": Value::Object(turn_metadata.clone()).to_string(),
            "turnMetadata": turn_metadata,
            "mcp_attribution": r#"{"status":"complete","sources":[]}"#
        }
    });
    body.as_object_mut()
        .unwrap()
        .extend(client_account_metadata());
    body["client_metadata"]
        .as_object_mut()
        .unwrap()
        .extend(client_account_metadata());
    body["turnMetadata"] = json!(turn_metadata);
    body["turn_metadata"] = json!(Value::Object(turn_metadata.clone()).to_string());
    let mut headers = vec![
        json!(["X-OpenAI-Account-Routing-Override", STANDARD.encode("us")]),
        json!([
            "x-openai-account-routing-override",
            STANDARD.encode("us_cr")
        ]),
        json!(["x-codex-guardian", STANDARD.encode("true")]),
        json!(["x-business-extension", STANDARD.encode("first")]),
        json!(["x-business-extension", STANDARD.encode("second")]),
    ];
    for (name, values) in CLIENT_ACCOUNT_HEADERS {
        for value in *values {
            headers.push(json!([name.to_ascii_uppercase(), STANDARD.encode(value)]));
        }
    }
    ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
        .unwrap()
        .with_context(Map::from_iter([
            ("opaque_request_headers".to_owned(), json!(headers)),
            (
                "turn_metadata".to_owned(),
                json!(Value::Object(turn_metadata).to_string()),
            ),
        ]))
}

fn isolation_context(owner: Option<&str>) -> AttemptContext {
    match owner {
        Some(owner) => context_with_state_owner("req_account_isolation", owner),
        None => context("req_account_isolation", CancellationToken::new()),
    }
}

fn assert_isolated_request(
    headers: &reqwest::header::HeaderMap,
    body: &Value,
    owner: Option<&str>,
) {
    assert!(!headers.contains_key("x-openai-account-routing-override"));
    let switched = owner.is_some_and(|owner| owner != SELECTED_ACCOUNT);
    for (name, values) in CLIENT_ACCOUNT_HEADERS {
        let actual: Vec<_> = headers
            .get_all(*name)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect();
        let expected = if switched { &[][..] } else { *values };
        assert_eq!(actual, expected, "{name}, owner={owner:?}");
        for metadata in [body, &body["client_metadata"]] {
            let expected = (!switched).then(|| json!(values[0]));
            assert_eq!(metadata.get(*name), expected.as_ref());
            assert_eq!(
                metadata.get(format!("ws_request_header_{name}")),
                expected.as_ref()
            );
        }
    }
    assert_eq!(headers["x-codex-guardian"], "true");
    assert_eq!(
        headers
            .get_all("x-business-extension")
            .iter()
            .map(|value| value.as_bytes())
            .collect::<Vec<_>>(),
        vec![b"first".as_slice(), b"second".as_slice()],
    );
    assert_eq!(
        body.pointer("/client_metadata/parent_response_id")
            .and_then(Value::as_str),
        (owner == Some(SELECTED_ACCOUNT)).then_some("resp_parent_old"),
    );
    assert_eq!(
        body["client_metadata"]["guardian_credits_requested"],
        "true"
    );
    let mut expected_turn_metadata = if switched {
        Map::new()
    } else {
        client_account_metadata()
    };
    expected_turn_metadata.insert("parent_response_id".to_owned(), json!("client-correlation"));
    expected_turn_metadata.insert("workspace".to_owned(), json!("/工作区"));
    expected_turn_metadata.insert("future".to_owned(), json!({"traceparent":"business-value"}));
    for metadata in [
        &body["turnMetadata"],
        &body["turn_metadata"],
        &body["client_metadata"]["turnMetadata"],
        &body["client_metadata"]["x-codex-turn-metadata"],
    ] {
        let actual = match metadata {
            Value::String(raw) => serde_json::from_str(raw).unwrap(),
            value => value.clone(),
        };
        assert_eq!(actual, Value::Object(expected_turn_metadata.clone()));
    }
    let header = headers["x-codex-turn-metadata"].to_str().unwrap();
    assert!(header.is_ascii());
    assert_eq!(
        serde_json::from_str::<Value>(header).unwrap(),
        Value::Object(expected_turn_metadata)
    );
    assert_eq!(
        body["client_metadata"]["mcp_attribution"],
        r#"{"status":"complete","sources":[]}"#
    );
    assert_eq!(body["input"], isolation_history());
}

#[tokio::test]
async fn http_scopes_client_headers_and_guardian_parent_reference_to_the_account() {
    for owner in [Some(SELECTED_ACCOUNT), Some("acct_scope_old"), None] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, SELECTED_ACCOUNT).await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(CAPTURE_COMPLETED_SSE),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut payload = isolation_payload();
        let mut context = payload.context().clone();
        context.insert("use_websocket".to_owned(), json!(false));
        payload = payload.with_context(context);
        let mut stream = provider_with_base_url(&store, server.uri())
            .execute(
                planned_request(
                    "openai",
                    Operation::Generate(GenerateRequest::from_protocol_payload(payload)),
                ),
                isolation_context(owner),
            )
            .await
            .unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_isolated_request(
            &requests[0].headers,
            &captured_request_body(&requests[0]),
            owner,
        );
    }
}

#[tokio::test]
async fn websocket_scopes_client_headers_and_guardian_parent_reference_to_the_account() {
    for owner in [Some(SELECTED_ACCOUNT), Some("acct_scope_old"), None] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, SELECTED_ACCOUNT).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut headers = None;
            let mut socket =
                crate::transport::accept_codex_test_websocket_with(socket, |request, _| {
                    headers = Some(request.headers().clone());
                })
                .await;
            let message = socket.next().await.unwrap().unwrap();
            let body: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            socket.send(Message::Text(json!({
                "type": "response.completed",
                "response": {"id":"resp_isolation", "model":"gpt-5.4", "status":"completed", "output":[]}
            }).to_string().into())).await.unwrap();
            (headers.unwrap(), body)
        });
        let mut stream = provider_with_base_url(&store, base_url)
            .execute(
                planned_request(
                    "openai",
                    Operation::Generate(
                        GenerateRequest::from_protocol_payload(isolation_payload()),
                    ),
                ),
                isolation_context(owner),
            )
            .await
            .unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        let (headers, body) = server.await.unwrap();
        assert_isolated_request(&headers, &body, owner);
    }
}
