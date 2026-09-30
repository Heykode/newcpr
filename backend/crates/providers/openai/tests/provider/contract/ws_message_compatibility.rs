use super::*;

#[tokio::test]
async fn malformed_ws_event_is_diagnosed_without_replay_or_account_penalty() {
    const ACCOUNT_ID: &str = "acct_provider_contract";
    for partial_output in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, ACCOUNT_ID).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_codex_test_websocket(stream).await;
            assert!(socket.next().await.unwrap().unwrap().is_text());
            if partial_output {
                socket.send(Message::Text(json!({"type":"response.output_text.delta","delta":"already delivered","output_index":0,"content_index":0,"item_id":"msg_compat"}).to_string().into())).await.unwrap();
            }
            socket
                .send(Message::Text("{invalid-business-event".into()))
                .await
                .unwrap();
            // A malformed message is not evidence that the upstream did not execute.
            assert!(
                timeout(Duration::from_millis(250), listener.accept())
                    .await
                    .is_err()
            );
        });
        let provider = provider_with_base_url_and_retry_budget(&store, base_url, 3);
        let operation = Operation::Generate(websocket_fixture_request(
            generate_with_persisted_session_context(
                ACCOUNT_ID,
                "conversation-message-compat",
                "session-message-compat",
                "turn-message-compat",
            ),
        ));
        let mut stream = provider
            .execute(
                planned_request("openai", operation),
                context("req_message_compat", CancellationToken::new()).with_request_tuning(
                    gateway_core::routing::RequestTuning {
                        stream_prefetch_bytes: 0,
                        ..gateway_core::routing::RequestTuning::default()
                    },
                ),
            )
            .await
            .unwrap();
        let mut delivered = false;
        let error = timeout(Duration::from_secs(3), async {
            loop {
                match stream.next().await {
                    Some(Ok(event)) => {
                        delivered |= event.has_client_event();
                    }
                    Some(Err(error)) => break error,
                    None => panic!("malformed message must not become successful EOF"),
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(delivered, partial_output);
        assert!(!error.replay_is_safe());
        assert_eq!(error.pre_delivery_retry(), None);
        assert!(matches!(
            error.send_state(),
            UpstreamSendState::Sent | UpstreamSendState::Ambiguous
        ));
        let diagnostic = error.diagnostic().unwrap();
        assert_eq!(diagnostic.stage(), Some("decode"));
        assert_eq!(diagnostic.code(), Some("invalid_event_json"));
        assert!(!diagnostic.as_str().contains("invalid-business-event"));
        assert!(!provider_openai::openai_failure_affects_account_score(
            &error
        ));
        let account = store.account(ACCOUNT_ID).unwrap();
        assert!(account.enabled());
        assert_eq!(account.credential_state(), CredentialState::Ready);
        assert_eq!(account.last_error_reason(), None);
        server.await.unwrap();
    }
}
