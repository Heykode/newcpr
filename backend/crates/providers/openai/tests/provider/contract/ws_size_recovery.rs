use super::*;

// Any text, including nonsemantic metadata, closes the early-size fallback window.
#[tokio::test]
async fn size_close_after_metadata_or_created_is_not_replayed() {
    for prelude in [
        json!({"type":"codex.rate_limits"}),
        json!({"type":"response.created","response":{"id":"resp_size","model":"gpt-5.4"}}),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_websocket_close").await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = accept_codex_test_websocket(socket).await;
            ws.next().await.unwrap().unwrap();
            ws.send(Message::Text(prelude.to_string().into()))
                .await
                .unwrap();
            ws.close(Some(CloseFrame {
                code: CloseCode::Size,
                reason: "".into(),
            }))
            .await
            .unwrap();
            assert!(
                timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let provider = provider_with_base_url_and_retry_budget(&store, url, 2);
        let operation = Operation::Generate(generate_with_persisted_session_context(
            "acct_websocket_close",
            "size-conversation",
            "size-session",
            "size-thread",
        ));
        let mut stream = provider
            .execute(
                planned_request("openai", operation),
                context("req_size_metadata", CancellationToken::new()),
            )
            .await
            .unwrap();
        let error = loop {
            match stream.next().await {
                Some(Ok(_)) => {}
                Some(Err(error)) => break error,
                None => panic!("size error required"),
            }
        };
        assert!(error.retry_is_prohibited());
        assert_eq!(error.pre_delivery_retry(), None);
        assert!(!error.replay_is_safe());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn size_close_on_exact_continuation_requests_client_replay_without_automatic_send() {
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
        assert_eq!(
            error.kind(),
            ProviderErrorKind::ContinuationRecoveryRequired
        );
        assert_eq!(
            error.continuation_recovery_disposition(),
            Some(ContinuationRecoveryDisposition::ClientReplayRequired)
        );
        assert_eq!(error.send_state(), UpstreamSendState::Ambiguous);
        assert_eq!(error.pre_delivery_retry(), None);
        assert_eq!(error.upstream_status(), None);
        assert_eq!(
            error.client_visible_upstream_error().unwrap().code(),
            Some("previous_response_not_found")
        );
        assert_eq!(
            error.client_visible_upstream_response().unwrap().status(),
            400
        );
        server.await.unwrap();
    })
    .await
    .unwrap();
}
