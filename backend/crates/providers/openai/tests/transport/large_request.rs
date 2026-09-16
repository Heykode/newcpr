use gateway_core::{routing::RequestTuning, runtime::RequestTuningHandle};

use super::*;

fn context() -> CodexRequestContext<'static> {
    CodexRequestContext {
        installation_id: Some("85c64ae8-4b44-4f82-9696-909adef2144c"),
        session_id: Some("large-request-session"),
        thread_id: Some("large-request-thread"),
        ..request_context("large-request-fixture", Some("selected-workspace"))
    }
}

fn request(input: Value) -> CodexResponsesRequest {
    let mut request = CodexResponsesRequest::from_body(
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
    );
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
    tuning: RequestTuning,
    expected: CodexBackendTransport,
) -> (CollectedBackendResponse, Observed) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = timeout(Duration::from_secs(10), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut prefix = [0; 1];
        stream.peek(&mut prefix).await.unwrap();
        if expected == CodexBackendTransport::HttpSse {
            assert_eq!(&prefix, b"P", "large request must not open a WebSocket");
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
            assert_eq!(&prefix, b"G", "protected request must remain on WebSocket");
            let headers = Arc::new(Mutex::new(String::new()));
            let captured = Arc::clone(&headers);
            let mut websocket =
                accept_codex_test_websocket_with(stream, move |request, response| {
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
    .with_websocket_pool(Arc::clone(&pool))
    .with_request_tuning(RequestTuningHandle::new(tuning));
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
async fn final_utf8_payload_threshold_and_switches_select_transport_before_opening() {
    let request = request(json!("\u{4f60}\u{597d}".repeat(2000)));
    let (_, websocket) = exchange(
        &request,
        RequestTuning {
            websocket_large_request_threshold_bytes: 0,
            ..RequestTuning::default()
        },
        CodexBackendTransport::WebSocket,
    )
    .await;
    let size = websocket.payload_bytes;
    assert!(
        size > serde_json::to_string(&websocket.body)
            .unwrap()
            .chars()
            .count() as u64
    );
    for (threshold, enabled, expected) in [
        (size - 1, true, CodexBackendTransport::HttpSse),
        (size, true, CodexBackendTransport::HttpSse),
        (size + 1, true, CodexBackendTransport::WebSocket),
        (0, true, CodexBackendTransport::WebSocket),
        (1, false, CodexBackendTransport::WebSocket),
    ] {
        let (response, observed) = exchange(
            &request,
            RequestTuning {
                websocket_large_request_threshold_bytes: threshold,
                websocket_http_fallback_enabled: enabled,
                ..RequestTuning::default()
            },
            expected,
        )
        .await;
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
                Some(CodexTransportDecision::HttpLargeRequest)
            );
            assert_eq!(response.transport_metrics.ws_connect_ms, None);
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

#[tokio::test]
async fn normalization_not_incoming_body_size_controls_the_threshold() {
    let base = request(json!("brief"));
    let (_, baseline) = exchange(
        &base,
        RequestTuning::default(),
        CodexBackendTransport::WebSocket,
    )
    .await;
    let incoming_size = serde_json::to_vec(base.body()).unwrap().len() as u64;
    assert!(incoming_size + 1 < baseline.payload_bytes);
    let (_, observed) = exchange(
        &base,
        RequestTuning {
            websocket_large_request_threshold_bytes: incoming_size + 1,
            ..RequestTuning::default()
        },
        CodexBackendTransport::HttpSse,
    )
    .await;
    assert_eq!(observed.body["input"], baseline.body["input"]);
    assert!(observed.body["input"].is_array());
}

#[tokio::test]
async fn default_threshold_selects_http_even_for_highly_compressible_payload() {
    let request = request(json!("x".repeat(15 * 1024 * 1024)));
    let (response, _) = exchange(
        &request,
        RequestTuning::default(),
        CodexBackendTransport::HttpSse,
    )
    .await;
    assert_eq!(
        response.transport_metrics.decision,
        Some(CodexTransportDecision::HttpLargeRequest)
    );
}

#[tokio::test]
async fn native_websocket_warmup_and_previous_response_scopes_are_not_size_fallback_candidates() {
    let mut native = request(json!("native"));
    native.downstream_websocket_connection_id = Some("downstream-connection".to_owned());
    let mut warmup_body = native.body().clone();
    warmup_body.insert("generate".to_owned(), json!(false));
    let mut warmup = CodexResponsesRequest::from_body(warmup_body);
    warmup.use_websocket = true;
    warmup.local_conversation_id = Some("warmup-conversation".to_owned());
    let mut cases = vec![native, warmup];
    for scope in [
        PreviousResponseScope::Persisted,
        PreviousResponseScope::ExternalUnknown,
    ] {
        let mut previous = request(json!("continuation"));
        previous.set_previous_response_id(Some("resp_owned_previous".to_owned()));
        previous.previous_response_scope = Some(scope);
        cases.push(previous);
    }
    for request in cases {
        let (_, observed) = exchange(
            &request,
            RequestTuning {
                websocket_large_request_threshold_bytes: 1,
                ..RequestTuning::default()
            },
            CodexBackendTransport::WebSocket,
        )
        .await;
        assert_eq!(
            observed.body.get("previous_response_id"),
            request.body().get("previous_response_id")
        );
    }
}

#[tokio::test]
async fn exact_continuation_cannot_escape_missing_connection_through_size_fallback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut request = request(json!("continuation"));
    request.set_previous_response_id(Some("resp_missing_connection".to_owned()));
    request.previous_response_scope = Some(PreviousResponseScope::ConnectionLocal);
    let pool = Arc::new(CodexWebSocketPool::default());
    let backend = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        format!("http://{}", listener.local_addr().unwrap()),
        test_wire_profile(),
    )
    .with_websocket_pool(Arc::clone(&pool))
    .with_request_tuning(RequestTuningHandle::new(RequestTuning {
        websocket_large_request_threshold_bytes: 1,
        ..RequestTuning::default()
    }));
    let error = timeout(
        Duration::from_secs(5),
        backend.create_response(&request, context()),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(error, CodexClientError::WebSocket(_)), "{error:?}");
    assert!(
        timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    pool.shutdown().await;
}

#[tokio::test]
async fn failed_large_http_request_is_not_replayed_over_websocket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let raw = read_http_request_with_body(&mut stream).await;
        assert!(raw.starts_with(b"POST "));
        stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await.unwrap();
        assert!(
            timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let backend = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        format!("http://{addr}"),
        test_wire_profile(),
    )
    .with_request_tuning(RequestTuningHandle::new(RequestTuning {
        websocket_large_request_threshold_bytes: 1,
        ..RequestTuning::default()
    }));
    let error = timeout(
        Duration::from_secs(5),
        backend.create_response(&request(json!("failed")), context()),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        error,
        CodexClientError::Upstream {
            status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
            ..
        }
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn later_independent_small_request_in_same_session_can_use_websocket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut http, _) = listener.accept().await.unwrap();
        let raw = read_http_request_with_body(&mut http).await;
        assert!(raw.starts_with(b"POST "));
        write_completed_sse_response(&mut http).await;
        drop(http);
        let (stream, _) = listener.accept().await.unwrap();
        let mut websocket = accept_codex_test_websocket(stream).await;
        let message = websocket.next().await.unwrap().unwrap();
        let body: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert!(body.get("previous_response_id").is_none());
        websocket
            .send(Message::Text(
                completed_websocket_response("resp_later_small", 3, 1).into(),
            ))
            .await
            .unwrap();
    });
    let pool = Arc::new(CodexWebSocketPool::default());
    let backend = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        format!("http://{addr}"),
        test_wire_profile(),
    )
    .with_websocket_pool(Arc::clone(&pool))
    .with_request_tuning(RequestTuningHandle::new(RequestTuning {
        websocket_large_request_threshold_bytes: 4096,
        ..RequestTuning::default()
    }));
    let large = timeout(
        Duration::from_secs(10),
        backend.create_response(&request(json!("x".repeat(8192))), context()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        large.transport_metrics.decision,
        Some(CodexTransportDecision::HttpLargeRequest)
    );
    let small = timeout(
        Duration::from_secs(10),
        backend.create_response(&request(json!("small")), context()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(small.transport, CodexBackendTransport::WebSocket);
    server.await.unwrap();
    pool.shutdown().await;
}
