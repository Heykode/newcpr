use super::*;

#[tokio::test]
async fn disable_fast_rewrites_only_the_top_level_known_fast_tier() {
    for (disable_fast, requested, expected) in [
        (false, "priority", "priority"),
        (true, "priority", "default"),
        (true, "fast", "default"),
        (true, "auto", "auto"),
    ] {
        let captured = capture_scoped_http_request_with_policy(
            "req_disable_fast",
            "acct_scope_same",
            "acct_scope_same",
            json!({
                "model":"gpt-5.4",
                "input":"hello",
                "service_tier":requested,
                "metadata":{"service_tier":"priority"}
            })
            .as_object()
            .unwrap()
            .clone(),
            Map::new(),
            Default::default(),
            disable_fast,
        )
        .await;
        let body = captured_request_body(&captured);
        assert_eq!(body["service_tier"], expected);
        assert_eq!(body["metadata"]["service_tier"], "priority");
    }
}

#[tokio::test]
async fn standalone_item_ids_are_adapted_without_rewriting_tool_links_or_encrypted_history() {
    let (input, expected) = item_id_fixture();
    let captured = capture_scoped_http_request(
        "req_safe_ids",
        "acct_scope_same",
        "acct_scope_same",
        json!({"model":"gpt-5.4","input":input})
            .as_object()
            .unwrap()
            .clone(),
        Map::new(),
    )
    .await;
    assert_eq!(captured_request_body(&captured)["input"], expected);
}

pub(super) fn item_id_fixture() -> (Value, Value) {
    let input = json!([
        {"type":"message","id":"local-message","role":"user","content":"hello"},
        {"type":"function_call","id":"local-function","call_id":"call-original",
            "name":"lookup","arguments":"{}"},
        {"type":"function_call_output","call_id":"call-original","output":"result"},
        {"type":"custom_tool_call","id":"local-custom","call_id":"custom-original",
            "name":"code","input":"print(1)"},
        {"type":"custom_tool_call_output","call_id":"custom-original","output":"1"},
        {"type":"message","id":"msg_valid","role":"assistant","content":"done"},
        {"type":"reasoning","id":"reasoning-id","encrypted_content":"ciphertext","summary":[]},
        {"type":"compaction","id":"compaction-id","encrypted_content":"compact-ciphertext"},
        {"type":"function_call","id":"encrypted-call-id","call_id":"call-encrypted",
            "name":"lookup","arguments":"{}","encrypted_function_args":"ciphertext"}
    ]);
    let mut expected = input.clone();
    for index in [0, 1, 3] {
        expected[index].as_object_mut().unwrap().remove("id");
    }
    (input, expected)
}

#[tokio::test]
async fn referenced_incomplete_and_unknown_item_ids_remain_untouched() {
    for input in [
        json!([
            {"type":"function_call","id":"local-call","call_id":"call-original","name":"lookup","arguments":"{}"},
            {"type":"item_reference","id":"local-call"}
        ]),
        json!([
            {"type":"function_call","id":"local-call","name":"lookup","arguments":"{}"},
            {"type":"custom_tool_call","id":"local-custom","call_id":"call-original"},
            {"type":"message","id":"local-message","role":"user"},
            {"type":"message","id":"local-image","role":"user","content":[{"type":"input_image","image_url":"data:opaque"}]}
        ]),
        json!([
            {"type":"function_call","id":"local-call","call_id":"call-original","name":"lookup","arguments":"{}"},
            {"type":"future_item","item_id":"local-call"}
        ]),
    ] {
        let captured = capture_scoped_http_request(
            "req_preserve_ids",
            "acct_scope_same",
            "acct_scope_same",
            json!({"model":"gpt-5.4","input":input})
                .as_object()
                .unwrap()
                .clone(),
            Map::new(),
        )
        .await;
        assert_eq!(captured_request_body(&captured)["input"], input);
    }
}

#[tokio::test]
async fn explicit_continuation_preserves_even_invalid_item_ids_for_the_original_owner() {
    let (input, _) = item_id_fixture();
    let captured = capture_scoped_http_request(
        "req_continuation_ids",
        "acct_scope_same",
        "acct_scope_same",
        json!({"model":"gpt-5.4","input":input,"previous_response_id":"resp_existing"})
            .as_object()
            .unwrap()
            .clone(),
        Map::new(),
    )
    .await;
    let body = captured_request_body(&captured);
    assert_eq!(body["input"], input);
    assert_eq!(body["previous_response_id"], "resp_existing");
}

#[test]
fn content_fallback_normalizes_text_formats_and_preserves_explicit_affinity() {
    let provider = provider(&Arc::new(MemoryAccountStore::default()));
    let key = |input: Value, session: Option<&str>| {
        let mut body = json!({"input":input,"instructions":"stable prefix","service_tier":"fast"});
        if let Some(session) = session {
            body["session_id"] = json!(session);
        }
        let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object("openai", body.as_object().unwrap().clone()).unwrap(),
        ));
        provider
            .request_observation(&operation, &ClientApiKeyId::new("client").unwrap())
            .continuation
            .affinity_hash
            .expect("existing content or session affinity")
    };
    let string_key = key(json!("hello"), None);
    // String-only requests now include the first user text. This intentionally
    // replaces the old instructions-only fallback, while explicit IDs stay stable.
    assert_ne!(string_key, key(json!("different"), None));
    let list_key = key(json!([{"role":"user","content":"hello"}]), None);
    assert_eq!(string_key, list_key);
    assert_eq!(
        list_key,
        key(
            json!([{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}]),
            None
        )
    );
    assert_ne!(
        list_key,
        key(json!([{"role":"user","content":"different"}]), None)
    );
    let session_key = key(json!("hello"), Some("explicit-session"));
    assert_eq!(
        session_key,
        key(
            json!([{"role":"user","content":"hello"}]),
            Some("explicit-session")
        )
    );
}

#[tokio::test]
async fn message_too_big_close_before_first_event_is_request_scoped_on_new_and_reused_sockets() {
    const ACCOUNT: &str = "acct_provider_contract";
    for reused in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, ACCOUNT).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (no_replay_tx, no_replay_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_codex_test_websocket(socket).await;
            if reused {
                socket.next().await.unwrap().unwrap();
                send_completion(&mut socket, "resp_seed").await;
            }
            socket.next().await.unwrap().unwrap();
            socket
                .close(Some(CloseFrame {
                    code: CloseCode::Size,
                    reason: "opaque upstream size detail".into(),
                }))
                .await
                .unwrap();
            assert!(
                timeout(Duration::from_millis(150), listener.accept())
                    .await
                    .is_err(),
                "1009 must not replay on a new WS or HTTP connection"
            );
            no_replay_tx.send(()).unwrap();
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_codex_test_websocket(socket).await;
            socket.next().await.unwrap().unwrap();
            send_completion(&mut socket, "resp_next_request").await;
        });
        let provider = provider_with_base_url_and_retry_budget(&store, base_url, 1);
        let generate = || {
            generate_with_persisted_session_context(
                ACCOUNT,
                "conversation-size",
                "session-size",
                "thread-size",
            )
        };
        if reused {
            let mut seed = provider
                .execute(
                    planned_request(
                        "openai",
                        Operation::Generate(websocket_fixture_request(generate())),
                    ),
                    context("req_size_seed", CancellationToken::new()),
                )
                .await
                .unwrap();
            while let Some(event) = timeout(Duration::from_secs(5), seed.next()).await.unwrap() {
                event.expect("seed completed");
            }
        }
        let mut stream = provider
            .execute(
                planned_request(
                    "openai",
                    Operation::Generate(websocket_fixture_request(generate())),
                ),
                context("req_size", CancellationToken::new()),
            )
            .await
            .unwrap();
        let error = loop {
            match timeout(Duration::from_secs(5), stream.next())
                .await
                .unwrap()
            {
                Some(Ok(event)) => assert!(!event.has_client_event()),
                Some(Err(error)) => break error,
                None => panic!("remote 1009 must fail this request"),
            }
        };
        assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
        assert_eq!(error.upstream_status(), Some(413));
        assert_eq!(error.send_state(), UpstreamSendState::Ambiguous);
        assert!(!error.replay_is_safe());
        assert!(!error.allows_pre_delivery_retry());
        assert!(!provider_openai::openai_failure_affects_account_score(
            &error
        ));
        assert_eq!(
            store.account(ACCOUNT).unwrap().credential_state(),
            CredentialState::Ready
        );
        let detail = error.client_visible_upstream_error().unwrap();
        assert_eq!(detail.code(), Some("message_too_big"));
        assert_eq!(detail.error_type(), Some("invalid_request_error"));
        assert_eq!(detail.message(), "upstream websocket message too big");
        let response = error.client_visible_upstream_response().unwrap();
        assert_eq!(response.status(), 413);
        assert_eq!(
            response.content_type(),
            Some(b"application/json".as_slice())
        );
        assert_eq!(
            serde_json::from_slice::<Value>(response.body()).unwrap(),
            json!({"error":{
                "message":"upstream websocket message too big",
                "code":"message_too_big",
                "type":"invalid_request_error"
            }})
        );
        let raw: Value =
            serde_json::from_str(error.raw_upstream_error().unwrap().as_str()).unwrap();
        assert_eq!(raw["code"], 1009);
        assert_eq!(raw["reason"], "opaque upstream size detail");
        assert!(!format!("{error:?}").contains("opaque upstream size detail"));
        assert!(stream.next().await.is_none());
        drop(stream);
        timeout(Duration::from_secs(5), no_replay_rx)
            .await
            .unwrap()
            .unwrap();

        // A request error must not exhaust the session's one-failure WS budget.
        let mut next = provider
            .execute(
                planned_request("openai", Operation::Generate(generate())),
                context("req_after_size", CancellationToken::new()),
            )
            .await
            .unwrap();
        assert_eq!(next.metadata().transport().as_str(), "websocket");
        let mut completed = false;
        while let Some(event) = timeout(Duration::from_secs(5), next.next()).await.unwrap() {
            completed |= event
                .unwrap()
                .canonical_facts()
                .iter()
                .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
        }
        assert!(completed);
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

pub(super) async fn send_completion(
    socket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    response_id: &str,
) {
    for event in [
        json!({"type":"response.created","response":{"id":response_id,"model":"gpt-5.4"}}),
        json!({"type":"response.completed","response":{
            "id":response_id,"model":"gpt-5.4","status":"completed","output":[],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        }}),
    ] {
        socket
            .send(Message::Text(event.to_string().into()))
            .await
            .unwrap();
    }
}
