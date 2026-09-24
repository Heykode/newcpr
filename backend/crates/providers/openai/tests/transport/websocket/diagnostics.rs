use super::*;
use gateway_core::diagnostics::TraceContext;
use provider_openai::transport::websocket::CodexWebSocketExchangeError;

#[tokio::test]
async fn liveness_exit_preserves_capture_clocks_without_inventing_an_upstream_close() {
    for reuse in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = accept_codex_test_websocket_with(stream, |_, response| {
                response
                    .headers_mut()
                    .insert("x-codex-primary-used-percent", "12".parse().unwrap());
            })
            .await;
            websocket.next().await.unwrap().unwrap();
            if reuse {
                websocket
                    .send(Message::Text(
                        completed_websocket_response("resp_liveness_warmup", 1, 1).into(),
                    ))
                    .await
                    .unwrap();
                websocket.next().await.unwrap().unwrap();
            }
            for raw in [
                rate_limit_event(43),
                json!({"type":"response.created","response":{"id":"resp_liveness","model":"gpt-5.5"}}).to_string(),
                json!({"type":"response.output_text.delta","delta":"private-response-body"}).to_string(),
            ] {
                websocket.send(Message::Text(raw.into())).await.unwrap();
            }
            futures::future::pending::<()>().await;
        });
        let backend = CodexBackendClient::new(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            format!("http://{address}"),
            test_wire_profile(),
        )
        .with_websocket_pool(Arc::new(CodexWebSocketPool::with_config(
            CodexWebSocketPoolConfig {
                maintenance_interval: None,
                ping_interval: None,
                liveness_timeout: Some(Duration::from_secs(5)),
                ..Default::default()
            },
        )));
        let mut request =
            websocket_only_request(codex_request("gpt-5.5", "private-request-body", Vec::new()));
        request.local_conversation_id = Some("liveness-exit".to_owned());
        let opening_time = if reuse {
            let mut response = backend
                .create_response_stream(
                    &request,
                    request_context("req_liveness_warmup", Some("chatgpt-account")),
                )
                .await
                .unwrap();
            let captured = response.rate_limit_observed_at;
            while let Some(chunk) = response.body.next().await {
                chunk.unwrap();
            }
            Some(captured)
        } else {
            None
        };
        let trace = TraceContext::new("req_liveness_exit");
        let attempt = trace.attempt(1);
        let mut response = backend
            .create_response_stream(
                &request,
                request_context("req_liveness_exit", Some("chatgpt-account")).with_trace(&attempt),
            )
            .await
            .unwrap();
        if let Some(captured) = opening_time {
            assert_eq!(response.rate_limit_observed_at, captured);
        }
        while let Some(chunk) = response.body.next().await {
            if String::from_utf8_lossy(&chunk.unwrap()).contains("response.output_text.delta") {
                break;
            }
        }
        let updates = response.rate_limit_updates.as_ref().unwrap();
        let captured = updates.lock().await[0].observed_at;
        assert!(captured >= response.rate_limit_observed_at);
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(6)).await;
        let error = loop {
            match response.body.next().await {
                Some(Ok(_)) => {}
                Some(Err(CodexClientError::WebSocket(error))) => break error,
                result => panic!("expected liveness failure, got {result:?}"),
            }
        };
        tokio::time::resume();
        server.abort();
        let _ = server.await;
        assert_eq!(updates.lock().await[0].observed_at, captured);
        assert!(error.close_before_terminal().is_none());
        let observation = error.connection_observation().unwrap();
        assert_eq!(observation.exit_reason(), "liveness_timeout");
        assert!(observation.age_ms() >= 5_000);
        assert!(observation.idle_ms() >= 5_000);
        let mut cause = &error;
        loop {
            match cause {
                CodexWebSocketExchangeError::PostSendAmbiguous {
                    source: Some(source),
                    ..
                }
                | CodexWebSocketExchangeError::ReusedConnectionDiedBeforeFirstEvent {
                    source: Some(source),
                    ..
                }
                | CodexWebSocketExchangeError::ConnectionObserved { source, .. } => {
                    cause = source;
                }
                CodexWebSocketExchangeError::StreamEndedBeforeTerminal {
                    reason,
                    timeout,
                    last_event_type,
                } => {
                    assert_eq!(*reason, "liveness_timeout");
                    assert_eq!(*timeout, Some(Duration::from_secs(5)));
                    assert_eq!(
                        last_event_type.as_deref(),
                        Some("response.output_text.delta")
                    );
                    break;
                }
                error => panic!("unexpected liveness cause: {error}"),
            }
        }
        let snapshot = trace.snapshot().unwrap();
        let eof = snapshot["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["stage"] == "upstream.eof")
            .unwrap();
        assert_eq!(eof["data"]["failureReason"], "liveness_timeout");
        assert!(!snapshot.to_string().contains("private-response-body"));
        assert!(!snapshot.to_string().contains("private-request-body"));
    }
}

#[tokio::test]
async fn diagnostics_preserve_abrupt_disconnect_facts_without_response_content() {
    for (reuse, send_event) in [(false, false), (false, true), (true, false), (true, true)] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = accept_codex_test_websocket(stream).await;
            websocket.next().await.unwrap().unwrap();
            if reuse {
                websocket
                    .send(Message::Text(
                        completed_websocket_response("resp_warmup", 1, 1).into(),
                    ))
                    .await
                    .unwrap();
                websocket.next().await.unwrap().unwrap();
            }
            if send_event {
                websocket
                    .send(Message::Text(
                        json!({"type": "codex.response.metadata", "private": "private-response-body"})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
            }
            // Drop the transport without the WebSocket close handshake.
        });
        let trace = TraceContext::new("req_abrupt_disconnect");
        let attempt = trace.attempt(1);
        let mut request = codex_request("gpt-5.5", "private-request-body", Vec::new());
        request.set_previous_response_id(Some("resp_previous".to_owned()));
        request.previous_response_scope = Some(PreviousResponseScope::Persisted);
        request.force_http_sse = false;
        request.local_conversation_id = Some("diagnostics-conversation".to_owned());
        let request = websocket_only_request(request);
        let backend = CodexBackendClient::new(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            format!("http://{addr}"),
            test_wire_profile(),
        )
        .with_websocket_pool(Arc::new(CodexWebSocketPool::new(Duration::from_mins(1))));
        if reuse {
            backend
                .create_response(
                    &request,
                    request_context("req_warmup", Some("chatgpt-account")),
                )
                .await
                .expect("warmup response must complete");
        }
        let result = backend
            .create_response_stream(
                &request,
                request_context("req_abrupt_disconnect", Some("chatgpt-account"))
                    .with_trace(&attempt),
            )
            .await;
        let mut failed = result.is_err();
        if let Ok(response) = result {
            let mut body = response.body;
            while let Some(chunk) = body.next().await {
                if chunk.is_err() {
                    failed = true;
                    break;
                }
            }
        }
        assert!(failed, "an abrupt disconnect must fail the exchange");
        server.await.unwrap();
        let snapshot = trace.snapshot().unwrap();
        let events = snapshot["events"].as_array().unwrap();
        let connection = events
            .iter()
            .find(|event| event["stage"] == "upstream.connection")
            .unwrap();
        let failure = events
            .iter()
            .find(|event| event["stage"] == "upstream.read.failed")
            .unwrap();
        assert_eq!(
            failure["data"]["failureReason"],
            "reset_without_closing_handshake"
        );
        assert_eq!(
            failure["data"]["connectionId"],
            connection["data"]["connectionId"]
        );
        assert_eq!(failure["data"]["reused"], reuse);
        assert_eq!(
            failure["data"]["lastEventType"],
            if send_event {
                json!("codex.response.metadata")
            } else {
                Value::Null
            }
        );
        assert!(failure["data"]["connectionAgeMs"].is_u64());
        assert!(failure["data"]["connectionIdleMs"].is_u64());
        assert!(!snapshot.to_string().contains("private-response-body"));
        assert!(!snapshot.to_string().contains("private-request-body"));
    }
}
