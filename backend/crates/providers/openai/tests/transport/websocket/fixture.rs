use super::*;

#[tokio::test]
async fn websocket_only_fixture_should_wait_for_slow_opening() {
    check_slow_opening(false).await;
}

#[tokio::test]
async fn pooled_websocket_fixture_should_wait_for_slow_opening() {
    check_slow_opening(true).await;
}

async fn check_slow_opening(pooled: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut websocket = accept_codex_test_websocket(stream).await;
        tokio::select! {
            payload = websocket.next() => {
                assert!(
                    matches!(payload, Some(Ok(Message::Text(_)))),
                    "WS-only fixture must receive response.create after a slow opening: {payload:?}"
                );
            }
            fallback = listener.accept() => {
                let (mut fallback, _) = fallback.unwrap();
                let request = read_http_request(&mut fallback).await;
                panic!("WS-only fixture received fallback: {}", request.lines().next().unwrap());
            }
        }
        websocket
            .send(Message::Text(
                completed_websocket_response("resp_slow_fixture", 1, 1).into(),
            ))
            .await
            .unwrap();
    });
    let response = timeout(Duration::from_secs(5), async {
        if pooled {
            let backend = CodexBackendClient::new(
                reqwest::Client::builder().no_proxy().build().unwrap(),
                format!("http://{address}"),
                test_wire_profile(),
            )
            .with_websocket_pool(Arc::new(CodexWebSocketPool::new(Duration::from_mins(1))));
            backend
                .create_response(
                    &websocket_only_request(pooled_websocket_request("slow-fixture")),
                    request_context("req_slow_fixture", Some("chatgpt-account")),
                )
                .await
        } else {
            execute_response_create_request(&prepared_websocket_request(&format!(
                "http://{address}"
            )))
            .await
        }
    })
    .await
    .expect("slow-opening fixture must not hang");
    server
        .await
        .expect("WS-only fixture should stay on its socket");
    let response = response.expect("slow-opening WebSocket response");
    assert_eq!(response.transport, CodexBackendTransport::WebSocket);
    assert!(response.body.contains("resp_slow_fixture"));
}

#[test]
fn websocket_only_fixture_should_preserve_the_wire_body() {
    for request in [
        codex_request("gpt-test", "be brief", Vec::new()),
        pooled_websocket_request("fixture-body"),
    ] {
        let body = request.body().clone();
        let request = websocket_only_request(request);
        assert_eq!(request.body(), &body);
    }
}
