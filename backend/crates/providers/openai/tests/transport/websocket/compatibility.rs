use super::*;

fn request() -> CodexResponsesRequest {
    let mut request = codex_request("gpt-test", "be brief", Vec::new());
    request.local_conversation_id = Some("compat-conversation".to_owned());
    request.client_session_id = Some("compat-session".to_owned());
    request.client_thread_id = Some("compat-thread".to_owned());
    request
}

fn backend(address: std::net::SocketAddr) -> CodexBackendClient {
    backend_with_prefetch(address, 0)
}

fn backend_with_prefetch(address: std::net::SocketAddr, prefetch_bytes: u64) -> CodexBackendClient {
    CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        format!("http://{address}"),
        test_wire_profile(),
    )
    .with_websocket_pool(Arc::new(CodexWebSocketPool::new(Duration::from_secs(60))))
    .with_request_tuning(gateway_core::runtime::RequestTuningHandle::new(
        gateway_core::routing::RequestTuning {
            stream_prefetch_bytes: prefetch_bytes,
            websocket_stream_idle_timeout_ms: 1000,
            ..gateway_core::routing::RequestTuning::defaults()
        },
    ))
}

async fn assert_opening_compatibility(
    first_beta: Option<&str>,
    second_beta: Option<&str>,
    first_window: Option<&str>,
    second_window: Option<&str>,
    reuse: bool,
    exact: bool,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend = backend(listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut sockets = Vec::new();
        let openings = Arc::new(Mutex::new(Vec::new()));
        for index in 0..2 {
            if index == 0 || !reuse {
                let (stream, _) = listener.accept().await.unwrap();
                let captured = openings.clone();
                sockets.push(
                    accept_codex_test_websocket_with(stream, move |request, _| {
                        captured.lock().unwrap().push(request.headers().clone());
                    })
                    .await,
                );
            }
            let socket = sockets.last_mut().unwrap();
            let payload = timeout(Duration::from_secs(3), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let payload: Value = serde_json::from_str(&payload.into_text().unwrap()).unwrap();
            if exact && index == 1 {
                assert_eq!(payload["previous_response_id"], "resp_compat_first");
            }
            let id = if index == 0 {
                "resp_compat_first"
            } else {
                "resp_compat_second"
            };
            socket
                .send(Message::Text(completed_websocket_response(id, 2, 1).into()))
                .await
                .unwrap();
        }
        openings.lock().unwrap().clone()
    });
    let mut first = request();
    first.beta_features = first_beta.map(str::to_owned);
    first.codex_window_id = first_window.map(str::to_owned);
    let initial = timeout(
        Duration::from_secs(3),
        backend.create_response(
            &first,
            request_context("compat-first", Some("compat-account")),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(initial.transport, CodexBackendTransport::WebSocket);
    let mut second = first.clone();
    second.beta_features = second_beta.map(str::to_owned);
    second.codex_window_id = second_window.map(str::to_owned);
    if exact {
        second.set_previous_response_id(Some("resp_compat_first".into()));
        second.previous_response_scope = Some(PreviousResponseScope::ConnectionLocal);
    }
    let result = timeout(
        Duration::from_secs(3),
        backend.create_response(
            &second,
            request_context("compat-second", Some("compat-account")),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.transport, CodexBackendTransport::WebSocket);
    assert_eq!(
        result.websocket_pool_decision.unwrap().kind() == "reuse",
        reuse
    );
    let openings = server.await.unwrap();
    assert_eq!(openings.len(), if reuse { 1 } else { 2 });
    if !reuse {
        assert_eq!(
            openings[1]
                .get("x-codex-beta-features")
                .map(|v| v.to_str().unwrap()),
            Some(second_beta.unwrap_or("remote_compaction_v2"))
        );
        if let Some(window) = second_window {
            assert!(
                openings[1]["x-codex-window-id"]
                    .to_str()
                    .unwrap()
                    .ends_with(window)
            );
        }
    }
}

#[tokio::test]
async fn changed_beta_uses_a_new_opening() {
    assert_opening_compatibility(
        Some("feature-a"),
        Some("feature-b"),
        None,
        None,
        false,
        false,
    )
    .await;
}

#[tokio::test]
async fn changed_window_uses_a_new_opening() {
    assert_opening_compatibility(
        None,
        None,
        Some("compat-thread:0"),
        Some("compat-thread:1"),
        false,
        false,
    )
    .await;
}

#[tokio::test]
async fn adding_or_removing_beta_uses_a_new_opening() {
    assert_opening_compatibility(None, Some("feature-a"), None, None, false, false).await;
    assert_opening_compatibility(Some("feature-a"), None, None, None, false, false).await;
}

#[tokio::test]
async fn equivalent_beta_sets_reuse_the_same_opening() {
    assert_opening_compatibility(
        Some("feature-b,feature-a"),
        Some("feature-a, feature-b,feature-a"),
        Some("compat-thread:0"),
        Some("compat-thread:0"),
        true,
        false,
    )
    .await;
}

#[tokio::test]
async fn unchanged_opening_attributes_reuse_the_same_socket() {
    assert_opening_compatibility(None, None, None, None, true, false).await;
}

#[tokio::test]
async fn default_and_explicit_default_beta_share_the_same_opening() {
    assert_opening_compatibility(None, Some("remote_compaction_v2"), None, None, true, false).await;
}

#[tokio::test]
async fn exact_continuation_retains_owner_after_beta_and_window_change() {
    assert_opening_compatibility(
        Some("feature-a"),
        Some("feature-b"),
        Some("compat-thread:0"),
        Some("compat-thread:1"),
        true,
        true,
    )
    .await;
}

async fn receive_messages(
    messages: Vec<String>,
    streaming: bool,
) -> Result<String, CodexClientError> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend = backend(listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_codex_test_websocket(stream).await;
        assert!(socket.next().await.unwrap().unwrap().is_text());
        for message in messages {
            if socket.send(Message::Text(message.into())).await.is_err() {
                break;
            }
        }
    });
    let result = timeout(Duration::from_secs(3), async {
        if streaming {
            let mut response = backend
                .create_response_stream(
                    &request(),
                    request_context("compat-events", Some("compat-account")),
                )
                .await?;
            let mut body = String::new();
            while let Some(frame) = response.body.next().await {
                body.push_str(std::str::from_utf8(&frame?).unwrap());
            }
            Ok(body)
        } else {
            backend
                .create_response(
                    &request(),
                    request_context("compat-events", Some("compat-account")),
                )
                .await
                .map(|r| r.body)
        }
    })
    .await
    .unwrap();
    server.await.unwrap();
    result
}

#[tokio::test]
async fn concatenated_text_tool_and_terminal_events_preserve_wire_order() {
    let events = [
        r#"{"type":"response.output_text.delta","delta":"keep  two spaces"}"#.to_owned(),
        r#"{"type":"response.function_call_arguments.delta","item_id":"fc_compat","delta":"{\"x\": 1}"}"#.to_owned(),
        completed_websocket_response("resp_compat", 2, 1),
    ];
    for streaming in [false, true] {
        let body = receive_messages(vec![events.join("\n")], streaming)
            .await
            .unwrap();
        let mut previous = 0;
        for event in &events {
            let position = body.find(event).expect("preserve original event JSON");
            assert!(position >= previous);
            previous = position + event.len();
        }
    }
}

#[tokio::test]
async fn malformed_event_cannot_be_hidden_by_a_later_success() {
    for streaming in [false, true] {
        let error = receive_messages(
            vec![
                "{broken-business-event".into(),
                completed_websocket_response("resp_compat", 2, 1),
            ],
            streaming,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("invalid event JSON"));
    }
}

#[tokio::test]
async fn malformed_tail_and_non_events_are_not_partially_recovered() {
    for payload in [
        "{\"type\":\"response.output_text.delta\",\"delta\":\"not-delivered\"}{broken",
        "{\"type\":\"response.created\"}{}",
        "{\"type\":\"response.created\"}{\"type\":\"bad\\ntype\"}",
    ] {
        let error = receive_messages(vec![payload.into()], true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid event JSON"));
    }
}

#[tokio::test]
async fn concatenated_document_limit_accepts_sixteen_and_rejects_seventeen() {
    for count in [16, 17] {
        let mut events =
            vec![r#"{"type":"response.output_text.delta","delta":"x"}"#.to_owned(); count - 1];
        events.push(completed_websocket_response("resp_compat", 2, 1));
        let result = receive_messages(vec![events.join("")], true).await;
        assert_eq!(result.is_ok(), count == 16);
    }
}

#[tokio::test]
async fn terminal_tail_retires_socket_instead_of_reusing_ambiguous_state() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend = backend(listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut first = accept_codex_test_websocket(stream).await;
        first.next().await.unwrap().unwrap();
        first
            .send(Message::Text(
                format!(
                    "{}{}",
                    completed_websocket_response("resp_tail", 2, 1),
                    r#"{"type":"response.output_text.delta","delta":"must-not-leak"}"#
                )
                .into(),
            ))
            .await
            .unwrap();
        let (stream, _) = timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut second = accept_codex_test_websocket(stream).await;
        second.next().await.unwrap().unwrap();
        second
            .send(Message::Text(
                completed_websocket_response("resp_fresh", 2, 1).into(),
            ))
            .await
            .unwrap();
    });
    let first = backend
        .create_response(
            &request(),
            request_context("compat-tail", Some("compat-account")),
        )
        .await
        .unwrap();
    assert!(!first.body.contains("must-not-leak"));
    let second = timeout(
        Duration::from_secs(3),
        backend.create_response(
            &request(),
            request_context("compat-fresh", Some("compat-account")),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(second.websocket_pool_decision.unwrap().kind(), "reuse");
    server.await.unwrap();
}

async fn assert_lifecycle_delivery(prefetch_bytes: u64, updated_prefetch: Option<u64>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tuning =
        gateway_core::runtime::RequestTuningHandle::new(gateway_core::routing::RequestTuning {
            stream_prefetch_bytes: prefetch_bytes,
            websocket_stream_idle_timeout_ms: 1000,
            ..gateway_core::routing::RequestTuning::defaults()
        });
    let backend = backend(listener.local_addr().unwrap()).with_request_tuning(tuning.clone());
    let (accepted, accept_received) = tokio::sync::oneshot::channel();
    let (open, opening) = tokio::sync::oneshot::channel();
    let (sent, received) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        accepted.send(()).unwrap();
        opening.await.unwrap();
        let mut socket = accept_codex_test_websocket(stream).await;
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(
                json!({"type":"response.created","response":{"id":"resp_lifecycle"}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        sent.send(()).unwrap();
        released.await.unwrap();
        socket
            .send(Message::Text(
                completed_websocket_response("resp_lifecycle", 2, 1).into(),
            ))
            .await
            .unwrap();
    });
    let mut task = tokio::spawn(async move {
        backend
            .create_response_stream(
                &request(),
                request_context("compat-lifecycle", Some("compat-account")),
            )
            .await
    });
    timeout(Duration::from_secs(3), accept_received)
        .await
        .unwrap()
        .unwrap();
    if let Some(updated_prefetch) = updated_prefetch {
        let mut updated = tuning.load();
        updated.stream_prefetch_bytes = updated_prefetch;
        tuning.publish(updated);
    }
    open.send(()).unwrap();
    timeout(Duration::from_secs(3), received)
        .await
        .unwrap()
        .unwrap();
    let mut response = if prefetch_bytes == 0 {
        // The upstream cannot send completion until the first event is delivered.
        let mut response = timeout(Duration::from_secs(1), &mut task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let first = timeout(Duration::from_secs(1), response.body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            std::str::from_utf8(&first)
                .unwrap()
                .contains("response.created")
        );
        release.send(()).unwrap();
        response
    } else {
        assert!(
            timeout(Duration::from_millis(200), &mut task)
                .await
                .is_err()
        );
        release.send(()).unwrap();
        let mut response = task.await.unwrap().unwrap();
        assert!(
            std::str::from_utf8(&response.body.next().await.unwrap().unwrap())
                .unwrap()
                .contains("response.created")
        );
        response
    };
    while let Some(frame) = response.body.next().await {
        frame.unwrap();
    }
    server.await.unwrap();
}

#[tokio::test]
async fn prefetch_zero_releases_lifecycle_before_further_upstream_events() {
    assert_lifecycle_delivery(0, None).await;
}

#[tokio::test]
async fn nonzero_prefetch_holds_lifecycle_below_the_configured_threshold() {
    for prefetch_bytes in [30 * 1024, 60 * 1024, 120 * 1024, 128 * 1024] {
        assert_lifecycle_delivery(prefetch_bytes, None).await;
    }
}

#[tokio::test]
async fn prefetch_policy_is_frozen_before_websocket_opening() {
    for (initial, updated) in [(0, 128 * 1024), (128 * 1024, 0)] {
        assert_lifecycle_delivery(initial, Some(updated)).await;
    }
}

async fn assert_connection_limit_before_delivery(prefetch_bytes: u64, send_created: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend = backend_with_prefetch(listener.local_addr().unwrap(), prefetch_bytes);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_codex_test_websocket(stream).await;
        socket.next().await.unwrap().unwrap();
        if send_created {
            socket
                .send(Message::Text(
                    json!({"type":"response.created","response":{"id":"resp_limit"}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        }
        socket.send(Message::Text(json!({"type":"error","status":400,"error":{"code":"websocket_connection_limit_reached","type":"invalid_request_error","message":"synthetic expired connection"}}).to_string().into())).await.unwrap();
    });
    let result = timeout(
        Duration::from_secs(3),
        backend.create_response_stream(
            &request(),
            request_context("compat-limit", Some("compat-account")),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        result,
        Err(CodexClientError::WebSocket(
            CodexWebSocketExchangeError::ConnectionLimitReached(_)
        ))
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn nonzero_prefetch_keeps_connection_limit_recoverable_after_created() {
    assert_connection_limit_before_delivery(128 * 1024, true).await;
}

#[tokio::test]
async fn prefetch_zero_keeps_immediate_connection_limit_recoverable() {
    assert_connection_limit_before_delivery(0, false).await;
}
