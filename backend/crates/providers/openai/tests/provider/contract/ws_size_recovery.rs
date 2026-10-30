use super::*;

// Source: official 12771aca, provider failure recovery before semantic output.
#[tokio::test]
async fn size_close_retries_within_budget_then_falls_back_on_same_account() {
    timeout(Duration::from_secs(30), async {
        for budget in [0, 2] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_websocket_close").await;
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                for _ in 0..=budget {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut ws = accept_codex_test_websocket(socket).await;
                    ws.next().await.unwrap().unwrap();
                    ws.close(Some(CloseFrame {
                        code: CloseCode::Size,
                        reason: "".into(),
                    }))
                    .await
                    .unwrap();
                }
                // A real failure fallback remembers HTTP for subsequent turns.
                for _ in 0..2 {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let request = capture_http_request(&mut socket).await;
                    assert!(request.starts_with(b"POST /codex/responses "));
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{CAPTURE_COMPLETED_SSE}",
                        CAPTURE_COMPLETED_SSE.len()
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                }
            });
            let provider = provider_with_base_url_and_retry_budget(&store, base_url, budget);
            let operation = || {
                Operation::Generate(
                    generate_with_persisted_session_context(
                        "acct_websocket_close", "size-conversation", "size-session", "size-thread",
                    ),
                )
            };
            for index in 0..=budget {
                let mut attempt = context("req_size_recovery", CancellationToken::new());
                if let Some(retry) = NonZeroU32::new(index) {
                    attempt = attempt.with_transport(AttemptTransport::Retry(retry));
                }
                let mut stream = provider.execute(planned_request("openai", operation()), attempt).await.unwrap();
                assert_eq!(stream.metadata().transport().as_str(), "websocket");
                let error = loop {
                    match stream.next().await {
                        Some(Ok(_)) => {},
                        Some(Err(error)) => break error,
                        None => panic!("1009 must produce a failure"),
                    }
                };
                assert_eq!(error.send_state(), UpstreamSendState::Ambiguous);
                assert!(!error.replay_is_safe());
                assert!(!provider_openai::openai_failure_affects_account_score(&error));
                assert_eq!(store.account("acct_websocket_close").unwrap().credential_state(), CredentialState::Ready);
                if index < budget {
                    assert!(matches!(error.pre_delivery_retry(), Some(PreDeliveryRetry::SameAccountTransportRetry { retry_index, .. }) if retry_index.get() == index + 1));
                } else {
                    assert_eq!(error.pre_delivery_retry(), Some(PreDeliveryRetry::SameAccountTransportFallback));
                }
            }
            for attempt in [fallback_transport_context("req_size_recovery"), context("req_after_size_recovery", CancellationToken::new())] {
                let mut stream = provider.execute(planned_request("openai", operation()), attempt).await.unwrap();
                assert_eq!(stream.metadata().transport().as_str(), "http_sse");
                let mut completed = false;
                while let Some(event) = stream.next().await {
                    completed |= event.unwrap().wire_event().is_some_and(|wire| wire.event_type() == Some("response.completed"));
                }
                assert!(completed);
            }
            server.await.unwrap();
        }
    }).await.unwrap();
}

#[tokio::test]
async fn size_close_on_exact_continuation_does_not_request_http_replay() {
    timeout(Duration::from_secs(15), async {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_websocket_close").await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = accept_codex_test_websocket(socket).await;
            ws.next().await.unwrap().unwrap();
            generate_compat::send_completion(&mut ws, "resp_size_seed").await;
            let request = ws.next().await.unwrap().unwrap();
            let body: Value = serde_json::from_str(request.to_text().unwrap()).unwrap();
            assert_eq!(body["previous_response_id"], "resp_size_seed");
            ws.close(Some(CloseFrame {
                code: CloseCode::Size,
                reason: "".into(),
            }))
            .await
            .unwrap();
        });
        let provider = provider_with_base_url(&store, base_url);
        let mut seed = provider
            .execute(
                planned_request(
                    "openai",
                    Operation::Generate(websocket_fixture_request(
                        generate_with_persisted_session_context(
                            "acct_websocket_close",
                            "size-root",
                            "size-session",
                            "size-turn",
                        ),
                    )),
                ),
                context("req_size_seed", CancellationToken::new()),
            )
            .await
            .unwrap();
        let mut state = None;
        while let Some(event) = seed.next().await {
            if let Some(update) = event.unwrap().session_update() {
                state = Some(update.clone());
            }
        }
        drop(seed);
        let state = state.unwrap();
        assert_eq!(state.payload()["continuation_scope"], "connection_local");
        let continuation = GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object(
                "openai",
                json!({
                    "model":"gpt-5.4", "input":"continue", "previous_response_id":"resp_size_seed"
                })
                .as_object()
                .unwrap()
                .clone(),
            )
            .unwrap(),
        )
        .with_provider_session_state(state);
        let mut stream = provider
            .execute(
                planned_request("openai", Operation::Generate(continuation)),
                pinned_continuation_context(
                    "req_size_exact",
                    "acct_websocket_close",
                    "resp_size_seed",
                    "resp_size_seed",
                    1,
                    ContinuationAttempt::Native,
                ),
            )
            .await
            .unwrap();
        let error = loop {
            match stream.next().await {
                Some(Ok(_)) => {}
                Some(Err(error)) => break error,
                None => panic!("exact continuation must return the size error"),
            }
        };
        assert_eq!(error.kind().as_str(), "message_too_big");
        assert_eq!(error.pre_delivery_retry(), None);
        assert_eq!(error.upstream_status(), Some(413));
        server.await.unwrap();
    })
    .await
    .unwrap();
}
