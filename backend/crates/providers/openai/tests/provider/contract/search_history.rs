use super::*;

#[tokio::test]
async fn search_history_repair_uses_the_effective_http_and_websocket_lite_flag() {
    for websocket in [false, true] {
        for lite in [false, true] {
            for has_tools in [false, true] {
                let store = Arc::new(MemoryAccountStore::default());
                create_account(&store, "acct_provider_contract").await;
                let http = MockServer::start().await;
                Mock::given(method("POST"))
                    .and(path("/codex/responses"))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .insert_header("content-type", "text/event-stream")
                            .set_body_string(CAPTURE_COMPLETED_SSE),
                    )
                    .mount(&http)
                    .await;
                let (url, ws_server) = if websocket {
                    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let url = format!("http://{}", listener.local_addr().unwrap());
                    let server = tokio::spawn(async move {
                        let (socket, _) = listener.accept().await.unwrap();
                        let mut ws = accept_codex_test_websocket(socket).await;
                        let request: Value = serde_json::from_str(
                            ws.next().await.unwrap().unwrap().to_text().unwrap(),
                        )
                        .unwrap();
                        ws.send(Message::Text(
                            json!({"type":"response.completed","response":{
                                "id":"resp_search","status":"completed","output":[],
                                "usage":{"input_tokens":1,"output_tokens":0,"total_tokens":1}
                            }})
                            .to_string()
                            .into(),
                        ))
                        .await
                        .unwrap();
                        request
                    });
                    (url, Some(server))
                } else {
                    (http.uri(), None)
                };
                let mut body = json!({"model":"gpt-5.4","store":false,"stream":true,
                "input":[{"type":"web_search_call","id":"ws_original","status":"completed"},
                    {"type":"compaction_trigger"}], "tools":[], "tool_choice":"auto"});
                if has_tools {
                    body["tools"] =
                        json!([{"type":"function","name":"echo","parameters":{"type":"object"}}]);
                    body["tool_choice"] = json!("required");
                }
                let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
                    ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
                        .unwrap()
                        .with_context(Map::from_iter([
                            ("use_websocket".into(), json!(websocket)),
                            (
                                "responses_lite".into(),
                                json!(if lite { "true" } else { "false" }),
                            ),
                        ])),
                ));
                let mut stream = provider_with_base_url(&store, url)
                    .execute(
                        planned_request("openai", operation),
                        context("req_search_history", CancellationToken::new()),
                    )
                    .await
                    .unwrap();
                while let Some(event) = stream.next().await {
                    event.unwrap();
                }
                let actual = if let Some(server) = ws_server {
                    server.await.unwrap()
                } else {
                    captured_request_body(&http.received_requests().await.unwrap()[0])
                };
                assert_eq!(actual["input"][0], body["input"][0]);
                assert_eq!(
                    actual["tool_choice"],
                    if has_tools { "required" } else { "none" }
                );
                let declaration = if lite {
                    assert_eq!(actual["input"][2]["type"], "compaction_trigger");
                    &actual["input"][1]["tools"][0]
                } else {
                    assert_eq!(actual["input"], body["input"]);
                    if has_tools {
                        assert_eq!(actual["tools"][0], body["tools"][0]);
                    }
                    &actual["tools"][usize::from(has_tools)]
                };
                assert_eq!(
                    declaration,
                    &json!({"type":"web_search","external_web_access":false})
                );
            }
        }
    }
}
