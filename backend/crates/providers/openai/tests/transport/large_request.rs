use super::*;

fn context() -> CodexRequestContext<'static> {
    CodexRequestContext {
        response_control: None,
        installation_id: Some("85c64ae8-4b44-4f82-9696-909adef2144c"),
        session_id: Some("large-request-session"),
        thread_id: Some("large-request-thread"),
        ..request_context("large-request-fixture", Some("selected-workspace"))
    }
}

fn request(input: Value) -> CodexResponsesRequest {
    let generate = gateway_core::operation::GenerateRequest::from_protocol_payload(
        gateway_core::operation::ProtocolPayload::json_object(
            "openai",
            json!({
                "model": "gpt-test",
                "instructions": "preserve instructions",
                "input": input,
                "stream": false,
                "store": false,
                "tools": [{"type": "function", "name": "lookup", "parameters": {"type":"object"}}],
                "metadata": {"opaque": "unchanged"},
                "client_metadata": {"session_id":"client-session", "opaque":{"keep":true}}
            })
            .as_object()
            .unwrap()
            .clone(),
        )
        .unwrap(),
    );
    let mut request = provider_openai::transport::request::encode_generate_request_with_location(
        &generate, "gpt-test", None,
    )
    .unwrap();
    request.use_websocket = true;
    request.local_conversation_id = Some("large-request-conversation".to_owned());
    request
}

struct Observed {
    headers: String,
    body: Value,
    payload_bytes: u64,
}

async fn exchange(
    request: &CodexResponsesRequest,
    limit: Option<u64>,
    expected: CodexBackendTransport,
) -> (CollectedBackendResponse, Observed) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = timeout(Duration::from_secs(10), listener.accept())
            .await
            .unwrap()
            .unwrap();
        if expected == CodexBackendTransport::HttpSse {
            let mut websocket = accept_codex_test_websocket_with(stream, |_, response| {
                response.headers_mut().insert(
                    "x-codex-websocket-max-message-bytes",
                    limit.unwrap().to_string().parse().unwrap(),
                );
            })
            .await;
            while let Some(Ok(message)) = websocket.next().await {
                assert!(!message.is_text());
            }
            let (mut stream, _) = listener.accept().await.unwrap();
            let raw = read_http_request_with_body(&mut stream).await;
            let split = raw.windows(4).position(|part| part == b"\r\n\r\n").unwrap();
            let headers = String::from_utf8(raw[..split].to_vec()).unwrap();
            let bytes = if headers
                .to_ascii_lowercase()
                .contains("content-encoding: zstd")
            {
                zstd::stream::decode_all(&raw[split + 4..]).unwrap()
            } else {
                raw[split + 4..].to_vec()
            };
            write_completed_sse_response(&mut stream).await;
            Observed {
                headers,
                body: serde_json::from_slice(&bytes).unwrap(),
                payload_bytes: bytes.len() as u64,
            }
        } else {
            let headers = Arc::new(Mutex::new(String::new()));
            let captured = Arc::clone(&headers);
            let mut websocket =
                accept_codex_test_websocket_with(stream, move |request, response| {
                    if let Some(limit) = limit {
                        response.headers_mut().insert(
                            "x-codex-websocket-max-message-bytes",
                            limit.to_string().parse().unwrap(),
                        );
                    }
                    *captured.lock().unwrap() = format!("{:?}", request.headers());
                    response.headers_mut().insert(
                        "sec-websocket-extensions",
                        "permessage-deflate".parse().unwrap(),
                    );
                })
                .await;
            let message = websocket.next().await.unwrap().unwrap();
            let text = message.to_text().unwrap();
            let observed = Observed {
                headers: headers.lock().unwrap().clone(),
                body: serde_json::from_str(text).unwrap(),
                payload_bytes: text.len() as u64,
            };
            websocket
                .send(Message::Text(
                    completed_websocket_response("resp_large_fixture", 3, 1).into(),
                ))
                .await
                .unwrap();
            observed
        }
    });
    let pool = Arc::new(CodexWebSocketPool::default());
    let backend = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        format!("http://{addr}"),
        test_wire_profile(),
    )
    .with_websocket_pool(Arc::clone(&pool));
    let original = request.body().clone();
    let response = timeout(
        Duration::from_secs(10),
        backend.create_response(request, context()),
    )
    .await
    .unwrap()
    .unwrap_or_else(|error| panic!("transport fixture failed: {error}"));
    let observed = server.await.unwrap();
    pool.shutdown().await;
    assert_eq!(request.body(), &original);
    assert_eq!(response.transport, expected);
    (response, observed)
}

#[tokio::test]
async fn advertised_limit_preserves_projected_http_and_websocket_payloads() {
    let request = request(json!("\u{4f60}\u{597d}".repeat(2000)));
    let (_, websocket) = exchange(&request, None, CodexBackendTransport::WebSocket).await;
    let size = websocket.payload_bytes;
    assert!(
        size > serde_json::to_string(&websocket.body)
            .unwrap()
            .chars()
            .count() as u64
    );
    for (threshold, expected) in [
        (size - 1, CodexBackendTransport::HttpSse),
        (size, CodexBackendTransport::WebSocket),
        (size + 1, CodexBackendTransport::WebSocket),
    ] {
        let (response, observed) = exchange(&request, Some(threshold), expected).await;
        assert_eq!(observed.body["input"], websocket.body["input"]);
        assert_eq!(observed.body["tools"], websocket.body["tools"]);
        assert_eq!(observed.body["metadata"], websocket.body["metadata"]);
        assert_eq!(
            observed.body["prompt_cache_key"],
            websocket.body["prompt_cache_key"]
        );
        assert_eq!(
            observed.body["client_metadata"]["session_id"],
            websocket.body["client_metadata"]["session_id"]
        );
        assert_eq!(
            observed.body["client_metadata"]["opaque"],
            json!({"keep":true})
        );
        assert_eq!(observed.body["stream"], true);
        assert_eq!(observed.body["store"], false);
        if expected == CodexBackendTransport::HttpSse {
            assert_eq!(
                response.transport_metrics.decision,
                Some(CodexTransportDecision::Http2WebSocketMessageTooLarge)
            );
            assert!(response.transport_metrics.ws_connect_ms.is_some());
            assert!(
                observed
                    .headers
                    .contains("authorization: Bearer access-token")
            );
            assert!(
                observed
                    .headers
                    .contains("chatgpt-account-id: selected-workspace")
            );
            assert!(observed.headers.contains("content-encoding: zstd"));
        }
    }
}
