//! Real OAuth selection, wire conversion and the Core response translator boundary.
use super::*;
use gateway_core::{event::ProtocolWireEvent, operation::TokenCountRequest};

fn body() -> Value {
    json!({"model":"public-model","max_tokens":128,"messages":[{"role":"user","content":"hello"}],"tools":[{"name":"lookup","description":"lookup","input_schema":{"type":"object","properties":{"id":{"type":"string"}}}}]})
}

#[tokio::test]
async fn messages_use_selected_oauth_account_and_translate_tools_and_usage() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let server = MockServer::start().await;
    let response = json!({"type":"response.completed","response":{"id":"resp_messages","model":"gpt-5.4","status":"completed","output":[{"type":"function_call","id":"fc_lookup","call_id":"call_lookup","name":"lookup","arguments":"{\"id\":\"one\"}"}],"usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13,"input_tokens_details":{"cached_tokens":4}}}});
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            format!("event: response.completed\ndata: {response}\n\n"),
            "text/event-stream",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider_with_base_url(&store, server.uri());
    let payload = ProtocolPayload::json_object("anthropic", body().as_object().unwrap().clone())
        .unwrap()
        .with_context(Map::from_iter([
            ("use_websocket".into(), json!(false)),
            (
                "opaque_request_headers".into(),
                json!([["anthropic-version", "2023-06-01"]]),
            ),
        ]));
    let original = payload.clone();
    let mut stream = provider
        .execute(
            planned_request(
                "openai",
                Operation::Generate(GenerateRequest::from_protocol_payload(payload)),
            ),
            context("req_messages_native", CancellationToken::new()),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_provider_contract"
    );
    let mut translated = Vec::new();
    while let Some(event) = stream.next().await {
        let event = event.unwrap();
        let terminal = event
            .wire_event()
            .is_some_and(|wire| wire.data()["type"] == "response.completed");
        for event in stream.translate_native_response(event, terminal).unwrap() {
            if let Some(wire) = event.wire_event() {
                assert_eq!(wire.protocol(), "anthropic");
                translated.push(wire.data().clone());
            }
        }
    }
    assert_eq!(
        translated
            .iter()
            .filter(|v| v["type"] == "message_stop")
            .count(),
        1
    );
    let tool = translated
        .iter()
        .find(|v| v["type"] == "content_block_start")
        .unwrap();
    assert_eq!(tool["content_block"]["type"], "tool_use");
    assert_eq!(tool["content_block"]["name"], "lookup");
    let finish = translated
        .iter()
        .find(|v| v["type"] == "message_delta")
        .unwrap();
    assert_eq!(finish["delta"]["stop_reason"], "tool_use");
    assert_eq!(finish["usage"]["output_tokens"], 3);
    let requests = server.received_requests().await.unwrap();
    let wire = &requests[0];
    let bytes = if wire
        .headers
        .get("content-encoding")
        .is_some_and(|v| v == "zstd")
    {
        zstd::stream::decode_all(wire.body.as_slice()).unwrap()
    } else {
        wire.body.clone()
    };
    let sent: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sent["model"], "gpt-5.4");
    assert_eq!(sent["max_output_tokens"], 128);
    assert_eq!(sent["tools"][0]["name"], "lookup");
    assert!(wire.headers.get("authorization").is_some());
    assert!(wire.headers.get("anthropic-version").is_none());
    assert_eq!(original.body()["model"], "public-model");
}

#[tokio::test]
async fn messages_count_tokens_is_local_and_cancellation_releases_selection() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let server = MockServer::start().await;
    let provider = provider_with_base_url(&store, server.uri());
    for cancel in [false, true] {
        let cancellation = CancellationToken::new();
        let payload = gateway_core::operation::RawJsonPayload::new(
            "anthropic",
            serde_json::to_vec(&body()).unwrap().into(),
        )
        .unwrap();
        let mut stream = provider
            .execute(
                planned_request(
                    "openai",
                    Operation::CountTokens(TokenCountRequest::from_raw_json(payload)),
                ),
                context("req_messages_count", cancellation.clone()),
            )
            .await
            .unwrap();
        assert_eq!(stream.metadata().transport().as_str(), "local_tokenizer");
        if cancel {
            cancellation.cancel();
        }
        let mut count = None;
        let mut cancelled = false;
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => {
                    if let Some(raw) = event
                        .wire_event()
                        .and_then(ProtocolWireEvent::raw_json_body)
                    {
                        count =
                            serde_json::from_slice::<Value>(raw).unwrap()["input_tokens"].as_u64();
                    }
                }
                Err(error) => {
                    assert_eq!(error.kind(), ProviderErrorKind::Cancelled);
                    cancelled = true;
                }
            }
        }
        assert_eq!(cancelled, cancel);
        assert_eq!(count.is_some_and(|n| n > 0), !cancel);
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn translated_messages_service_tiers_follow_the_selected_responses_upstream() {
    for (options, expected) in [
        (json!({"service_tier":"auto"}), None),
        (json!({"service_tier":"standard_only"}), None),
        (json!({"speed":"fast"}), Some("priority")),
    ] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut body = json!({"model":"gpt-5.4","max_tokens":128,"stream":true,"messages":[{"role":"user","content":"hello"}]});
        body.as_object_mut()
            .unwrap()
            .extend(options.as_object().unwrap().clone());
        let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object("anthropic", body.as_object().unwrap().clone())
                .unwrap()
                .with_context(Map::from_iter([("use_websocket".to_owned(), json!(false))])),
        ));
        let mut stream = provider_with_base_url(&store, server.uri())
            .execute(
                planned_request("openai", operation),
                context("req_translated_tier", CancellationToken::new()),
            )
            .await
            .unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        let requests = server.received_requests().await.unwrap();
        let sent = captured_request_body(&requests[0]);
        assert_eq!(sent.get("service_tier").and_then(Value::as_str), expected);
        let hint = expected.map_or_else(
            || "model=gpt-5.4".to_owned(),
            |tier| format!("model=gpt-5.4;tier={tier}"),
        );
        assert_eq!(requests[0].headers["x-codex-routing-hint"], hint);
        server.verify().await;
    }
}
