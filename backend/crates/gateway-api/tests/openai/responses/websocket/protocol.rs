use axum::http::{HeaderMap, HeaderValue};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gateway_api::openai::responses::{OpenAiRequestHeaders, decode_response_create_with_context};
use gateway_core::operation::{Operation, ProviderSessionState};
use serde_json::json;

use super::super::http::{FakeSession, NextStep, Trace};
use super::decode_response_create;
use super::*;

struct WindowAuditExecution {
    client: AuthenticatedClient,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl ExecutionService for WindowAuditExecution {
    fn authenticate(
        &self,
        plaintext: &str,
    ) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        (plaintext == "sk_window_fixture")
            .then(|| self.client.clone())
            .ok_or(ClientAuthenticationError::InvalidKey)
    }

    fn public_models(&self, _: &AuthenticatedClient) -> Vec<PublicModelId> {
        vec![PublicModelId::new("model-a").unwrap()]
    }

    fn contains_public_model(&self, _: &AuthenticatedClient, model: &PublicModelId) -> bool {
        model.as_str() == "model-a"
    }

    fn start(
        &self,
        request: StartExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move {
            let Operation::Generate(generate) = request.operation else {
                panic!("fixture expects Generate");
            };
            let mut requests = self.requests.lock().unwrap();
            requests.push(json!({
                "body":generate.protocol_payload().body(),
                "previous":request.metadata.previous_response_id.as_ref().map(|id| id.as_str()),
                "user_agent":request.metadata.user_agent,
                "replay":generate.provider_session_state("openai").is_some(),
            }));
            if generate.protocol_payload().body().get("fixture_fail") == Some(&json!(true)) {
                return Err(GatewayError::new(
                    gateway_core::error::GatewayErrorKind::Internal,
                    "synthetic fixture failure",
                ));
            }
            let response_id = format!("resp_window_{}", requests.len());
            let mut events = ["response.created", "response.completed"].map(|kind| {
                ProviderEvent::wire(ProtocolWireEvent::json("openai", Some(kind.into()), json!({
                    "type":kind,"response":{"id":response_id,"model":"model-a",
                        "status":if kind == "response.completed" {"completed"} else {"in_progress"},
                        "output":[]}
                })).unwrap())
            });
            events[1].attach_session_update(
                ProviderSessionState::new("openai", serde_json::Map::new()).unwrap(),
            );
            Ok(StartedExecution {
                request_id: ModelRequestId::new(format!("req_window_{}", requests.len())).unwrap(),
                created_at: SystemTime::now(),
                stream: true,
                session: Box::new(FakeSession::streaming(
                    Arc::new(Trace::default()),
                    vec![
                        NextStep::Event(
                            CoordinatedEvent::try_batch(
                                events.into(),
                                CommitRequirement::CommitBeforeDelivery,
                            )
                            .unwrap(),
                        ),
                        NextStep::FinalizeSuccess,
                    ],
                )),
            })
        })
    }

    fn start_provider_endpoint(
        &self,
        _: StartProviderExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async { unreachable!("no provider endpoint in fixture") })
    }
}

#[tokio::test]
async fn websocket_window_rollover_clears_only_the_changed_window_anchor() {
    for (name, opening, first, second, third, direct) in [
        (
            "embedded_with_handshake",
            Some("window-a"),
            Some("window-a"),
            Some("window-b"),
            Some("window-b"),
            false,
        ),
        (
            "embedded_without_handshake",
            None,
            Some("window-a"),
            Some("window-b"),
            Some("window-b"),
            false,
        ),
        (
            "handshake_initial_window",
            Some("window-a"),
            None,
            Some("window-b"),
            Some("window-b"),
            false,
        ),
        (
            "same_frame_window",
            Some("window-a"),
            Some("window-b"),
            Some("window-b"),
            Some("window-b"),
            false,
        ),
        (
            "omitted_after_rollover",
            Some("window-a"),
            Some("window-a"),
            Some("window-b"),
            None,
            false,
        ),
        (
            "direct_frame_window",
            Some("window-a"),
            Some("window-a"),
            Some("window-b"),
            Some("window-b"),
            true,
        ),
        ("missing_window", Some("window-a"), None, None, None, false),
        (
            "unknown_initial_window",
            None,
            None,
            Some("window-b"),
            Some("window-b"),
            false,
        ),
    ] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let router = api_router(Arc::new(WindowAuditExecution {
            client: authenticated_client("sk_window_fixture"),
            requests: requests.clone(),
        }))
        .await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut upgrade = format!("ws://{address}/v1/responses")
            .into_client_request()
            .unwrap();
        upgrade.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer sk_window_fixture"),
        );
        upgrade.headers_mut().insert(
            "user-agent",
            HeaderValue::from_static("window-audit-fixture/1.0"),
        );
        if let Some(window) = opening {
            upgrade.headers_mut().insert(
                "x-codex-turn-metadata",
                HeaderValue::from_str(&json!({"window_id":window}).to_string()).unwrap(),
            );
        }
        let (mut socket, _) = connect_async(upgrade).await.unwrap();
        for (turn, window) in [first, second, third].into_iter().enumerate() {
            let mut frame = json!({"type":"response.create","model":"model-a","input":"synthetic context","prompt_cache_key":"cache-window-fixture"});
            if turn > 0 {
                frame["previous_response_id"] = json!(format!("resp_window_{turn}"));
            }
            if let Some(window) = window {
                frame["client_metadata"] = if direct {
                    json!({"x-codex-window-id":window})
                } else {
                    json!({"x-codex-turn-metadata":json!({"window_id":window}).to_string()})
                };
            }
            socket
                .send(ClientMessage::Text(frame.to_string().into()))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let frame = socket.next().await.unwrap().unwrap();
                    if let ClientMessage::Text(text) = frame {
                        let event: Value = serde_json::from_str(&text).unwrap();
                        assert_ne!(event["type"], "error", "{name}: {event}");
                        if event["type"] == "response.completed" {
                            break;
                        }
                    }
                }
            })
            .await
            .unwrap();
        }
        socket.close(None).await.unwrap();
        server.abort();
        let _ = server.await;
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3, "{name}");
        for (turn, request) in requests.iter().enumerate() {
            let rollover = turn == 1
                && first.or(opening).is_some()
                && second.is_some()
                && first.or(opening) != second;
            let previous = if turn == 0 || rollover {
                Value::Null
            } else {
                json!(format!("resp_window_{turn}"))
            };
            assert_eq!(
                request["previous"], previous,
                "{name}: only the window boundary clears the explicit anchor"
            );
            assert_eq!(request["body"]["previous_response_id"], previous, "{name}");
            assert_eq!(request["replay"], !previous.is_null(), "{name}");
            assert_eq!(
                request["body"]["prompt_cache_key"], "cache-window-fixture",
                "{name}"
            );
            assert_eq!(request["user_agent"], "window-audit-fixture/1.0", "{name}");
        }
        println!(
            "WINDOW_AUDIT {name}: 3 turns; rollover isolated; same-window continuation preserved"
        );
    }
}

fn window_frame(metadata: Value, previous: Option<&str>) -> Value {
    let mut frame = json!({
        "type":"response.create", "model":"model-a",
        "input":[{"type":"function_call_output","call_id":"call_fixture","output":"opaque"}],
        "client_metadata":metadata, "prompt_cache_key":"cache-fixture",
        "future_extension":{"keep":true}
    });
    if let Some(previous) = previous {
        frame["previous_response_id"] = json!(previous);
    }
    frame
}

async fn exchange_window_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    frame: &Value,
) -> Value {
    socket
        .send(ClientMessage::Text(frame.to_string().into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let ClientMessage::Text(text) = socket.next().await.unwrap().unwrap() {
                let event: Value = serde_json::from_str(&text).unwrap();
                if matches!(
                    event["type"].as_str(),
                    Some("error" | "response.failed" | "response.completed")
                ) {
                    return event;
                }
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn websocket_window_replay_handles_unknown_hints_failure_and_independent_connections() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let router = api_router(Arc::new(WindowAuditExecution {
        client: authenticated_client("sk_window_fixture"),
        requests: requests.clone(),
    }))
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut upgrade = format!("ws://{address}/v1/responses")
        .into_client_request()
        .unwrap();
    upgrade.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer sk_window_fixture"),
    );
    let (mut socket, _) = connect_async(upgrade.clone()).await.unwrap();
    let mut failed = window_frame(
        json!({"x-codex-window-id":"window-b"}),
        Some("resp_window_5"),
    );
    failed["fixture_fail"] = json!(true);
    let cases = [
        (
            window_frame(json!({"x-codex-window-id":"window-a"}), None),
            None,
            false,
        ),
        (
            window_frame(
                json!({"x-codex-window-id":" window-a "}),
                Some("resp_window_1"),
            ),
            Some("resp_window_1"),
            true,
        ),
        (
            window_frame(
                json!({"x-codex-window-id":"window-a","x-codex-turn-metadata":"{\"window_id\":\"window-b\"}"}),
                Some("resp_window_2"),
            ),
            Some("resp_window_2"),
            true,
        ),
        (
            window_frame(
                json!({"x-codex-window-id":" ","x-codex-turn-metadata":"not-json"}),
                Some("resp_window_3"),
            ),
            Some("resp_window_3"),
            true,
        ),
        (
            window_frame(
                json!({"x-codex-turn-metadata":"{\"window_id\":42}"}),
                Some("resp_window_4"),
            ),
            Some("resp_window_4"),
            true,
        ),
        (failed, None, false),
        (
            window_frame(
                json!({"x-codex-turn-metadata":"{\"window_id\":\"window-b\"}"}),
                Some("resp_window_5"),
            ),
            None,
            false,
        ),
        (
            window_frame(
                json!({"x-codex-window-id":"window-b"}),
                Some("resp_window_7"),
            ),
            Some("resp_window_7"),
            true,
        ),
        (
            window_frame(json!({"x-codex-window-id":"window-c"}), None),
            None,
            false,
        ),
        (
            window_frame(json!({}), Some("resp_window_9")),
            Some("resp_window_9"),
            true,
        ),
    ];
    for (frame, previous, replay) in cases {
        let event = exchange_window_frame(&mut socket, &frame).await;
        let expected_terminal = if frame["fixture_fail"] == true {
            "error"
        } else {
            "response.completed"
        };
        assert_eq!(event["type"], expected_terminal);
        let mut expected = frame.as_object().unwrap().clone();
        expected.remove("type");
        if previous.is_none() {
            expected.remove("previous_response_id");
        }
        let captured = requests.lock().unwrap().last().unwrap().clone();
        assert_eq!(captured["body"], json!(expected));
        assert_eq!(captured["previous"], json!(previous));
        assert_eq!(captured["replay"], replay);
    }
    let mut invalid = window_frame(
        json!({"x-codex-window-id":"window-d"}),
        Some("resp_window_10"),
    );
    invalid["model"] = Value::Null;
    assert_eq!(
        exchange_window_frame(&mut socket, &invalid).await["type"],
        "error"
    );
    assert_eq!(requests.lock().unwrap().len(), 10);
    let same = window_frame(
        json!({"x-codex-window-id":"window-c"}),
        Some("resp_window_10"),
    );
    assert_eq!(
        exchange_window_frame(&mut socket, &same).await["type"],
        "response.completed"
    );
    let captured = requests.lock().unwrap().last().unwrap().clone();
    assert_eq!(captured["previous"], "resp_window_10");
    assert_eq!(captured["replay"], true);
    socket.close(None).await.unwrap();
    let (mut independent, _) = connect_async(upgrade).await.unwrap();
    let first = window_frame(
        json!({"x-codex-window-id":"window-d"}),
        Some("resp_window_11"),
    );
    assert_eq!(
        exchange_window_frame(&mut independent, &first).await["type"],
        "response.completed"
    );
    let captured = requests.lock().unwrap().last().unwrap().clone();
    assert_eq!(captured["previous"], "resp_window_11");
    assert_eq!(captured["replay"], false);
    independent.close(None).await.unwrap();
    server.abort();
    let _ = server.await;
}

#[test]
fn response_create_should_exclude_cloudflare_headers_on_every_frame() {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("CF-Visitor", r#"{"scheme":"https"}"#),
        ("cdn-loop", "cloudflare; loops=1"),
        ("cf-warp-tag-id", "downstream-warp"),
        ("cf-ipcountry", "US"),
        ("cf-worker", "worker.example"),
        ("cf-ew-via", "15"),
        ("cf-future-proxy-field", "downstream-only"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers.append("x-openai-future-mode", HeaderValue::from_static("first"));
    headers.append("x-openai-future-mode", HeaderValue::from_static("second"));
    let request_headers = OpenAiRequestHeaders::from_headers(&headers);

    // 连接级头会用于同一 WebSocket 的每一帧，后续帧也不能恢复下游链路元数据。
    for input in ["hello", "continue"] {
        let decoded = decode_response_create_with_context(
            &json!({"type": "response.create", "model": "smart-code", "input": input}).to_string(),
            &request_headers,
        )
        .expect("decode response.create behind Cloudflare");
        let Operation::Generate(request) = decoded.operation() else {
            panic!("Responses must map to Generate");
        };
        assert_eq!(
            request
                .protocol_payload()
                .context()
                .get("opaque_request_headers"),
            Some(&json!([
                ["x-openai-future-mode", STANDARD.encode(b"first")],
                ["x-openai-future-mode", STANDARD.encode(b"second")],
            ])),
        );
        assert_eq!(request.protocol_payload().body()["input"], input);
    }
}

#[test]
fn latest_official_codex_response_create_fixture_decodes_unchanged() {
    // Audited against openai/codex main 94cbbddafc1776d5e377bca1b05932c697e82238.
    let decoded = decode_response_create(
        &json!({
            "type": "response.create",
            "model": "smart-code",
            "input": "hello",
            "store": false
        })
        .to_string(),
    )
    .expect("latest official Codex response.create fixture must decode");
    let Operation::Generate(request) = decoded.operation() else {
        panic!("Responses must map to Generate");
    };

    assert_eq!(
        request.protocol_payload().body(),
        json!({
            "model": "smart-code",
            "input": "hello",
            "store": false
        })
        .as_object()
        .expect("fixture body")
    );
}
