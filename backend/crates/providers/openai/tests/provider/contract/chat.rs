//! OAuth Chat 实际选号、HTTP 出站、目标协议转换和计量合同。
use super::*;
use gateway_protocol::chat_completions::{CHAT_PROTOCOL, ChatAccumulator};

fn chat_operation(stream: bool, fields: Value) -> Operation {
    let mut body = json!({"model":"public-model","messages":[{"role":"user","content":"hello"}],
        "stream":stream});
    if stream {
        body["stream_options"] = json!({"include_usage":true});
    }
    body.as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object(CHAT_PROTOCOL, body.as_object().unwrap().clone())
            .unwrap()
            .with_context(Map::from_iter([("use_websocket".into(), json!(false))])),
    ))
}

fn sse(response: Value) -> String {
    format!(
        "event: response.created\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
        json!({"type":"response.created","response":{"id":response["id"],"model":response["model"],"status":"in_progress","output":[]}}),
        json!({"type":"response.completed","response":response})
    )
}

#[tokio::test]
async fn chat_selected_oauth_converts_tools_and_keeps_actual_usage_and_public_model() {
    for streaming in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/codex/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse(json!({
                "id":"resp_chat_actual","model":"gpt-5.4","status":"completed",
                "output":[{"type":"function_call","id":"fc_lookup","call_id":"call_lookup","name":"lookup","arguments":"{\"id\":\"one\"}"}],
                "usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13,"input_tokens_details":{"cached_tokens":4}}
            })), "text/event-stream")).expect(1).mount(&server).await;
        let provider = provider_with_base_url(&store, server.uri());
        let mut stream = provider.execute(planned_request("openai", chat_operation(streaming, json!({
            "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}]
        }))), context_with_request_settings("req_chat_actual",
            gateway_core::settings::OpenAiRequestSettings { residency: (!streaming).then(|| "fixture-region".to_owned()), account_routing_override: (!streaming).then(|| "us_cr".to_owned()), ..Default::default() }
        )).await.unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_provider_contract"
        );
        let mut accumulator = ChatAccumulator::default();
        let mut usage = Usage::new();
        let mut terminal_count = 0;
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            let mut terminal = false;
            for fact in event.canonical_facts() {
                match fact {
                    GatewayEvent::Completed(_) => {
                        terminal = true;
                        terminal_count += 1;
                    }
                    GatewayEvent::Usage(value) => usage.merge(value),
                    _ => {}
                }
            }
            for translated in stream.translate_native_response(event, terminal).unwrap() {
                if let Some(wire) = translated.wire_event() {
                    assert_eq!(wire.protocol(), CHAT_PROTOCOL);
                    accumulator.push(wire.data()).unwrap();
                }
            }
        }
        let result = accumulator.finish().unwrap();
        assert_eq!(terminal_count, 1);
        assert_eq!(result["model"], "public-model");
        assert_eq!(result["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            result["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "lookup"
        );
        assert_eq!(result["usage"]["prompt_tokens"], 10);
        assert_eq!(result["usage"]["completion_tokens"], 3);
        assert_eq!(result["usage"]["prompt_tokens_details"]["cached_tokens"], 4);
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(3));
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
        assert_eq!(sent["tools"][0]["name"], "lookup");
        assert_eq!(sent["input"][0]["role"], "user");
        assert!(sent.get("messages").is_none());
        assert!(wire.headers.contains_key("authorization"));
        assert_eq!(
            wire.headers
                .get("x-openai-internal-codex-residency")
                .map(|v| v.to_str().unwrap()),
            (!streaming).then_some("fixture-region")
        );
        assert_eq!(
            wire.headers
                .get("x-openai-account-routing-override")
                .map(|v| v.to_str().unwrap()),
            (!streaming).then_some("us_cr")
        );
    }
}

#[tokio::test]
async fn chat_malformed_target_outputs_fail_before_success_translation() {
    for output in [
        Value::Null,
        json!({}),
        json!([{"type":"function_call","call_id":"","name":"lookup","arguments":"{}"}]),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/codex/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse(json!({"id":"resp_bad_chat","model":"gpt-5.4","status":"completed","output":output})), "text/event-stream"))
            .mount(&server).await;
        let provider = provider_with_base_url(&store, server.uri());
        let mut stream = provider
            .execute(
                planned_request("openai", chat_operation(true, json!({}))),
                context("req_chat_bad", CancellationToken::new()),
            )
            .await
            .unwrap();
        let mut rejected = false;
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => {
                    let terminal = event
                        .canonical_facts()
                        .iter()
                        .any(|f| matches!(f, GatewayEvent::Completed(_)));
                    if stream.translate_native_response(event, terminal).is_err() {
                        rejected = true;
                        break;
                    }
                }
                Err(_) => {
                    rejected = true;
                    break;
                }
            }
        }
        assert!(
            rejected,
            "malformed output must not become a successful Chat result"
        );
    }
}

#[tokio::test]
async fn chat_cancelled_before_poll_never_sends_and_does_not_invent_native_continuation() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let server = MockServer::start().await;
    let provider = provider_with_base_url(&store, server.uri());
    let cancellation = CancellationToken::new();
    let mut stream = provider
        .execute(
            planned_request("openai", chat_operation(true, json!({}))),
            context("req_chat_cancel", cancellation.clone()),
        )
        .await
        .unwrap();
    cancellation.cancel();
    let error = stream.next().await.unwrap().unwrap_err();
    assert_eq!(error.kind(), ProviderErrorKind::Cancelled);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn chat_target_validation_keeps_field_errors_without_sending() {
    for (fields, param) in [
        (
            json!({"web_search_options":{"search_context_size":7}}),
            "web_search_options.search_context_size",
        ),
        (
            json!({"web_search_options":{"user_location":{"type":"approximate"}}}),
            "web_search_options.user_location.approximate",
        ),
        (
            json!({"web_search_options":{}, "tool_choice":"none"}),
            "tool_choice",
        ),
        (
            json!({"web_search_options":{},"tools":[{"type":"function","function":{"name":"lookup"}}]}),
            "tools",
        ),
        (
            json!({"messages":[{"role":"assistant","content":[{"type":"text","text":"history","prompt_cache_breakpoint":{"type":"default"}}]}]}),
            "messages[0].content[0].prompt_cache_breakpoint",
        ),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let server = MockServer::start().await;
        let provider = provider_with_base_url(&store, server.uri());
        let error = provider
            .execute(
                planned_request("openai", chat_operation(false, fields)),
                context("req_chat_invalid", CancellationToken::new()),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
        assert_eq!(error.send_state(), UpstreamSendState::NotSent);
        let public = gateway_core::error::GatewayError::from_provider(&error);
        assert_eq!(public.client_error_param(), Some(param));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
