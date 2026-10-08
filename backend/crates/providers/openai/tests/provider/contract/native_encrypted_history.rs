use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn rejection() -> Value {
    json!({"type":"response.failed","response":{"id":"resp_rejected","status":"failed",
        "error":{"code":"invalid_encrypted_content","type":"invalid_request_error","message":"Invalid encrypted history"}}})
}

fn completed() -> Value {
    json!({"type":"response.completed","response":{"id":"resp_recovered","model":"gpt-5.4",
        "status":"completed","output":[],"usage":{"input_tokens":3,"output_tokens":1,"total_tokens":4}}})
}

fn as_sse(value: &Value) -> String {
    format!(
        "event: {}\ndata: {value}\n\n",
        value["type"].as_str().unwrap()
    )
}

fn created() -> Value {
    json!({"type":"response.created","response":{"id":"resp_recovered","model":"gpt-5.4","status":"in_progress"}})
}

fn success_sse() -> String {
    as_sse(&created()) + &as_sse(&completed())
}

fn body() -> Value {
    json!({"model":"gpt-5.4","store":false,"input":[
        {"type":"reasoning","id":"rs_invalid","encrypted_content":"cipher-fixture","summary":[]},
        {"role":"user","content":"hello"},
        {"type":"function_call","call_id":"call_fixture","name":"echo","arguments":"{}"},
        {"type":"function_call_output","call_id":"call_fixture","output":"ok"}
    ],"session_id":"recovery-session","thread_id":"recovery-thread",
    "tools":[{"type":"function","name":"echo","parameters":{"type":"object"}}],
    "future_field":{"opaque":"keep"}})
}

fn operation(body: &Value, websocket: bool) -> Operation {
    Operation::Generate(
        GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
                .unwrap()
                .with_context(Map::from_iter([("use_websocket".into(), json!(websocket))])),
        )
        .with_provider_session_state(
            ProviderSessionState::new(
                "openai",
                Map::from_iter([
                    ("account_id".into(), json!("acct_provider_contract")),
                    ("conversation_id".into(), json!("recovery-conversation")),
                    ("continuation_scope".into(), json!("persisted")),
                ]),
            )
            .unwrap(),
        ),
    )
}

async fn run(
    provider: &CodexProvider,
    body: &Value,
    websocket: bool,
) -> Result<(), gateway_core::error::ProviderError> {
    let mut stream = provider
        .execute(
            planned_request("openai", operation(body, websocket)),
            context("req_encrypted_recovery", CancellationToken::new()),
        )
        .await?;
    while let Some(event) = timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
    {
        event?;
    }
    Ok(())
}

#[tokio::test]
async fn native_encrypted_http_recovery_is_same_account_once_and_remembers_only_success() {
    for http_status in [200, 400] {
        for fail_again in [false, true] {
            let server = MockServer::start().await;
            let count = Arc::new(AtomicUsize::new(0));
            let calls = count.clone();
            Mock::given(method("POST"))
                .and(path("/codex/responses"))
                .respond_with(move |_: &wiremock::Request| {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 || fail_again {
                        if http_status == 400 {
                            ResponseTemplate::new(400)
                                .set_body_json(json!({"error":rejection()["response"]["error"]}))
                        } else {
                            ResponseTemplate::new(200)
                                .set_body_raw(as_sse(&rejection()), "text/event-stream")
                        }
                    } else {
                        ResponseTemplate::new(200).set_body_raw(success_sse(), "text/event-stream")
                    }
                })
                .mount(&server)
                .await;
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            // Protocol repair does not consume or enable ordinary transport retries.
            let provider = provider_with_base_url_and_retry_budget(&store, server.uri(), 0);
            let original = body();
            let result = run(&provider, &original, false).await;
            assert_eq!(result.is_err(), fail_again);
            assert_eq!(count.load(Ordering::SeqCst), 2);
            let requests = server.received_requests().await.unwrap();
            let first = captured_request_body(&requests[0]);
            let second = captured_request_body(&requests[1]);
            assert_eq!(first["input"][0]["encrypted_content"], "cipher-fixture");
            assert_eq!(
                second["input"],
                Value::Array(first["input"].as_array().unwrap()[1..].to_vec())
            );
            for field in [
                "model",
                "tools",
                "future_field",
                "session_id",
                "thread_id",
                "client_metadata",
            ] {
                assert_eq!(first[field], second[field], "{field}");
            }
            for header in [
                "authorization",
                "chatgpt-account-id",
                "user-agent",
                "session_id",
                "x-client-installation-id",
                "x-codex-turn-state",
            ] {
                assert_eq!(
                    captured_header_values(&requests[0], header),
                    captured_header_values(&requests[1], header),
                    "{header}"
                );
            }
            let _ = run(&provider, &original, false).await;
            let requests = server.received_requests().await.unwrap();
            let next = captured_request_body(&requests[2]);
            assert_eq!(
                next["input"][0].get("encrypted_content").is_some(),
                fail_again
            );
            assert_eq!(count.load(Ordering::SeqCst), if fail_again { 4 } else { 3 });
            assert_eq!(original, body());
            assert_eq!(
                store
                    .account("acct_provider_contract")
                    .unwrap()
                    .credential_state(),
                CredentialState::Ready
            );
        }
    }
}

#[tokio::test]
async fn native_encrypted_websocket_recovery_preserves_identity_and_never_delivers_rejected_frames()
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut bodies = Vec::new();
        for attempt in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_codex_test_websocket(stream).await;
            let frame = socket.next().await.unwrap().unwrap().into_text().unwrap();
            bodies.push(serde_json::from_str::<Value>(&frame).unwrap());
            if attempt == 1 {
                socket
                    .send(Message::Text(created().to_string().into()))
                    .await
                    .unwrap();
            }
            socket
                .send(Message::Text(
                    if attempt == 0 {
                        rejection()
                    } else {
                        completed()
                    }
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let _ = socket.close(None).await;
        }
        bodies
    });
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let provider = provider_with_base_url_and_retry_budget(&store, url, 0);
    let mut stream = provider
        .execute(
            planned_request("openai", operation(&body(), true)),
            context("req_encrypted_ws", CancellationToken::new()),
        )
        .await
        .unwrap();
    let mut completed_count = 0;
    while let Some(event) = timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
    {
        let event = event.unwrap();
        for fact in event.canonical_facts() {
            if let GatewayEvent::Completed(meta) = fact {
                completed_count += 1;
                assert_eq!(meta.response_id(), "resp_recovered");
            }
        }
    }
    assert_eq!(completed_count, 1);
    let mut bodies = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bodies[0]["input"].as_array().unwrap().len(), 4);
    assert_eq!(bodies[1]["input"].as_array().unwrap().len(), 3);
    // Attempt timestamps advance; device and session identity must not.
    for body in &mut bodies {
        body["client_metadata"]
            .as_object_mut()
            .unwrap()
            .remove("x-codex-ws-stream-request-start-ms");
    }
    for field in [
        "session_id",
        "thread_id",
        "client_metadata",
        "tools",
        "future_field",
    ] {
        assert_eq!(bodies[0][field], bodies[1][field], "{field}");
    }
}

#[tokio::test]
async fn native_encrypted_recovery_never_replays_delivered_output_or_other_errors() {
    for semantic_output in [false, true] {
        let server = MockServer::start().await;
        let mut error = rejection();
        if !semantic_output {
            error["response"]["error"]["code"] = json!("server_error");
        }
        let mut frames = String::new();
        if semantic_output {
            frames.push_str(&as_sse(&created()));
            frames.push_str(&as_sse(
                &json!({"type":"response.output_text.delta","delta":"already delivered"}),
            ));
        }
        frames.push_str(&as_sse(&error));
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(frames, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let provider = provider_with_base_url_and_retry_budget(&store, server.uri(), 0);
        assert!(run(&provider, &body(), false).await.is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn minimal_alias_is_native_http_and_ws_and_observes_effective_effort() {
    for websocket in [false, true] {
        let http = MockServer::start().await;
        let (url, server) = if websocket {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let handle = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = accept_codex_test_websocket(stream).await;
                let request: Value = serde_json::from_str(
                    &socket.next().await.unwrap().unwrap().into_text().unwrap(),
                )
                .unwrap();
                socket
                    .send(Message::Text(created().to_string().into()))
                    .await
                    .unwrap();
                socket
                    .send(Message::Text(completed().to_string().into()))
                    .await
                    .unwrap();
                request
            });
            (url, Some(handle))
        } else {
            Mock::given(method("POST"))
                .and(path("/codex/responses"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_raw(success_sse(), "text/event-stream"),
                )
                .expect(1)
                .mount(&http)
                .await;
            (http.uri(), None)
        };
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let provider = provider_with_base_url(&store, url);
        let source = json!({"model":"gpt-5.4","input":"hello","reasoning":{"effort":"minimal","summary":"auto"}});
        let mut stream = provider
            .execute(
                planned_request("openai", operation(&source, websocket)),
                context("req_minimal_alias", CancellationToken::new()),
            )
            .await
            .unwrap();
        let mut observed = false;
        while let Some(event) = timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
        {
            let event = event.unwrap();
            if let Some(metadata) = event
                .response_observation()
                .and_then(|observation| observation.provider_metadata())
            {
                let value: Value = serde_json::from_str(metadata.as_json()).unwrap();
                if value["requestSummary"]["effectiveReasoningEffort"] == "none" {
                    assert_eq!(
                        value["requestSummary"]["requestedReasoningEffort"],
                        "minimal"
                    );
                    observed = true;
                }
            }
        }
        let actual = if let Some(server) = server {
            server.await.unwrap()
        } else {
            captured_request_body(&http.received_requests().await.unwrap()[0])
        };
        assert_eq!(
            actual["reasoning"],
            json!({"effort":"none","summary":"auto"})
        );
        assert_eq!(source["reasoning"]["effort"], "minimal");
        assert!(observed);
    }
}

#[tokio::test]
async fn native_encrypted_zero_buffer_does_not_replay_even_structural_delivery() {
    let (url, release, _, server) =
        paused_chunked_sse_server(as_sse(&created()), as_sse(&rejection())).await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let provider = provider_with_base_url(&store, url);
    let attempt = context("req_zero_buffer", CancellationToken::new()).with_request_tuning(
        gateway_core::routing::RequestTuning {
            stream_prefetch_bytes: 0,
            ..Default::default()
        },
    );
    let mut stream = provider
        .execute(
            planned_request("openai", operation(&body(), false)),
            attempt,
        )
        .await
        .unwrap();
    loop {
        let event = timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if event.has_client_event() {
            break;
        }
    }
    release.send(()).unwrap();
    let mut failed = false;
    while let Some(event) = stream.next().await {
        if let Err(error) = event {
            assert_eq!(
                error.upstream_code().unwrap().as_str(),
                "invalid_encrypted_content"
            );
            failed = true;
            break;
        }
    }
    assert!(failed);
    server.await.unwrap();
}

#[tokio::test]
async fn native_encrypted_cancelled_recovery_does_not_send_again() {
    let server = MockServer::start().await;
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(move |_: &wiremock::Request| {
            cancel.cancel();
            ResponseTemplate::new(400)
                .set_body_json(json!({"error":rejection()["response"]["error"]}))
        })
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let provider = provider_with_base_url(&store, server.uri());
    let mut stream = provider
        .execute(
            planned_request("openai", operation(&body(), false)),
            context("req_cancel_recovery", cancellation),
        )
        .await
        .unwrap();
    let mut failed = false;
    while let Some(event) = stream.next().await {
        if event.is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
