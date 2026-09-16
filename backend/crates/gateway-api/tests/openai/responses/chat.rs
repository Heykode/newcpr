use super::chat_cpa::{ChatSession, Script};
use super::http::{FakeSession, NextStep, Trace, delivery_provider};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use axum::{
    body::{Body, Bytes, to_bytes},
    http::{
        Request, StatusCode,
        header::{AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE},
    },
};
use futures::future::BoxFuture;
use gateway_core::engine::ModelRequestId;
use gateway_core::engine::execution::{
    AuthenticatedClient, ClientAuthenticationError, ExecutionService, StartExecution,
    StartProviderExecution, StartedExecution,
};
use gateway_core::engine::{CommitRequirement, EngineError};
use gateway_core::error::{
    ClientVisibleUpstreamResponse, GatewayError, GatewayErrorKind, ProviderError, ProviderErrorKind,
};
use gateway_core::event::{ProtocolWireEvent, ProviderEvent, ProviderResponseHeader};
use gateway_core::operation::Operation;
use gateway_core::routing::PublicModelId;
use gateway_core::upstream::{OpaqueUpstreamValue, UpstreamSendState};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::openai::{api_router, authenticated_client_for_provider};

const PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAAEElEQVR4nGIyaDgACAAA//8CXAFz1fWjqwAAAABJRU5ErkJggg==";

#[derive(Clone, Copy)]
pub(super) enum Scenario {
    Text,
    CommentPreamble,
    MetadataPreamble,
    MetadataAfterStarted,
    OnlyComments,
    OnlyUnknown,
    UnknownPreamble,
    Function,
    Custom,
    Image,
    ImagePreviewInterrupted,
    JsonWithSearchAndImage,
    OpaqueImage,
    Interrupted,
    MissingTerminal,
    UpstreamError,
    RateLimited,
    BodyReadFailure,
    CommitError,
    ConversionErrorFirst,
    ConversionErrorAfterText,
}

pub(super) struct ChatExecution {
    client: AuthenticatedClient,
    captured: Mutex<Vec<Value>>,
    pub(super) trace: Arc<Trace>,
    scenario: Scenario,
    pub(super) script: Mutex<Option<Script>>,
}

impl ChatExecution {
    pub(super) fn new(scenario: Scenario) -> Arc<Self> {
        Arc::new(Self {
            client: authenticated_client_for_provider("sk_chat_test", "openai"),
            captured: Mutex::new(Vec::new()),
            trace: Arc::new(Trace::default()),
            scenario,
            script: Mutex::new(None),
        })
    }
}

impl ExecutionService for ChatExecution {
    fn authenticate(
        &self,
        plaintext: &str,
    ) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        (plaintext == "sk_chat_test")
            .then(|| self.client.clone())
            .ok_or(ClientAuthenticationError::InvalidKey)
    }

    fn public_models(&self, _: &AuthenticatedClient) -> Vec<PublicModelId> {
        vec![PublicModelId::new("model-a").expect("model")]
    }

    fn contains_public_model(&self, _: &AuthenticatedClient, model: &PublicModelId) -> bool {
        matches!(model.as_str(), "model-a" | "gpt-5.4")
    }

    fn start(
        &self,
        request: StartExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move {
            let Operation::Generate(generation) = &request.operation else {
                panic!("Chat must use the existing Generate operation");
            };
            self.captured.lock().expect("capture").push(json!({
                "endpoint": request.metadata.endpoint,
                "model": request.public_model.as_str(),
                "stream": request.metadata.stream,
                "transport": format!("{:?}", request.metadata.transport),
                "body": generation.protocol_payload().body(),
                "context": generation.protocol_payload().context(),
            }));
            let events = response_events(self.scenario);
            let mut session = if request.metadata.stream {
                let mut steps: Vec<_> = events
                    .into_iter()
                    .map(|event| {
                        NextStep::Event(delivery_provider(
                            event,
                            CommitRequirement::CommitBeforeDelivery,
                        ))
                    })
                    .collect();
                steps.push(
                    if matches!(
                        self.scenario,
                        Scenario::Interrupted | Scenario::ImagePreviewInterrupted
                    ) {
                        NextStep::Error(EngineError::Deadline)
                    } else {
                        NextStep::FinalizeSuccess
                    },
                );
                FakeSession::streaming(Arc::clone(&self.trace), steps)
            } else {
                FakeSession::buffered_provider(Arc::clone(&self.trace), events)
            };
            if matches!(self.scenario, Scenario::CommitError) {
                session = session.with_commit_failure();
            }
            if matches!(self.scenario, Scenario::BodyReadFailure) {
                let error = EngineError::Provider(
                    ProviderError::new(ProviderErrorKind::Transport, UpstreamSendState::Ambiguous)
                        .with_status(503)
                        .with_upstream_request_id(OpaqueUpstreamValue::new("req-chat-body-read")),
                );
                if request.metadata.stream {
                    session.next = VecDeque::from([NextStep::Error(error)]);
                } else {
                    session = session.with_collect_error(error);
                }
                session = session.with_response_headers(vec![
                    ProviderResponseHeader::new("x-request-id", Bytes::from_static(b"old-opening")),
                    ProviderResponseHeader::new(
                        "x-oai-request-id",
                        Bytes::from_static(b"old-alias"),
                    ),
                    ProviderResponseHeader::new(
                        "x-codex-turn-state",
                        Bytes::from_static(b"retained-turn"),
                    ),
                ]);
            }
            if matches!(
                self.scenario,
                Scenario::UpstreamError | Scenario::RateLimited
            ) {
                let rate_limited = matches!(self.scenario, Scenario::RateLimited);
                let error = EngineError::Provider(
                    ProviderError::new(
                        if rate_limited {
                            ProviderErrorKind::RateLimited
                        } else {
                            ProviderErrorKind::Unavailable
                        },
                        UpstreamSendState::Sent,
                    )
                    .with_client_visible_upstream_response(
                        ClientVisibleUpstreamResponse::new(
                            if rate_limited { 429 } else { 502 },
                            Some(b"text/plain".to_vec()),
                            Bytes::from_static(b"event: error\nupstream unavailable"),
                        )
                        .with_headers(vec![
                            ProviderResponseHeader::new(
                                "x-oai-request-id",
                                Bytes::from_static(b"req-chat-failure"),
                            ),
                            ProviderResponseHeader::new("retry-after", Bytes::from_static(b"17")),
                            ProviderResponseHeader::new(
                                "content-length",
                                Bytes::from_static(b"9999"),
                            ),
                        ]),
                    ),
                );
                if request.metadata.stream {
                    session.next = VecDeque::from([NextStep::Error(error)]);
                } else {
                    session = session.with_collect_error(error);
                }
                session = session.with_response_headers(vec![ProviderResponseHeader::new(
                    "x-request-id",
                    Bytes::from_static(b"old-opening"),
                )]);
            }
            let session = match self.script.lock().expect("script").take() {
                Some(script) => script.session(Arc::clone(&self.trace), request.metadata.stream),
                None => ChatSession::new(session),
            };
            Ok(StartedExecution {
                request_id: ModelRequestId::new("req_chat_test").expect("request"),
                created_at: SystemTime::now(),
                stream: request.metadata.stream,
                session: Box::new(session),
            })
        })
    }

    fn start_provider_endpoint(
        &self,
        _: StartProviderExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        panic!("Chat must not bypass model routing via a provider endpoint")
    }
}

pub(super) fn wire(kind: &str, data: Value) -> ProviderEvent {
    ProviderEvent::wire(
        ProtocolWireEvent::json("openai", Some(kind.to_owned()), data).expect("wire"),
    )
}

pub(super) fn response_events(scenario: Scenario) -> Vec<ProviderEvent> {
    let conversion_error = || {
        wire(
            "response.failed",
            json!({"type":"response.failed","response":{"status":"failed"}}),
        )
    };
    if matches!(scenario, Scenario::ConversionErrorFirst) {
        return vec![conversion_error()];
    }
    let comment = || {
        ProviderEvent::wire(
            ProtocolWireEvent::raw_sse("openai", Bytes::from_static(b": keep-alive\n\n"))
                .expect("SSE comment"),
        )
    };
    if matches!(scenario, Scenario::OnlyComments) {
        return vec![comment()];
    }
    if matches!(scenario, Scenario::OnlyUnknown) {
        return vec![wire(
            "response.unknown_semantic_event",
            json!({"type":"response.unknown_semantic_event"}),
        )];
    }
    let mut events = vec![wire(
        "response.created",
        json!({"type":"response.created","response":{
            "id":"resp_chat","model":"model-a","created_at":1700000000,
            "status":"in_progress","output":[]
        }}),
    )];
    if matches!(scenario, Scenario::CommentPreamble) {
        events.insert(0, comment());
    }
    if matches!(
        scenario,
        Scenario::MetadataPreamble | Scenario::MetadataAfterStarted
    ) {
        events.insert(
            usize::from(matches!(scenario, Scenario::MetadataAfterStarted)),
            wire(
                "codex.response.metadata",
                json!({"type":"codex.response.metadata","headers":{"x-request-id":"upstream-1"}}),
            ),
        );
    }
    if matches!(scenario, Scenario::UnknownPreamble) {
        events.insert(
            0,
            wire(
                "response.unknown_semantic_event",
                json!({"type":"response.unknown_semantic_event"}),
            ),
        );
    }
    let output = if matches!(scenario, Scenario::Function) {
        let item = json!({
            "type":"function_call","id":"fc_1","call_id":"call_1",
            "name":"lookup","arguments":""
        });
        events.push(wire(
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":0,"item":item}),
        ));
        events.push(wire(
            "response.function_call_arguments.delta",
            json!({"type":"response.function_call_arguments.delta","output_index":0,
                "item_id":"fc_1","delta":"{\"city\":\"Paris\"}"}),
        ));
        json!([{
            "type":"function_call","id":"fc_1","call_id":"call_1",
            "name":"lookup","arguments":"{\"city\":\"Paris\"}","status":"completed"
        }])
    } else if matches!(scenario, Scenario::Custom) {
        events.push(wire(
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":0,
                "item":{"type":"custom_tool_call","id":"ct_1","call_id":"call_1","name":"code","input":""}}),
        ));
        for delta in ["print(", "1)"] {
            events.push(wire(
                "response.custom_tool_call_input.delta",
                json!({"type":"response.custom_tool_call_input.delta","output_index":0,
                    "item_id":"ct_1","delta":delta}),
            ));
        }
        json!([{"type":"custom_tool_call","id":"ct_1","call_id":"call_1","name":"code","input":"print(1)"}])
    } else if matches!(
        scenario,
        Scenario::Image | Scenario::ImagePreviewInterrupted
    ) {
        events.push(wire(
            "response.image_generation_call.partial_image",
            json!({"type":"response.image_generation_call.partial_image","output_index":0,
                "item_id":"img_1","partial_image_index":0,"partial_image_b64":PIXEL_PNG,
                "output_format":"png"}),
        ));
        json!([{"type":"image_generation_call","id":"img_1","result":PIXEL_PNG,
            "output_format":"png","status":"completed"}])
    } else if matches!(scenario, Scenario::JsonWithSearchAndImage) {
        json!([
            {"type":"web_search_call","id":"ws_1","status":"completed","action":{
                "type":"search","sources":[{"type":"url","url":"https://example.test/"}]}},
            {"type":"message","id":"msg_json","role":"assistant","status":"completed",
                "content":[{"type":"output_text","text":"{\"answer\":42}","annotations":[]}]},
            {"type":"image_generation_call","id":"img_1","result":PIXEL_PNG,
                "output_format":"png","status":"completed"}
        ])
    } else if matches!(scenario, Scenario::OpaqueImage) {
        json!([{"type":"image_generation_call","id":"img_1","result":"opaque-not-base64!",
            "output_format":"image/future","status":"completed"}])
    } else {
        events.push(wire(
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":0,
                "item":{"type":"message","id":"msg_1","role":"assistant","content":[]}}),
        ));
        events.push(wire(
            "response.output_text.delta",
            json!({"type":"response.output_text.delta","item_id":"msg_1",
                "output_index":0,"content_index":0,"delta":"hello"}),
        ));
        json!([{"type":"message","id":"msg_1","role":"assistant","status":"completed",
            "content":[{"type":"output_text","text":"hello","annotations":[]}]}])
    };
    if matches!(scenario, Scenario::ConversionErrorAfterText) {
        events.push(conversion_error());
        return events;
    }
    if !matches!(
        scenario,
        Scenario::Interrupted | Scenario::MissingTerminal | Scenario::ImagePreviewInterrupted
    ) {
        if matches!(scenario, Scenario::JsonWithSearchAndImage) {
            events.push(wire(
                "response.output_text.delta",
                json!({"type":"response.output_text.delta","item_id":"msg_json",
                    "output_index":1,"content_index":0,"delta":"{\"answer\":42}"}),
            ));
        }
        for (index, item) in output.as_array().expect("output items").iter().enumerate() {
            events.push(wire(
                "response.output_item.done",
                json!({"type":"response.output_item.done","output_index":index,"item":item}),
            ));
        }
        events.push(wire(
            "response.completed",
            json!({"type":"response.completed","response":{
                "id":"resp_chat","model":"model-a","created_at":1700000000,
                "status":"completed","output":output,
                "usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13,
                    "input_tokens_details":{"cached_tokens":4},
                    "output_tokens_details":{"reasoning_tokens":1}}
            }}),
        ));
    }
    events
}

pub(super) async fn request(
    execution: Arc<ChatExecution>,
    body: Value,
) -> axum::response::Response {
    api_router(execution)
        .await
        .oneshot(
            Request::post("/v1/chat/completions")
                .header(AUTHORIZATION, "Bearer sk_chat_test")
                .header(CONTENT_TYPE, "application/json")
                .header("session-id", "test-session")
                .header("cookie", "downstream-only")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("handler")
}

pub(super) fn text_request(streaming: bool) -> Value {
    json!({"model":"model-a","messages":[{"role":"user","content":"hi"}],
        "stream":streaming,"stream_options":{"include_usage":true}})
}

pub(super) async fn read_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body")
            .to_vec(),
    )
    .expect("utf8")
}

pub(super) fn chunks(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|line| *line != "[DONE]")
        .map(|line| serde_json::from_str(line).expect("Chat SSE JSON"))
        .collect()
}

#[tokio::test]
async fn chat_preamble_is_not_a_false_error_or_an_early_commit() {
    for scenario in [Scenario::CommentPreamble, Scenario::MetadataPreamble] {
        let execution = ChatExecution::new(scenario);
        let response = request(Arc::clone(&execution), text_request(true)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            execution.trace.snapshot(),
            vec![
                "next_event",
                "defer_commit",
                "next_event",
                "defer_commit",
                "next_event",
                "defer_commit",
                "next_event",
                "commit"
            ]
        );
        let body = read_text(response).await;
        assert!(!body.contains("\"error\""), "{body}");
        assert!(!body.contains("codex.response.metadata"));
        assert!(body.contains("hello"));
        assert_eq!(body.matches("\"finish_reason\":\"stop\"").count(), 1);
        assert_eq!(
            chunks(&body)
                .iter()
                .filter(|chunk| chunk["usage"].is_object())
                .count(),
            1
        );
        assert!(body.ends_with("data: [DONE]\n\n"));
        assert!(!execution.trace.is_cancelled());
    }
}

#[tokio::test]
async fn chat_metadata_after_started_does_not_interrupt_text() {
    let execution = ChatExecution::new(Scenario::MetadataAfterStarted);
    let response = request(execution, text_request(true)).await;
    let body = read_text(response).await;
    assert!(!body.contains("\"error\""), "{body}");
    assert!(body.contains("hello"));
    assert_eq!(body.matches("\"finish_reason\":\"stop\"").count(), 1);
}

#[tokio::test]
async fn chat_only_comments_or_unknown_events_do_not_become_success() {
    for scenario in [Scenario::OnlyComments, Scenario::OnlyUnknown] {
        let execution = ChatExecution::new(scenario);
        let response = request(Arc::clone(&execution), text_request(true)).await;
        assert!(response.status().is_server_error());
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        let body: Value = serde_json::from_str(&read_text(response).await).expect("JSON error");
        assert!(body["error"].is_object());
        assert_eq!(
            execution.trace.snapshot(),
            ["next_event", "defer_commit", "next_end"]
        );
    }
}

#[tokio::test]
async fn chat_normalizes_rate_limit_body_without_losing_status_or_retry_delay() {
    for streaming in [true, false] {
        let execution = ChatExecution::new(Scenario::RateLimited);
        let response = request(
            execution,
            json!({
                "model":"model-a","messages":[{"role":"user","content":"hi"}],"stream":streaming
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "17");
        assert_eq!(response.headers()["x-request-id"], "req-chat-failure");
        assert_eq!(response.headers()["x-oai-request-id"], "req-chat-failure");
        assert_eq!(response.headers()["x-gateway-request-id"], "req_chat_test");
        assert_ne!(
            response
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok()),
            Some("9999")
        );
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        let body: Value = serde_json::from_str(&read_text(response).await).expect("JSON error");
        assert_eq!(body["error"]["code"], "rate_limit_exceeded");
        assert!(body.get("choices").is_none());
    }
}

#[tokio::test]
async fn chat_incomplete_error_body_keeps_current_failure_id_and_turn_state() {
    for streaming in [false, true] {
        let execution = ChatExecution::new(Scenario::BodyReadFailure);
        let mut body = text_request(streaming);
        if !streaming {
            body.as_object_mut()
                .expect("object")
                .remove("stream_options");
        }
        let response = request(Arc::clone(&execution), body).await;
        assert_eq!(
            execution.captured.lock().expect("capture").len(),
            1,
            "valid request must reach the injected execution failure"
        );
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(response.headers()["x-request-id"], "req-chat-body-read");
        assert_eq!(response.headers()["x-gateway-request-id"], "req_chat_test");
        assert!(response.headers().get("x-oai-request-id").is_none());
        assert_eq!(response.headers()["x-codex-turn-state"], "retained-turn");
        assert_eq!(response.headers().get_all("x-request-id").iter().count(), 1);
        let body: Value = serde_json::from_str(&read_text(response).await).expect("JSON error");
        assert!(body.get("error").is_some());
        assert!(body.get("choices").is_none());
    }
}

#[tokio::test]
async fn actual_newapi_converter_fixtures_use_the_same_execution_and_keep_tool_results() {
    let fixtures = [
        include_str!("../fixtures/newapi-chat-text.json"),
        include_str!("../fixtures/newapi-chat-text-stream.json"),
        include_str!("../fixtures/newapi-chat-image.json"),
        include_str!("../fixtures/newapi-chat-function.json"),
        include_str!("../fixtures/newapi-chat-structured.json"),
        include_str!("../fixtures/newapi-messages-tool-multiturn.json"),
        include_str!("../fixtures/newapi-messages-image.json"),
    ];
    for fixture in fixtures {
        let source: Value = serde_json::from_str(fixture).expect("actual converter fixture");
        let execution = ChatExecution::new(Scenario::Text);
        let response = request(Arc::clone(&execution), source.clone()).await;
        assert_eq!(response.status(), StatusCode::OK, "{fixture}");
        let response_body = read_text(response).await;
        if source["stream"] == true {
            assert!(response_body.ends_with("data: [DONE]\n\n"));
            assert!(
                chunks(&response_body)
                    .iter()
                    .all(|chunk| chunk.get("error").is_none())
            );
        } else {
            let response: Value = serde_json::from_str(&response_body).expect("Chat JSON");
            assert_eq!(response["choices"][0]["message"]["content"], "hello");
        }
        let captured = execution.captured.lock().expect("capture");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0]["endpoint"], "/v1/chat/completions");
        assert_eq!(captured[0]["model"], source["model"]);
        let input = captured[0]["body"]["input"]
            .as_array()
            .expect("Responses input");
        for message in source["messages"].as_array().expect("messages") {
            if message["role"] == "tool" {
                let result = input
                    .iter()
                    .find(|item| {
                        item["type"] == "function_call_output"
                            && item["call_id"] == message["tool_call_id"]
                    })
                    .expect("tool result kept");
                assert_eq!(result["output"], message["content"]);
            }
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    let mapped = input
                        .iter()
                        .find(|item| {
                            item["type"] == "function_call" && item["call_id"] == call["id"]
                        })
                        .expect("tool call kept");
                    assert_eq!(mapped["name"], call["function"]["name"]);
                    assert_eq!(mapped["arguments"], call["function"]["arguments"]);
                }
            }
            if let Some(parts) = message["content"].as_array() {
                for image in parts.iter().filter(|part| part["type"] == "image_url") {
                    assert!(
                        input
                            .iter()
                            .filter_map(|item| item["content"].as_array())
                            .flatten()
                            .any(|part| part["type"] == "input_image"
                                && part["image_url"] == image["image_url"]["url"])
                    );
                }
            }
        }
        if source["response_format"]["type"] == "json_schema" {
            assert_eq!(
                captured[0]["body"]["text"]["format"]["schema"],
                source["response_format"]["json_schema"]["schema"]
            );
            assert_eq!(captured[0]["body"]["text"]["format"]["strict"], true);
        }
        assert_eq!(
            execution
                .trace
                .snapshot()
                .iter()
                .filter(|event| **event == "commit")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn chat_accepts_a_large_prompt_without_the_default_two_mib_body_limit() {
    let execution = ChatExecution::new(Scenario::Text);
    let prompt = "long synthetic prompt ".repeat(150_000);
    assert!(prompt.len() > 2 * 1024 * 1024);
    let response = request(
        Arc::clone(&execution),
        json!({"model":"model-a","messages":[{"role":"user","content":prompt}]}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let captured = execution.captured.lock().expect("capture");
    assert_eq!(
        captured[0]["body"]["input"][0]["content"][0]["text"],
        prompt
    );
}

#[tokio::test]
async fn chat_decodes_compressed_requests_and_preserves_downstream_stream_choice() {
    use std::io::Write;
    let execution = ChatExecution::new(Scenario::Text);
    let body = json!({"model":"model-a","messages":[{"role":"user","content":"compressed"}],
        "stream":false,"use_websocket":true});
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(body.to_string().as_bytes())
        .expect("gzip");
    let response = api_router(execution.clone())
        .await
        .oneshot(
            Request::post("/v1/chat/completions")
                .header(AUTHORIZATION, "Bearer sk_chat_test")
                .header(CONTENT_TYPE, "application/json")
                .header(CONTENT_ENCODING, "gzip")
                .body(Body::from(encoder.finish().expect("gzip")))
                .expect("request"),
        )
        .await
        .expect("handler");
    assert_eq!(response.status(), StatusCode::OK);
    let captured = execution.captured.lock().expect("capture");
    assert_eq!(captured[0]["stream"], false);
    assert_eq!(captured[0]["context"]["use_websocket"], true);
    assert_eq!(
        captured[0]["body"]["input"][0]["content"][0]["text"],
        "compressed"
    );
}

#[tokio::test]
async fn native_responses_keeps_hosted_image_output_and_opaque_compaction_input() {
    let execution = ChatExecution::new(Scenario::Image);
    let input = json!([
        {"type":"compaction","encrypted_content":"synthetic-opaque","future_field":9007199254740993_u64},
        {"role":"user","content":"Continue"}
    ]);
    let tools = json!([{"type":"image_generation"},{"type":"web_search"}]);
    let response = api_router(execution.clone())
        .await
        .oneshot(
            Request::post("/v1/responses")
                .header(AUTHORIZATION, "Bearer sk_chat_test")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"model":"model-a","stream":false,
                "input":input,"tools":tools})
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("handler");
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = serde_json::from_str(&read_text(response).await).expect("JSON");
    assert_eq!(result["output"][0]["type"], "image_generation_call");
    assert_eq!(result["output"][0]["result"], PIXEL_PNG);
    let captured = execution.captured.lock().expect("capture");
    assert_eq!(captured[0]["endpoint"], "/v1/responses");
    assert_eq!(captured[0]["body"]["input"], input);
    assert_eq!(captured[0]["body"]["tools"], tools);
}

#[tokio::test]
async fn chat_buffered_preserves_usage_and_uses_existing_execution() {
    let execution = ChatExecution::new(Scenario::Text);
    let mut body = text_request(false);
    body.as_object_mut().expect("object").remove("stream");
    body.as_object_mut()
        .expect("object")
        .remove("stream_options");
    body["use_websocket"] = json!(true);
    let response = request(Arc::clone(&execution), body).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-gateway-request-id"], "req_chat_test");
    assert_eq!(response.headers()["x-request-id"], "req_chat_test");
    let value: Value = serde_json::from_str(&read_text(response).await).expect("json");
    assert_eq!(value["object"], "chat.completion");
    assert_eq!(value["choices"][0]["message"]["content"], "hello");
    assert_eq!(value["usage"]["prompt_tokens"], 10);
    assert_eq!(value["usage"]["completion_tokens"], 3);
    assert_eq!(value["usage"]["prompt_tokens_details"]["cached_tokens"], 4);
    let captured = execution.captured.lock().expect("capture");
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0]["endpoint"], "/v1/chat/completions");
    assert_eq!(captured[0]["stream"], false);
    assert_eq!(captured[0]["transport"], "HttpJson");
    assert_eq!(captured[0]["context"]["use_websocket"], true);
    assert_eq!(captured[0]["context"]["session_id"], "test-session");
    assert!(captured[0]["body"].get("messages").is_none());
    assert!(captured[0]["body"].get("use_websocket").is_none());
    assert!(
        !captured[0]["context"]
            .to_string()
            .contains("downstream-only")
    );
    assert_eq!(execution.trace.snapshot(), ["collect", "commit"]);
    assert!(!execution.trace.is_cancelled());
}

#[tokio::test]
async fn chat_stream_returns_chat_chunks_and_usage_without_repeating_text() {
    let execution = ChatExecution::new(Scenario::Text);
    let response = request(Arc::clone(&execution), text_request(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "text/event-stream");
    let body = read_text(response).await;
    let values = chunks(&body);
    let text: String = values
        .iter()
        .filter_map(|value| value["choices"][0]["delta"]["content"].as_str())
        .collect();
    assert_eq!(text, "hello");
    assert!(
        values
            .iter()
            .any(|value| value["usage"]["total_tokens"] == 13)
    );
    assert!(
        values
            .iter()
            .any(|value| value["choices"][0]["finish_reason"] == "stop")
    );
    assert!(!body.contains("event: response."));
    assert!(body.ends_with("data: [DONE]\n\n"));
    assert_eq!(execution.trace.client_statuses(), [200]);
    assert!(!execution.trace.is_cancelled());
}

#[tokio::test]
async fn chat_tool_calls_and_tool_results_keep_matching_ids() {
    for streaming in [false, true] {
        let execution = ChatExecution::new(Scenario::Function);
        let mut body = text_request(streaming);
        if !streaming {
            body.as_object_mut()
                .expect("object")
                .remove("stream_options");
        }
        body["messages"] = json!([
            {"role":"user","content":"look up"},
            {"role":"assistant","content":null,"tool_calls":[{
                "id":"call_old","type":"function",
                "function":{"name":"lookup","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"call_old","content":"{\"ok\":true}"}
        ]);
        body["tools"] = json!([{"type":"function","function":{
            "name":"lookup","parameters":{"type":"object","properties":{}}
        }}]);
        let response = request(Arc::clone(&execution), body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = read_text(response).await;
        let captured = execution.captured.lock().expect("capture");
        let input = captured[0]["body"]["input"].as_array().expect("input");
        assert!(
            input
                .iter()
                .any(|item| item["type"] == "function_call" && item["call_id"] == "call_old")
        );
        assert!(input.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_old"));
        if streaming {
            let values = chunks(&body);
            assert!(
                values
                    .iter()
                    .any(|value| value["choices"][0]["finish_reason"] == "tool_calls")
            );
            let arguments: String = values
                .iter()
                .filter_map(|value| {
                    value["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"].as_str()
                })
                .collect();
            assert_eq!(arguments, "{\"city\":\"Paris\"}");
        } else {
            let value: Value = serde_json::from_str(&body).expect("JSON");
            assert_eq!(
                value["choices"][0]["message"]["tool_calls"][0]["id"],
                "call_1"
            );
            assert_eq!(value["choices"][0]["finish_reason"], "tool_calls");
        }
    }
}

#[tokio::test]
async fn chat_image_output_does_not_validate_base64_or_restrict_mime() {
    for streaming in [false, true] {
        let execution = ChatExecution::new(Scenario::OpaqueImage);
        let mut body = text_request(streaming);
        body.as_object_mut()
            .expect("object")
            .remove("stream_options");
        let response = request(Arc::clone(&execution), body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = read_text(response).await;
        assert!(!body.contains("\"error\""), "{body}");
        let images = if streaming {
            chunks(&body)
                .into_iter()
                .filter_map(|value| value["choices"][0]["delta"]["images"].as_array().cloned())
                .flatten()
                .collect::<Vec<_>>()
        } else {
            let value: Value = serde_json::from_str(&body).expect("JSON");
            value["choices"][0]["message"]["images"]
                .as_array()
                .expect("images")
                .clone()
        };
        assert_eq!(images.len(), 1);
        assert_eq!(
            images[0]["image_url"]["url"],
            "data:image/future;base64,opaque-not-base64!"
        );
        assert!(body.contains("\"finish_reason\":\"stop\""));
        assert!(!execution.trace.is_cancelled());
    }
}

#[tokio::test]
async fn chat_image_output_is_separate_from_content_with_original_usage() {
    for streaming in [false, true] {
        let execution = ChatExecution::new(Scenario::Image);
        let mut body = text_request(streaming);
        if !streaming {
            body.as_object_mut().unwrap().remove("stream_options");
        }
        body["tools"] = json!([{"type":"image_generation","output_format":"png"}]);
        let response = request(Arc::clone(&execution), body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = read_text(response).await;
        assert!(!body.contains("\"error\""));
        let (content, images) = if streaming {
            let values = chunks(&body);
            assert_eq!(
                values
                    .iter()
                    .filter(|value| value["usage"]["total_tokens"] == 13)
                    .count(),
                1
            );
            assert_eq!(
                values
                    .iter()
                    .filter(|value| value["choices"][0]["finish_reason"] == "stop")
                    .count(),
                1
            );
            assert!(body.ends_with("data: [DONE]\n\n"));
            let content = values
                .iter()
                .filter_map(|value| value["choices"][0]["delta"]["content"].as_str())
                .collect::<String>();
            let images = values
                .iter()
                .filter_map(|value| value["choices"][0]["delta"]["images"].as_array())
                .flatten()
                .cloned()
                .collect::<Vec<_>>();
            (content, images)
        } else {
            let value: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(value["usage"]["total_tokens"], 13);
            assert_eq!(value["choices"][0]["finish_reason"], "stop");
            assert!(value["choices"][0]["message"]["content"].is_null());
            (
                String::new(),
                value["choices"][0]["message"]["images"]
                    .as_array()
                    .unwrap()
                    .clone(),
            )
        };
        assert!(content.is_empty());
        assert_eq!(images.len(), 1);
        assert_eq!(
            images[0]["image_url"]["url"],
            format!("data:image/png;base64,{PIXEL_PNG}")
        );
        assert!(!execution.trace.is_cancelled());
        assert_eq!(execution.trace.client_statuses(), [200]);
        let captured = execution.captured.lock().unwrap();
        assert_eq!(captured[0]["body"]["tools"][0]["type"], "image_generation");
        assert_eq!(captured[0]["body"]["tools"][0]["output_format"], "png");
    }
}

#[tokio::test]
async fn chat_json_output_is_unchanged_by_search_metadata_and_images() {
    for streaming in [false, true] {
        let execution = ChatExecution::new(Scenario::JsonWithSearchAndImage);
        let mut body = text_request(streaming);
        if !streaming {
            body.as_object_mut().unwrap().remove("stream_options");
        }
        body["response_format"] = json!({"type":"json_object"});
        body["tools"] = json!([{"type":"web_search"},{"type":"image_generation"}]);
        let response = request(Arc::clone(&execution), body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = read_text(response).await;
        assert!(!body.contains("\"error\""));
        assert!(!body.contains("Sources:"));
        let content = if streaming {
            let values = chunks(&body);
            assert!(
                values
                    .iter()
                    .any(|v| v["choices"][0]["delta"]["images"].is_array())
            );
            values
                .iter()
                .filter_map(|v| v["choices"][0]["delta"]["content"].as_str())
                .collect::<String>()
        } else {
            let value: Value = serde_json::from_str(&body).unwrap();
            assert!(value["choices"][0]["message"]["images"].is_array());
            value["choices"][0]["message"]["content"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(content, "{\"answer\":42}");
        assert_eq!(
            serde_json::from_str::<Value>(&content).unwrap(),
            json!({"answer":42})
        );
        assert_eq!(
            execution.captured.lock().unwrap()[0]["body"]["text"]["format"]["type"],
            "json_object"
        );
        assert!(!execution.trace.is_cancelled());
    }
}

#[tokio::test]
async fn chat_custom_function_envelope_restores_upstream_kind_on_next_turn() {
    for streaming in [false, true] {
        let execution = ChatExecution::new(Scenario::Custom);
        let mut body = text_request(streaming);
        if !streaming {
            body.as_object_mut().unwrap().remove("stream_options");
        }
        let tools = json!([{"type":"custom","custom":{"name":"code","format":{"type":"text"}}}]);
        body["tools"] = tools.clone();
        let response = request(execution, body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = read_text(response).await;
        assert!(!body.contains("\"error\""));
        let call = if streaming {
            let values = chunks(&body);
            let calls: Vec<_> = values
                .iter()
                .filter_map(|value| value["choices"][0]["delta"]["tool_calls"].as_array())
                .flatten()
                .collect();
            assert_eq!(calls[0]["type"], "function");
            let arguments: String = calls
                .iter()
                .filter_map(|call| call["function"]["arguments"].as_str())
                .collect();
            json!({"id":calls[0]["id"],"type":"function","function":{"name":calls[0]["function"]["name"],"arguments":arguments}})
        } else {
            let value: Value = serde_json::from_str(&body).unwrap();
            value["choices"][0]["message"]["tool_calls"][0].clone()
        };
        assert_eq!(call["type"], "function");
        assert_eq!(call["function"]["arguments"], "print(1)");
        assert!(call.get("custom").is_none());
        let next = ChatExecution::new(Scenario::Text);
        let next_response = request(
            Arc::clone(&next),
            json!({
                "model":"model-a","tools":tools,
                "messages":[{"role":"assistant","content":null,"tool_calls":[call]},
                    {"role":"tool","tool_call_id":"call_1","content":"1"}]
            }),
        )
        .await;
        assert_eq!(next_response.status(), StatusCode::OK);
        let _ = read_text(next_response).await;
        let captured = next.captured.lock().unwrap();
        assert_eq!(captured[0]["body"]["input"][0]["type"], "custom_tool_call");
        assert_eq!(captured[0]["body"]["input"][0]["input"], "print(1)");
        assert_eq!(
            captured[0]["body"]["input"][1]["type"],
            "custom_tool_call_output"
        );
        assert_eq!(captured[0]["body"]["input"][1]["call_id"], "call_1");
    }
}

#[tokio::test]
async fn chat_image_preview_does_not_hide_an_interrupted_response() {
    let execution = ChatExecution::new(Scenario::ImagePreviewInterrupted);
    let response = request(Arc::clone(&execution), text_request(true)).await;
    let body = read_text(response).await;
    assert!(body.contains("\"images\""));
    assert!(body.contains("\"error\""));
    assert!(!body.contains("\"finish_reason\":\"stop\""));
    assert!(body.ends_with("data: [DONE]\n\n"));
    let trace = execution.trace.snapshot();
    assert_eq!(trace.iter().filter(|event| **event == "commit").count(), 1);
    assert_eq!(trace.last(), Some(&"next_error"));
    // The simulated deadline has already finalized execution.
    assert!(!execution.trace.is_cancelled());
}

#[tokio::test]
async fn chat_interrupted_or_missing_terminal_cannot_end_as_success() {
    for scenario in [Scenario::Interrupted, Scenario::MissingTerminal] {
        let execution = ChatExecution::new(scenario);
        let response = request(execution, text_request(true)).await;
        let body = read_text(response).await;
        assert!(body.contains("\"error\""));
        assert!(!body.contains("\"finish_reason\":\"stop\""));
        assert!(body.ends_with("data: [DONE]\n\n"));
    }
}

#[tokio::test]
async fn chat_normalizes_non_json_upstream_errors_for_newapi() {
    for streaming in [false, true] {
        let execution = ChatExecution::new(Scenario::UpstreamError);
        let mut body = text_request(streaming);
        body.as_object_mut()
            .expect("object")
            .remove("stream_options");
        let response = request(execution, body).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let value: Value = serde_json::from_str(&read_text(response).await).expect("JSON error");
        assert!(value["error"].is_object());
    }
}

#[tokio::test]
async fn chat_commit_failure_never_returns_a_successful_completion() {
    for streaming in [false, true] {
        let execution = ChatExecution::new(Scenario::CommitError);
        let mut body = text_request(streaming);
        body.as_object_mut()
            .expect("object")
            .remove("stream_options");
        let response = request(Arc::clone(&execution), body).await;
        assert!(!response.status().is_success());
        let body = read_text(response).await;
        assert!(!body.contains("\"object\":\"chat.completion"));
        assert!(execution.trace.is_cancelled());
        assert_eq!(execution.trace.client_statuses(), [500]);
        let errors = execution.trace.delivery_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind(), GatewayErrorKind::Internal);
    }
}

#[tokio::test]
async fn chat_conversion_failure_finalizes_delivery_with_the_actual_http_status_once() {
    for streaming in [false, true] {
        for scenario in [
            Scenario::ConversionErrorFirst,
            Scenario::ConversionErrorAfterText,
        ] {
            let execution = ChatExecution::new(scenario);
            let mut body = text_request(streaming);
            body.as_object_mut().unwrap().remove("stream_options");
            let response = request(Arc::clone(&execution), body).await;
            let committed = streaming && matches!(scenario, Scenario::ConversionErrorAfterText);
            let status = if committed {
                StatusCode::OK
            } else {
                StatusCode::BAD_GATEWAY
            };
            assert_eq!(response.status(), status);
            let body = read_text(response).await;
            assert!(body.contains("\"error\""));
            assert!(!body.contains("\"finish_reason\":\"stop\""));
            assert!(!body.contains("\"total_tokens\""));
            if committed {
                assert!(body.contains("hello"));
                assert!(body.ends_with("data: [DONE]\n\n"));
            } else {
                assert!(serde_json::from_str::<Value>(&body).unwrap()["error"].is_object());
            }
            assert_eq!(execution.trace.client_statuses(), [status.as_u16()]);
            let errors = execution.trace.delivery_errors();
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].kind(), GatewayErrorKind::UpstreamUnavailable);
            assert_eq!(
                errors[0].client_error_code(),
                Some("incompatible_upstream_response")
            );
            assert!(
                execution.trace.is_cancelled(),
                "remaining work is cancelled"
            );
            assert_eq!(execution.trace.snapshot().last(), Some(&"delivery_failed"));
        }
    }
}

#[tokio::test]
async fn chat_invalid_parameters_fail_before_account_execution() {
    let execution = ChatExecution::new(Scenario::Text);
    for body in [
        json!({"model":"model-a","messages":[]}),
        json!({"model":"model-a","messages":[{"role":"user","content":"hi"}],"n":2}),
        json!({"model":"model-a","messages":[{"role":"user","content":"hi"}],"stream":"yes"}),
    ] {
        let response = request(Arc::clone(&execution), body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert!(execution.captured.lock().expect("capture").is_empty());
}
