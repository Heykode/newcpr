use super::*;

#[tokio::test]
async fn streamed_parameter_errors_are_request_scoped_for_http_and_ws() {
    for (code, param, message) in [
        (
            "invalid_prompt",
            serde_json::Value::Null,
            "unsupported prompt",
        ),
        (
            "unsupported_value",
            json!("reasoning.effort"),
            "This reasoning effort is not supported with this model",
        ),
        (
            "unsupported_parameter",
            json!("frequency_penalty"),
            "Parameter is not supported by this model",
        ),
    ] {
        for websocket in [false, true] {
            let failure = json!({
                "type":"response.failed",
                "response":{"id":"resp_prompt","status":"failed","error":{
                    "code":code,"message":message,"param":param,
                    "type":"invalid_request_error"
                }}
            });
            let http = MockServer::start().await;
            let (url, server) = if websocket {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let url = format!("http://{}", listener.local_addr().unwrap());
                let server = tokio::spawn(async move {
                    let (stream, _) = listener.accept().await.unwrap();
                    let mut socket = accept_codex_test_websocket(stream).await;
                    socket.next().await.unwrap().unwrap();
                    socket
                        .send(Message::Text(failure.to_string().into()))
                        .await
                        .unwrap();
                });
                (url, Some(server))
            } else {
                Mock::given(method("POST"))
                    .and(path("/codex/responses"))
                    .respond_with(ResponseTemplate::new(200).set_body_raw(
                        format!("event: response.failed\ndata: {failure}\n\n"),
                        "text/event-stream",
                    ))
                    .expect(1)
                    .mount(&http)
                    .await;
                (http.uri(), None)
            };
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            let provider = provider_with_base_url_and_retry_budget(&store, url, 0);
            let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object(
                    "openai",
                    json!({
                        "model":"gpt-5.4","input":"hello"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                )
                .unwrap()
                .with_context(Map::from_iter([(
                    "use_websocket".to_owned(),
                    json!(websocket),
                )])),
            ));
            let mut stream = provider
                .execute(
                    planned_request("openai", operation),
                    context("req_invalid_prompt", CancellationToken::new()),
                )
                .await
                .unwrap();
            let error = loop {
                match timeout(Duration::from_secs(5), stream.next())
                    .await
                    .unwrap()
                {
                    Some(Ok(_)) => {}
                    Some(Err(error)) => break error,
                    None => panic!("invalid_prompt must fail the request"),
                }
            };
            assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
            assert!(!error.allows_pre_delivery_retry());
            assert!(!provider_openai::openai_failure_affects_account_score(
                &error
            ));
            assert_eq!(
                error.client_visible_upstream_error().unwrap().code(),
                Some(code)
            );
            assert_eq!(
                store
                    .account("acct_provider_contract")
                    .unwrap()
                    .credential_state(),
                CredentialState::Ready
            );
            if let Some(server) = server {
                timeout(Duration::from_secs(5), server)
                    .await
                    .unwrap()
                    .unwrap();
            }
        }
    }
}

#[tokio::test]
async fn reasoning_replay_preserves_unrelated_fields_and_plaintext_on_http_and_ws() {
    let input = json!([
        {"type":"reasoning","id":"rs_replay","status":"completed","summary":[],"content":[{"type":"reasoning_text","text":"replay"}],"encrypted_content":"test-cipher","extension":{"status":"keep","content":[1]}},
        {"type":"reasoning","status":"completed","summary":[],"content":[],"encrypted_content":"test-cipher"},
        {"type":"reasoning","status":"in_progress","summary":[],"content":[1]},
        {"type":"reasoning","content":[1],"encrypted_content":"  "},
        {"type":"reasoning","content":[1],"encrypted_content":null},
        {"type":"reasoning","content":[1],"encrypted_content":42},
        {"type":"reasoning","content":{"status":"keep"},"encrypted_content":"test-cipher"},
        {"type":"reasoning","content":"keep","encrypted_content":"test-cipher"},
        {"type":"compaction","id":"cmp_replay","content":[1],"encrypted_content":"test-cipher"},
        {"type":"message","role":"user","status":"completed","content":[{"type":"input_text","text":"hello"}]},
        {"type":"function_call","call_id":"call_replay","name":"echo","arguments":"{}","status":"completed"},
        {"type":"function_call_output","call_id":"call_replay","output":{"status":"keep","content":[1]},"status":"completed"},
        {"type":"tool_search_output","call_id":"call_search","status":"completed","tools":[]},
        {"type":"future_item","status":"keep","content":[1]},
        {"status":"keep","content":[1]},
        null,
        "opaque-item",
        {"type":"reasoning","id":"rs_missing_ciphertext","summary":[{"type":"summary_text","text":"summary"}],"content":[],"encrypted_content":null},
        {"type":"reasoning","id":"rs_missing_content","summary":[]},
        {"type":"reasoning","id":"rs_plaintext","content":[{"type":"reasoning_text","text":"replay"}]}
    ]);
    let mut expected = input.clone();
    for index in [0, 1, 2] {
        expected[index]
            .as_object_mut()
            .unwrap()
            .shift_remove("status");
    }
    expected[0].as_object_mut().unwrap().shift_remove("content");
    expected[19] = json!({"type":"reasoning", "summary":[{"type":"summary_text","text":"replay"}]});
    expected
        .as_array_mut()
        .unwrap()
        .retain(|item| item["id"] != "rs_missing_ciphertext" && item["id"] != "rs_missing_content");
    for qx in [false, true] {
        for websocket in [false, true] {
            let actual = capture(input.clone(), websocket, qx).await;
            assert_eq!(actual["input"].to_string(), expected.to_string());
        }
    }
}

#[tokio::test]
async fn stored_reasoning_reference_remains_available() {
    let input = json!([{"type":"reasoning","id":"rs_stored","summary":[]}]);
    for websocket in [false, true] {
        assert_eq!(
            capture_with_store(input.clone(), websocket, false, true).await["input"],
            input
        );
    }
}

#[tokio::test]
async fn reasoning_replay_preserves_non_array_input_without_guessing() {
    for input in [
        Value::Null,
        json!({"type":"reasoning","status":"keep"}),
        json!(7),
    ] {
        assert_eq!(capture(input.clone(), false, false).await["input"], input);
    }
}

async fn capture(input: Value, websocket: bool, qx: bool) -> Value {
    capture_with_store(input, websocket, qx, false).await
}

async fn capture_with_store(input: Value, websocket: bool, qx: bool, stored: bool) -> Value {
    let http = MockServer::start().await;
    let (base_url, websocket_server) = if websocket {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_codex_test_websocket(stream).await;
            let frame = socket.next().await.unwrap().unwrap();
            let body: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
            super::generate_compat::send_completion(&mut socket, "resp_replay").await;
            body
        });
        (base_url, Some(task))
    } else {
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(
                    format!("event: response.created\ndata: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_scope_capture\",\"model\":\"gpt-5.4\"}}}}\n\n{CAPTURE_COMPLETED_SSE}"),
                    "text/event-stream",
                ),
            )
            .expect(1)
            .mount(&http)
            .await;
        (http.uri(), None)
    };
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let original = json!({
        "model":"gpt-5.4","store":stored,"stream":true,"input":input,
        "instructions":"Preserve the original instructions.",
        "tools":[{"type":"function","name":"echo","parameters":{"type":"object","properties":{"status":{"type":"string"},"content":{"type":"array"}}}}],
        "future_field":{"status":"keep","content":[1]},
        "client_metadata":{"extension":{"status":"keep","content":[1]}}
    });
    let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", original.as_object().unwrap().clone())
            .unwrap()
            .with_context(Map::from_iter([(
                "use_websocket".to_owned(),
                json!(websocket),
            )])),
    ));
    let provider = if qx {
        super::identity_isolation::qx_provider(&store, base_url)
    } else {
        provider_with_base_url(&store, base_url)
    };
    let mut stream = provider
        .execute(
            planned_request("openai", operation),
            context("req_replay_compatibility", CancellationToken::new()),
        )
        .await
        .unwrap();
    while let Some(event) = stream.next().await {
        event.expect("successful completion");
    }
    let actual = if let Some(server) = websocket_server {
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
    } else {
        let requests = http.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            1,
            "normalization must not introduce retries"
        );
        captured_request_body(&requests[0])
    };
    for field in ["model", "store", "instructions", "tools", "future_field"] {
        assert_eq!(
            actual[field].to_string(),
            original[field].to_string(),
            "{field}"
        );
    }
    assert_eq!(
        actual["client_metadata"]["extension"],
        original["client_metadata"]["extension"]
    );
    actual
}

#[tokio::test]
async fn message_ids_are_normalized_without_changing_references_tools_or_ciphertext() {
    let input = json!([
        {"type":"message","id":"item_history","role":"assistant","content":[{"type":"output_text","text":"hello"}],"extension":{"id":"item_nested"}},
        {"type":"message","id":"msg_valid","role":"user","content":"keep"},
        {"type":"reasoning","id":"item_cipher","encrypted_content":"opaque-cipher","summary":[]},
        {"type":"message","id":"item_referenced","role":"assistant","content":"referenced"},
        {"type":"item_reference","id":"item_referenced"},
        {"type":"message","id":"item_collision","role":"assistant","content":"collision"},
        {"type":"message","id":"msg_collision","role":"assistant","content":"existing"},
        {"type":"message","id":"item_lookup"},
        {"type":"function_call","id":"item_call","call_id":"call_original","name":"echo","arguments":"{}"},
        {"type":"function_call_output","call_id":"call_original","output":"ok"},
        {"type":"message","id":"item_duplicate","role":"user","content":"a"},
        {"type":"message","id":"item_duplicate","role":"user","content":"b"}
    ]);
    for websocket in [false, true] {
        for stored in [false, true] {
            let actual = capture_with_store(input.clone(), websocket, false, stored).await;
            let mut expected = input.clone();
            if !stored {
                expected[0]["id"] = json!("msg_history");
            }
            assert_eq!(actual["input"], expected);
        }
    }
}

#[tokio::test]
async fn plaintext_reasoning_only_moves_when_every_part_can_be_preserved() {
    let input = json!([
        {"type":"reasoning","id":"rs_plain","summary":[],"content":[{"type":"reasoning_text","text":"first"},{"type":"reasoning_text","text":"second"}]},
        {"type":"reasoning","summary":[{"type":"summary_text","text":"existing"}],"content":[{"type":"reasoning_text","text":"different"}]},
        {"type":"reasoning","content":[{"type":"reasoning_text","text":"keep","extension":true}]},
        {"type":"reasoning","content":[{"type":"reasoning_text","text":"keep"},{"type":"future_part","value":1}]},
        {"type":"reasoning","id":"rs_referenced","content":[{"type":"reasoning_text","text":"reference"}]},
        {"type":"item_reference","id":"rs_referenced"}
    ]);
    for websocket in [false, true] {
        for stored in [false, true] {
            let actual = capture_with_store(input.clone(), websocket, false, stored).await;
            let mut expected = input.clone();
            expected[0].as_object_mut().unwrap().remove("content");
            expected[0]["summary"] = json!([
                {"type":"summary_text","text":"first"},{"type":"summary_text","text":"second"}
            ]);
            if !stored {
                expected[0].as_object_mut().unwrap().remove("id");
            }
            assert_eq!(actual["input"], expected);
        }
    }
}
