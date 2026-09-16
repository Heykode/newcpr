use super::chat::{
    ChatExecution, Scenario, chunks, read_text, request, response_events, text_request, wire,
};
use super::http::{FakeSession, NextStep, Trace, delivery_provider};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use axum::{
    body::{Body, Bytes},
    http::{
        Request, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE},
    },
};
use futures::{StreamExt, future::BoxFuture};
use gateway_core::engine::execution::ExecutionSession;
use gateway_core::engine::{CommitRequirement, CoordinatedEvent, EngineError};
use gateway_core::error::{
    ClientVisibleUpstreamError, ClientVisibleUpstreamResponse, GatewayError, GatewayErrorKind,
    ProviderError, ProviderErrorKind,
};
use gateway_core::event::{GatewayEvent, ProviderEvent, ProviderResponseHeader};
use gateway_core::upstream::UpstreamSendState;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tower::ServiceExt;

use crate::openai::api_router;

pub(super) struct Script {
    events: Vec<ProviderEvent>,
    steps: Option<Vec<NextStep>>,
    collect_error: Option<EngineError>,
    final_step: NextStep,
    gate: Option<(usize, oneshot::Receiver<()>)>,
    pending_failure: Option<GatewayError>,
}

impl Script {
    fn new(events: Vec<ProviderEvent>) -> Self {
        Self {
            events,
            steps: None,
            collect_error: None,
            final_step: NextStep::FinalizeSuccess,
            gate: None,
            pending_failure: None,
        }
    }

    pub(super) fn session(self, trace: Arc<Trace>, streaming: bool) -> ChatSession {
        let inner = if streaming {
            let mut steps = self.steps.unwrap_or_else(|| {
                self.events
                    .into_iter()
                    .map(|event| {
                        NextStep::Event(delivery_provider(
                            event,
                            CommitRequirement::CommitBeforeDelivery,
                        ))
                    })
                    .collect()
            });
            steps.push(self.final_step);
            FakeSession::streaming(trace, steps)
        } else {
            let inner = FakeSession::buffered_provider(trace, self.events);
            match self.collect_error {
                Some(error) => inner.with_collect_error(error),
                None => inner,
            }
        };
        ChatSession {
            gate: self.gate,
            pending_failure: self.pending_failure,
            ..ChatSession::new(inner)
        }
    }
}

// Keep the shared fake's trace, but enforce Core's pending-delivery handshake.
pub(super) struct ChatSession {
    inner: FakeSession,
    committed: bool,
    pending: bool,
    terminal: bool,
    pending_failure: Option<GatewayError>,
    cancelled: AtomicBool,
    delivered: usize,
    gate: Option<(usize, oneshot::Receiver<()>)>,
}

impl ChatSession {
    pub(super) fn new(inner: FakeSession) -> Self {
        Self {
            inner,
            committed: false,
            pending: false,
            terminal: false,
            pending_failure: None,
            cancelled: AtomicBool::new(false),
            delivered: 0,
            gate: None,
        }
    }
}

impl ExecutionSession for ChatSession {
    fn next_event(&mut self) -> BoxFuture<'_, Result<Option<CoordinatedEvent>, EngineError>> {
        Box::pin(async move {
            let cancelled = self.cancelled.load(Ordering::Acquire);
            if self.pending && !cancelled {
                return Err(EngineError::DownstreamCommitRequired);
            }
            if let Some((after_events, release)) = self.gate.as_mut()
                && self.delivered == *after_events
                && !cancelled
            {
                release.await.map_err(|_| EngineError::Cancelled)?;
                self.gate = None;
            }
            let Some(event) = self.inner.next_event().await? else {
                return Ok(None);
            };
            self.delivered += 1;
            let events = event.into_provider_events();
            self.terminal = events.iter().any(|event| {
                event
                    .canonical_facts()
                    .iter()
                    .any(|fact| matches!(fact, GatewayEvent::Completed(_)))
                    || event.wire_event().is_some_and(|wire| {
                        matches!(
                            wire.event_type(),
                            Some(
                                "response.completed"
                                    | "response.incomplete"
                                    | "response.failed"
                                    | "error"
                            )
                        )
                    })
            });
            self.pending = !self.committed;
            CoordinatedEvent::try_batch(
                events,
                if self.committed {
                    CommitRequirement::AlreadyCommitted
                } else {
                    CommitRequirement::CommitBeforeDelivery
                },
            )
            .map(Some)
        })
    }

    fn collect_uncommitted(&mut self) -> BoxFuture<'_, Result<Vec<ProviderEvent>, EngineError>> {
        Box::pin(async move {
            let events = self.inner.collect_uncommitted().await?;
            self.pending = true;
            self.terminal = true;
            Ok(events)
        })
    }

    fn response_headers(&self) -> &[ProviderResponseHeader] {
        self.inner.response_headers()
    }

    fn defer_downstream_commit(&mut self) -> Result<(), EngineError> {
        if !self.pending || self.committed || self.terminal || self.pending_failure.is_some() {
            return Err(EngineError::InvalidDeliveryState);
        }
        self.inner.defer_downstream_commit()?;
        self.pending = false;
        Ok(())
    }

    fn commit_downstream(
        &mut self,
        client_status_code: Option<u16>,
    ) -> BoxFuture<'_, Result<(), EngineError>> {
        Box::pin(async move {
            if !self.pending || self.committed {
                return Err(EngineError::InvalidDeliveryState);
            }
            self.inner.commit_downstream(client_status_code).await?;
            self.pending = false;
            self.committed = true;
            Ok(())
        })
    }

    fn record_client_status(
        &mut self,
        client_status_code: u16,
    ) -> BoxFuture<'_, Result<(), EngineError>> {
        self.inner.record_client_status(client_status_code)
    }

    fn is_finalized(&self) -> bool {
        self.inner.is_finalized()
    }

    fn fail_delivery(&mut self, error: GatewayError) -> BoxFuture<'_, Result<(), EngineError>> {
        // Mirror Core's pending-provider-error precedence without polling an
        // additional event after the adapter has already rejected delivery.
        self.inner
            .fail_delivery(self.pending_failure.take().unwrap_or(error))
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.inner.cancel();
    }

    fn detach_finalize(self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::new(self.inner).detach_finalize()
    }
}

fn scripted(script: Script) -> Arc<ChatExecution> {
    let execution = ChatExecution::new(Scenario::Text);
    *execution.script.lock().expect("script") = Some(script);
    execution
}

fn created() -> ProviderEvent {
    response_events(Scenario::Text).remove(0)
}

fn completed(output: Value) -> ProviderEvent {
    wire(
        "response.completed",
        json!({"type":"response.completed","response":{
            "id":"resp_chat","model":"model-a","created_at":1700000000,
            "status":"completed","output":output,
            "usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13}
        }}),
    )
}

fn text_delta(text: &str) -> ProviderEvent {
    wire(
        "response.output_text.delta",
        json!({"type":"response.output_text.delta","item_id":"msg_1",
            "output_index":0,"content_index":0,"delta":text}),
    )
}

fn message(text: &str) -> Value {
    json!({"type":"message","id":"msg_1","role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":text,"annotations":[]}]})
}

fn unknown() -> ProviderEvent {
    wire(
        "response.future_event",
        json!({"type":"response.future_event","future":{"opaque":9007199254740993_u64}}),
    )
}

fn body_for(streaming: bool) -> Value {
    let mut body = text_request(streaming);
    if !streaming {
        body.as_object_mut()
            .expect("request")
            .remove("stream_options");
    }
    body
}

fn streamed_content(values: &[Value]) -> String {
    values
        .iter()
        .filter_map(|value| value["choices"][0]["delta"]["content"].as_str())
        .collect()
}

fn assert_finished_once(body: &str) {
    assert!(!body.contains("\"error\""), "{body}");
    assert_eq!(body.matches("\"finish_reason\":\"stop\"").count(), 1);
    assert_eq!(
        chunks(body)
            .iter()
            .filter(|value| value["usage"].is_object())
            .count(),
        1
    );
    assert!(body.ends_with("data: [DONE]\n\n"));
}

#[tokio::test]
async fn unknown_preamble_is_skipped_before_successful_text() {
    let execution = ChatExecution::new(Scenario::UnknownPreamble);
    let response = request(Arc::clone(&execution), text_request(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        execution.trace.snapshot(),
        [
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
    assert_eq!(streamed_content(&chunks(&body)), "hello");
    assert!(!body.contains("unknown_semantic_event"));
    assert_finished_once(&body);
}

#[tokio::test]
async fn first_actual_text_is_delivered_while_terminal_is_still_gated() {
    let (release_terminal, terminal_gate) = oneshot::channel();
    let mut script = Script::new(vec![
        created(),
        wire(
            "response.queued",
            json!({"type":"response.queued","response":{"status":"queued"}}),
        ),
        wire(
            "response.in_progress",
            json!({"type":"response.in_progress","response":{"status":"in_progress"}}),
        ),
        unknown(),
        text_delta("first actual text"),
        completed(json!([message("first actual text")])),
    ]);
    script.gate = Some((5, terminal_gate));
    let execution = scripted(script);
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        request(Arc::clone(&execution), text_request(true)),
    )
    .await
    .expect("first content must not wait for terminal release");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        execution.trace.snapshot(),
        [
            "next_event",
            "defer_commit",
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
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("content must arrive while the terminal gate is closed")
        .expect("first frame")
        .expect("body frame");
    let first = std::str::from_utf8(&first).expect("UTF-8");
    let values = chunks(first);
    assert_eq!(values.len(), 1, "metadata must not emit empty role chunks");
    assert_eq!(
        values[0]["choices"][0]["delta"]["content"],
        "first actual text"
    );
    assert!(values[0]["choices"][0]["finish_reason"].is_null());
    assert!(values[0]["usage"].is_null());
    assert!(!first.contains("[DONE]"));

    // Only the reader of real content can release the later terminal event.
    release_terminal.send(()).expect("terminal still waiting");
    let mut body = first.to_owned();
    while let Some(frame) = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("released stream must finish")
    {
        body.push_str(std::str::from_utf8(&frame.expect("body frame")).expect("UTF-8"));
    }
    assert_eq!(streamed_content(&chunks(&body)), "first actual text");
    assert_finished_once(&body);
    assert_eq!(execution.trace.client_statuses(), [200]);
    assert_eq!(
        &execution.trace.snapshot()[10..],
        ["next_event", "next_end"]
    );
    assert!(!execution.trace.is_cancelled());
}

#[tokio::test]
async fn empty_or_missing_text_and_reasoning_deltas_do_not_commit() {
    for kind in [
        "response.output_text.delta",
        "response.reasoning_text.delta",
        "response.reasoning_summary_text.delta",
    ] {
        for delta in [None, Some(Value::Null), Some(json!(""))] {
            let mut data =
                json!({"type":kind,"item_id":"msg_1","output_index":0,"content_index":0});
            if let Some(delta) = delta {
                data["delta"] = delta;
            }
            let execution = scripted(Script::new(vec![created(), wire(kind, data)]));
            let response = request(Arc::clone(&execution), text_request(true)).await;
            assert!(response.status().is_server_error());
            assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
            let body: Value = serde_json::from_str(&read_text(response).await).expect("error JSON");
            assert!(body["error"].is_object());
            assert!(body.get("choices").is_none());
            assert_eq!(
                execution.trace.snapshot(),
                [
                    "next_event",
                    "defer_commit",
                    "next_event",
                    "defer_commit",
                    "next_end"
                ]
            );
        }
    }
}

#[tokio::test]
async fn chat_uses_body_terminal_even_when_the_sse_header_is_unknown() {
    let terminal = completed(json!([]))
        .wire_event()
        .expect("terminal")
        .data()
        .clone();
    let execution = scripted(Script::new(vec![
        created(),
        text_delta("hello"),
        wire("response.future_event", terminal),
    ]));
    let response = request(Arc::clone(&execution), text_request(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_text(response).await;
    assert_eq!(streamed_content(&chunks(&body)), "hello");
    assert_finished_once(&body);
    assert_eq!(
        execution.trace.snapshot(),
        [
            "next_event",
            "defer_commit",
            "next_event",
            "commit",
            "next_event",
            "next_end"
        ]
    );
    assert!(!execution.trace.is_cancelled());
}

#[tokio::test]
async fn terminal_sse_header_with_unknown_body_does_not_turn_eof_into_success() {
    let execution = scripted(Script::new(vec![
        created(),
        text_delta("hello"),
        wire(
            "response.completed",
            json!({
                "type":"response.future_event","response":{"id":"resp_chat","status":"completed","output":[]}
            }),
        ),
    ]));
    let response = request(Arc::clone(&execution), text_request(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_text(response).await;
    let values = chunks(&body);
    assert_eq!(streamed_content(&values), "hello");
    assert_eq!(
        values
            .iter()
            .filter(|value| value["error"].is_object())
            .count(),
        1
    );
    assert!(!body.contains("\"finish_reason\":\"stop\""));
    assert!(!body.contains("\"total_tokens\""));
    assert!(body.ends_with("data: [DONE]\n\n"));
    assert_eq!(
        execution.trace.snapshot(),
        [
            "next_event",
            "defer_commit",
            "next_event",
            "commit",
            "next_event",
            "next_end"
        ]
    );
}

#[tokio::test]
async fn buffered_and_streaming_chat_classify_terminal_headers_and_bodies_consistently() {
    for (header, body_kind, succeeds) in [
        ("response.future_event", "response.completed", true),
        ("response.completed", "response.future_event", false),
        ("response.failed", "response.completed", false),
        ("response.completed", "response.failed", false),
        ("response.completed", "response.cancelled", false),
        ("error", "response.completed", false),
        ("response.completed", "error", false),
    ] {
        for streaming in [false, true] {
            let mut terminal = completed(json!([message("hello")]))
                .wire_event()
                .expect("terminal")
                .data()
                .clone();
            terminal["type"] = json!(body_kind);
            let execution = scripted(Script::new(vec![
                created(),
                text_delta("hello"),
                wire(header, terminal),
            ]));
            let response = request(Arc::clone(&execution), body_for(streaming)).await;
            assert_eq!(
                response.status(),
                if streaming || succeeds {
                    StatusCode::OK
                } else {
                    StatusCode::BAD_GATEWAY
                },
                "header={header} body={body_kind} streaming={streaming}"
            );
            let body = read_text(response).await;
            if streaming {
                assert_eq!(streamed_content(&chunks(&body)), "hello");
                if succeeds {
                    assert_finished_once(&body);
                } else {
                    assert!(body.contains("\"error\""), "{body}");
                    assert!(!body.contains("\"finish_reason\":\"stop\""), "{body}");
                    assert!(!body.contains("\"total_tokens\""), "{body}");
                    assert!(body.ends_with("data: [DONE]\n\n"));
                }
            } else {
                let value: Value = serde_json::from_str(&body).expect("buffered JSON");
                if succeeds {
                    assert_eq!(value["choices"][0]["message"]["content"], "hello");
                    assert_eq!(value["choices"][0]["finish_reason"], "stop");
                    assert_eq!(value["usage"]["total_tokens"], 13);
                    assert_eq!(execution.trace.snapshot(), ["collect", "commit"]);
                } else {
                    assert!(value["error"].is_object(), "{body}");
                    assert!(value.get("choices").is_none());
                    assert_eq!(execution.trace.snapshot(), ["collect", "delivery_failed"]);
                }
            }
        }
    }
}

#[tokio::test]
async fn buffered_and_streaming_chat_never_hide_a_failure_before_a_later_terminal() {
    for streaming in [false, true] {
        for failure in [
            wire(
                "response.future_event",
                json!({"type":"response.failed","response":{"status":"completed","output":[]}}),
            ),
            wire(
                "response.in_progress",
                json!({"type":"response.in_progress","response":{"status":"failed"}}),
            ),
        ] {
            let execution = scripted(Script::new(vec![created(), failure, completed(json!([]))]));
            let response = request(Arc::clone(&execution), body_for(streaming)).await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            let value: Value =
                serde_json::from_str(&read_text(response).await).expect("JSON error");
            assert!(value["error"].is_object());
            assert!(value.get("choices").is_none());
            assert!(!execution.trace.snapshot().contains(&"commit"));
        }
    }
}

#[tokio::test]
async fn explicit_failure_in_either_sse_header_or_body_cannot_be_skipped() {
    for failure_header in [false, true] {
        let (header, body_kind) = if failure_header {
            ("response.failed", "response.future_event")
        } else {
            ("response.future_event", "response.failed")
        };
        let mut script = Script::new(vec![
            created(),
            wire(
                header,
                json!({
                    "type":body_kind,"response":{"id":"resp_chat","status":"failed",
                        "error":{"code":"server_error","message":"failure remains authoritative"}}
                }),
            ),
        ]);
        script.final_step = NextStep::Error(EngineError::Deadline);
        let execution = scripted(script);
        let response = request(Arc::clone(&execution), text_request(true)).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body: Value = serde_json::from_str(&read_text(response).await).expect("error JSON");
        assert!(body["error"].is_object());
        assert_eq!(
            execution.trace.snapshot(),
            [
                "next_event",
                "defer_commit",
                "next_event",
                "delivery_failed"
            ]
        );
        let errors = execution.trace.delivery_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind(), GatewayErrorKind::UpstreamUnavailable);
        assert_eq!(
            errors[0].client_error_code(),
            Some("incompatible_upstream_response")
        );
        assert_eq!(execution.trace.client_statuses(), [502]);
        assert!(execution.trace.is_cancelled());
    }
}

#[tokio::test]
async fn metadata_and_unknown_events_followed_by_eof_never_commit_success() {
    for final_step in [NextStep::End, NextStep::FinalizeSuccess] {
        let mut script = Script::new(vec![
            created(),
            wire("response.queued", json!({"type":"response.queued"})),
            wire(
                "response.in_progress",
                json!({"type":"response.in_progress"}),
            ),
            unknown(),
        ]);
        script.final_step = final_step;
        let execution = scripted(script);
        let response = request(Arc::clone(&execution), text_request(true)).await;
        assert!(response.status().is_server_error());
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        let body: Value = serde_json::from_str(&read_text(response).await).expect("error JSON");
        assert!(body["error"].is_object());
        assert!(body.get("choices").is_none());
        let trace = execution.trace.snapshot();
        assert_eq!(
            &trace[..8],
            [
                "next_event",
                "defer_commit",
                "next_event",
                "defer_commit",
                "next_event",
                "defer_commit",
                "next_event",
                "defer_commit"
            ]
        );
        assert!(!trace.contains(&"commit"));
        assert!(
            execution
                .trace
                .client_statuses()
                .iter()
                .all(|status| *status >= 500)
        );
    }
}

#[tokio::test]
async fn actual_empty_completed_output_is_not_an_extra_converter_error() {
    for streaming in [false, true] {
        let execution = scripted(Script::new(vec![
            created(),
            unknown(),
            completed(json!([])),
        ]));
        let response = request(Arc::clone(&execution), body_for(streaming)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = read_text(response).await;
        assert!(!body.contains("\"error\""), "{body}");
        if streaming {
            assert_eq!(streamed_content(&chunks(&body)), "");
            assert_finished_once(&body);
            assert_eq!(
                execution.trace.snapshot(),
                [
                    "next_event",
                    "defer_commit",
                    "next_event",
                    "defer_commit",
                    "next_event",
                    "commit",
                    "next_end"
                ]
            );
        } else {
            let value: Value = serde_json::from_str(&body).expect("Chat JSON");
            assert_eq!(value["choices"][0]["finish_reason"], "stop");
            assert_eq!(value["usage"]["total_tokens"], 13);
            assert_eq!(execution.trace.snapshot(), ["collect", "commit"]);
        }
        assert_eq!(execution.trace.client_statuses(), [200]);
        assert!(!execution.trace.is_cancelled());
    }
}

#[tokio::test]
async fn text_done_content_and_terminal_snapshots_are_not_reconstructed() {
    let events = vec![
        created(),
        wire(
            "response.content_part.added",
            json!({
                "type":"response.content_part.added","item_id":"msg_1","output_index":0,"content_index":0,
                "part":{"type":"output_text","text":"added snapshot","annotations":[]}
            }),
        ),
        text_delta("direct "),
        unknown(),
        text_delta("delta"),
        wire(
            "response.output_text.done",
            json!({
                "type":"response.output_text.done","item_id":"msg_1","output_index":0,"content_index":0,
                "text":"a completely different done snapshot"
            }),
        ),
        wire(
            "response.content_part.done",
            json!({
                "type":"response.content_part.done","item_id":"msg_1","output_index":0,"content_index":0,
                "part":{"type":"output_text","text":"content snapshot","annotations":[]}
            }),
        ),
        wire(
            "response.output_item.done",
            json!({
                "type":"response.output_item.done","output_index":0,"item":message("item snapshot")
            }),
        ),
        completed(json!([message("terminal snapshot")])),
    ];
    let execution = scripted(Script::new(events));
    let response = request(Arc::clone(&execution), text_request(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_text(response).await;
    assert_eq!(streamed_content(&chunks(&body)), "direct delta");
    assert!(!body.contains("snapshot"));
    assert_finished_once(&body);
    assert!(!execution.trace.is_cancelled());
}

#[tokio::test]
async fn images_emit_from_partial_and_item_done_but_never_from_completed_snapshot() {
    let image = |id: &str, result: &str| {
        json!({
            "type":"image_generation_call","id":id,"result":result,
            "output_format":"png","status":"completed"
        })
    };
    let mut events = vec![created()];
    for result in ["preview-a!", "preview-a!", "preview-b!", "preview-a!"] {
        events.push(wire(
            "response.image_generation_call.partial_image",
            json!({
                "type":"response.image_generation_call.partial_image","item_id":"img_1",
                "partial_image_b64":result,"output_format":"png"
            }),
        ));
    }
    for result in ["done-result!", "done-result!"] {
        events.push(wire(
            "response.output_item.done",
            json!({
                "type":"response.output_item.done","output_index":0,"item":image("img_1", result)
            }),
        ));
    }
    events.push(completed(json!([
        image("img_1", "changed-terminal-only!"),
        image("img_2", "unseen-terminal-only!")
    ])));
    let execution = scripted(Script::new(events));
    let response = request(Arc::clone(&execution), text_request(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_text(response).await;
    let values = chunks(&body);
    let urls = values
        .iter()
        .filter_map(|value| value["choices"][0]["delta"]["images"].as_array())
        .flatten()
        .map(|image| image["image_url"]["url"].as_str().expect("image URL"))
        .collect::<Vec<_>>();
    assert_eq!(
        urls,
        [
            "data:image/png;base64,preview-a!",
            "data:image/png;base64,preview-b!",
            "data:image/png;base64,preview-a!",
            "data:image/png;base64,done-result!",
        ]
    );
    assert_eq!(streamed_content(&values), "");
    assert!(!body.contains("terminal-only"));
    assert_finished_once(&body);
    assert!(!execution.trace.is_cancelled());
}

#[tokio::test]
async fn explicit_wire_failures_remain_errors_before_and_after_content() {
    for kind in ["response.failed", "error"] {
        for after_content in [false, true] {
            let failure = if kind == "error" {
                json!({"type":"error","error":{"code":"server_error","message":"upstream failed"}})
            } else {
                json!({"type":"response.failed","response":{
                    "id":"resp_chat","status":"failed",
                    "error":{"code":"server_error","message":"upstream failed"}
                }})
            };
            let mut events = vec![created(), unknown()];
            if after_content {
                events.push(text_delta("visible first"));
            }
            events.push(wire(kind, failure));
            let mut script = Script::new(events);
            script.final_step = NextStep::Error(EngineError::Provider(ProviderError::new(
                ProviderErrorKind::Unavailable,
                UpstreamSendState::Sent,
            )));
            let execution = scripted(script);
            let response = request(Arc::clone(&execution), text_request(true)).await;
            assert_eq!(
                response.status(),
                if after_content {
                    StatusCode::OK
                } else {
                    StatusCode::BAD_GATEWAY
                }
            );
            let body = read_text(response).await;
            assert!(body.contains("\"error\""), "{body}");
            assert!(!body.contains("\"finish_reason\":\"stop\""));
            assert!(!body.contains("\"total_tokens\""));
            let trace = execution.trace.snapshot();
            assert_eq!(
                trace.iter().filter(|event| **event == "commit").count(),
                usize::from(after_content)
            );
            assert_eq!(
                trace
                    .iter()
                    .filter(|event| **event == "delivery_failed")
                    .count(),
                1
            );
            assert!(!trace.contains(&"next_error"));
            let errors = execution.trace.delivery_errors();
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].kind(), GatewayErrorKind::UpstreamUnavailable);
            assert_eq!(
                errors[0].client_error_code(),
                Some("incompatible_upstream_response")
            );
            assert_eq!(
                execution.trace.client_statuses(),
                [if after_content { 200 } else { 502 }]
            );
            assert!(execution.trace.is_cancelled());
            if after_content {
                assert_eq!(
                    chunks(&body)
                        .iter()
                        .filter(|value| value["error"].is_object())
                        .count(),
                    1
                );
                assert!(body.ends_with("data: [DONE]\n\n"));
            } else {
                let value: Value = serde_json::from_str(&body).expect("precommit JSON error");
                assert!(value["error"].is_object());
                assert!(value.get("choices").is_none());
            }
        }
    }
}

#[tokio::test]
async fn pending_atomic_failure_is_not_discarded_when_its_metadata_is_skipped() {
    for explicit_failure in [false, true] {
        let mut events = vec![created(), unknown()];
        if explicit_failure {
            events.push(wire(
                "response.failed",
                json!({
                    "type":"response.failed","response":{"id":"resp_chat","status":"failed",
                        "error":{"code":"rate_limit_exceeded","message":"atomic upstream failure"}}
                }),
            ));
        }
        let mut script = Script::new(Vec::new());
        script.steps = Some(vec![NextStep::Event(
            CoordinatedEvent::try_batch(events, CommitRequirement::CommitBeforeDelivery)
                .expect("atomic failure"),
        )]);
        let provider_error =
            ProviderError::new(ProviderErrorKind::RateLimited, UpstreamSendState::Sent)
                .with_status(429)
                .with_client_visible_upstream_error(ClientVisibleUpstreamError::new(
                    "atomic upstream failure",
                    Some("rate_limit_exceeded".to_owned()),
                    Some("rate_limit_error".to_owned()),
                ));
        script.pending_failure = Some(GatewayError::from_provider(&provider_error));
        script.final_step = NextStep::Error(EngineError::Provider(provider_error));
        let execution = scripted(script);
        let response = request(Arc::clone(&execution), text_request(true)).await;
        let expected_status = if explicit_failure { 502 } else { 500 };
        assert_eq!(response.status().as_u16(), expected_status);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        let body: Value = serde_json::from_str(&read_text(response).await).expect("JSON error");
        assert!(body["error"].is_object());
        assert!(body.get("choices").is_none());
        assert_eq!(
            execution.trace.snapshot(),
            ["next_event", "delivery_failed"]
        );
        let errors = execution.trace.delivery_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind(), GatewayErrorKind::RateLimited);
        assert_eq!(errors[0].client_error_code(), Some("rate_limit_exceeded"));
        assert_eq!(errors[0].client_message(), "atomic upstream failure");
        assert_eq!(execution.trace.client_statuses(), [expected_status]);
        assert!(execution.trace.is_cancelled());
    }
}

#[tokio::test]
async fn terminal_does_not_hide_a_pending_execution_failure_with_or_without_prior_content() {
    for after_content in [false, true] {
        let mut events = vec![created()];
        if after_content {
            events.push(text_delta("hello"));
        }
        events.push(completed(json!([])));
        let mut script = Script::new(events);
        script.final_step = NextStep::Error(EngineError::Deadline);
        let execution = scripted(script);
        let response = request(Arc::clone(&execution), text_request(true)).await;
        let body = read_text(response).await;
        assert_eq!(
            streamed_content(&chunks(&body)),
            if after_content { "hello" } else { "" }
        );
        assert!(body.contains("\"error\""));
        assert!(!body.contains("\"finish_reason\":\"stop\""), "{body}");
        assert!(!body.contains("\"total_tokens\""));
        let trace = execution.trace.snapshot();
        assert_eq!(trace.iter().filter(|event| **event == "commit").count(), 1);
        assert_eq!(
            trace.iter().filter(|event| **event == "next_error").count(),
            1
        );
        assert!(
            !execution.trace.is_cancelled(),
            "the execution error already finalized the session"
        );
    }
}

#[tokio::test]
async fn first_atomic_chat_batch_holds_only_terminal_frames_until_execution_success() {
    for succeeds in [false, true] {
        let mut script = Script::new(Vec::new());
        script.steps = Some(vec![NextStep::Event(
            CoordinatedEvent::try_batch(
                vec![created(), text_delta("hello"), completed(json!([]))],
                CommitRequirement::CommitBeforeDelivery,
            )
            .expect("atomic terminal batch"),
        )]);
        if !succeeds {
            script.final_step = NextStep::Error(EngineError::Deadline);
        }
        let execution = scripted(script);
        let response = request(Arc::clone(&execution), text_request(true)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = read_text(response).await;
        let values = chunks(&body);
        assert_eq!(streamed_content(&values), "hello");
        if succeeds {
            assert_finished_once(&body);
        } else {
            assert_eq!(
                values
                    .iter()
                    .filter(|value| value["error"].is_object())
                    .count(),
                1
            );
            assert!(!body.contains("\"finish_reason\":\"stop\""), "{body}");
            assert!(!body.contains("\"total_tokens\""), "{body}");
            assert!(body.ends_with("data: [DONE]\n\n"));
        }
        assert_eq!(
            execution.trace.snapshot(),
            [
                "next_event",
                "commit",
                if succeeds { "next_end" } else { "next_error" }
            ]
        );
        assert!(!execution.trace.is_cancelled());
    }
}

#[tokio::test]
async fn completed_snapshot_only_is_available_to_buffered_chat_but_not_replayed_in_stream() {
    let output = json!([
        message("snapshot-only text"),
        {"type":"image_generation_call","id":"img_1","result":"snapshot-image!",
            "output_format":"png","status":"completed"}
    ]);
    for streaming in [false, true] {
        let execution = scripted(Script::new(vec![created(), completed(output.clone())]));
        let response = request(Arc::clone(&execution), body_for(streaming)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = read_text(response).await;
        if streaming {
            assert_eq!(streamed_content(&chunks(&body)), "");
            assert!(!body.contains("snapshot-only"));
            assert!(!body.contains("snapshot-image"));
            assert!(!body.contains("\"images\""));
            assert_finished_once(&body);
        } else {
            let value: Value = serde_json::from_str(&body).expect("buffered Chat");
            assert_eq!(
                value["choices"][0]["message"]["content"],
                "snapshot-only text"
            );
            assert_eq!(
                value["choices"][0]["message"]["images"][0]["image_url"]["url"],
                "data:image/png;base64,snapshot-image!"
            );
        }
        assert_eq!(execution.trace.client_statuses(), [200]);
        assert!(!execution.trace.is_cancelled());
    }
}

#[tokio::test]
async fn buffered_metadata_and_text_without_terminal_remain_an_error() {
    let execution = scripted(Script::new(vec![
        created(),
        unknown(),
        text_delta("unfinished"),
    ]));
    let response = request(Arc::clone(&execution), body_for(false)).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = serde_json::from_str(&read_text(response).await).expect("JSON error");
    assert!(body["error"].is_object());
    assert!(body.get("choices").is_none());
    assert_eq!(execution.trace.snapshot(), ["collect", "delivery_failed"]);
    assert!(execution.trace.is_cancelled());
}

#[tokio::test]
async fn native_responses_preserves_annotations_and_unknown_events_while_chat_omits_them() {
    let annotation = json!({"type":"url_citation","start_index":0,"end_index":5,
        "url":"https://example.test/source","title":"opaque citation","future_annotation":true});
    let mut output = message("hello");
    output["content"][0]["annotations"] = json!([annotation]);
    let annotation_event = wire(
        "response.output_text.annotation.added",
        json!({
            "type":"response.output_text.annotation.added","item_id":"msg_1","output_index":0,
            "content_index":0,"annotation_index":0,"annotation":annotation
        }),
    );
    let events = vec![
        created(),
        unknown(),
        text_delta("hello"),
        annotation_event,
        completed(json!([output])),
    ];

    for streaming in [false, true] {
        for native in [false, true] {
            let execution = scripted(Script::new(events.clone()));
            let response = if native {
                api_router(execution.clone())
                    .await
                    .oneshot(
                        Request::post("/v1/responses")
                            .header(AUTHORIZATION, "Bearer sk_chat_test")
                            .header(CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                json!({"model":"model-a","input":"hi","stream":streaming})
                                    .to_string(),
                            ))
                            .expect("native request"),
                    )
                    .await
                    .expect("native handler")
            } else {
                request(Arc::clone(&execution), body_for(streaming)).await
            };
            assert_eq!(response.status(), StatusCode::OK);
            let body = read_text(response).await;
            if native && streaming {
                let values = chunks(&body);
                let expected: Vec<_> = events
                    .iter()
                    .map(|event| event.wire_event().expect("wire").data().clone())
                    .collect();
                assert_eq!(
                    values, expected,
                    "native event JSON must remain transparent"
                );
                assert!(body.contains("event: response.output_text.annotation.added"));
                assert!(body.contains("event: response.future_event"));
                assert!(body.ends_with("data: [DONE]\n\n"));
            } else if native {
                let value: Value = serde_json::from_str(&body).expect("native JSON");
                assert_eq!(
                    value["output"][0]["content"][0]["annotations"],
                    json!([annotation])
                );
            } else {
                assert!(!body.contains("annotations"));
                assert!(!body.contains("url_citation"));
                assert!(!body.contains("opaque citation"));
                assert!(!body.contains("future_event"));
                if streaming {
                    assert_eq!(streamed_content(&chunks(&body)), "hello");
                    assert_finished_once(&body);
                } else {
                    let value: Value = serde_json::from_str(&body).expect("Chat JSON");
                    assert_eq!(value["choices"][0]["message"]["content"], "hello");
                }
            }
            assert_eq!(execution.trace.client_statuses(), [200]);
            assert!(!execution.trace.is_cancelled());
        }
    }
}

async fn recovery_request(
    execution: Arc<ChatExecution>,
    native: bool,
    streaming: bool,
) -> axum::response::Response {
    if !native {
        return request(execution, body_for(streaming)).await;
    }
    api_router(execution)
        .await
        .oneshot(
            Request::post("/v1/responses")
                .header(AUTHORIZATION, "Bearer sk_chat_test")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"model":"model-a","input":"hi","stream":streaming}).to_string(),
                ))
                .expect("native recovery request"),
        )
        .await
        .expect("native recovery handler")
}

#[tokio::test]
async fn buffered_recovery_delivers_raw_done_items_without_changing_streaming() {
    let tool = json!({"type":"function_call","id":"fc","call_id":"call-a","name":"lookup",
        "arguments":"{\"q\":1}","status":"completed","future_tool":true});
    let search = json!({"type":"web_search_call","id":"search","status":"completed",
        "action":{"type":"search","query":"q"},"future_search":{"keep":true}});
    let image = json!({"type":"image_generation_call","id":"img","status":"completed",
        "result":"opaque-image!","output_format":"png","future_image":[1,2]});
    let future = json!({"type":"future_output","opaque":{"keep":true}});
    let output = json!([message("answer"), tool, search, image, future]);
    let events = vec![
        created(),
        text_delta("answer"),
        wire(
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":1,"item":{
                "type":"function_call","id":"fc","call_id":"call-a","name":"lookup","arguments":""
            }}),
        ),
        wire(
            "response.function_call_arguments.delta",
            json!({"type":"response.function_call_arguments.delta","item_id":"fc",
                "output_index":1,"delta":"{\"q\":1}"}),
        ),
        wire(
            "response.output_item.done",
            json!({"type":"response.output_item.done","output_index":3,"item":image}),
        ),
        wire(
            "response.output_item.done",
            json!({"type":"response.output_item.done","item":future}),
        ),
        wire(
            "response.output_item.done",
            json!({"type":"response.output_item.done","output_index":1,"item":tool}),
        ),
        wire(
            "response.output_item.done",
            json!({"type":"response.output_item.done","output_index":0,"item":message("answer")}),
        ),
        wire(
            "response.output_item.done",
            json!({"type":"response.output_item.done","output_index":2,"item":search}),
        ),
        wire(
            "response.output_item.done",
            json!({"type":"response.output_item.done","output_index":3,"item":image}),
        ),
        completed(json!([])),
    ];
    for native in [false, true] {
        for streaming in [false, true] {
            let execution = scripted(Script::new(events.clone()));
            let response = recovery_request(Arc::clone(&execution), native, streaming).await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = read_text(response).await;
            if streaming {
                let values = chunks(&body);
                if native {
                    let expected: Vec<_> = events
                        .iter()
                        .map(|event| event.wire_event().unwrap().data().clone())
                        .collect();
                    assert_eq!(values, expected, "native frames must not be repaired");
                    assert_eq!(values.last().unwrap()["response"]["output"], json!([]));
                } else {
                    assert_eq!(streamed_content(&values), "answer");
                    assert_eq!(
                        values.iter().filter_map(|chunk|
                            chunk["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]
                                .as_str()
                        ).collect::<String>(),
                        "{\"q\":1}"
                    );
                    assert_eq!(
                        values
                            .iter()
                            .filter(|chunk| chunk["choices"][0]["delta"]["images"].is_array())
                            .count(),
                        1
                    );
                    assert_eq!(body.matches("\"finish_reason\":\"tool_calls\"").count(), 1);
                }
                assert!(body.ends_with("data: [DONE]\n\n"));
            } else {
                let value: Value = serde_json::from_str(&body).expect("buffered recovery JSON");
                if native {
                    assert_eq!(value["output"], output);
                    assert_eq!(value["usage"]["total_tokens"], 13);
                } else {
                    assert_eq!(value["choices"][0]["message"]["content"], "answer");
                    assert_eq!(
                        value["choices"][0]["message"]["tool_calls"][0]["id"],
                        "call-a"
                    );
                    assert_eq!(
                        value["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
                        "{\"q\":1}"
                    );
                    let images = value["choices"][0]["message"]["images"].as_array().unwrap();
                    assert_eq!(images.len(), 1);
                    assert_eq!(
                        images[0]["image_url"]["url"],
                        "data:image/png;base64,opaque-image!"
                    );
                    assert_eq!(value["usage"]["total_tokens"], 13);
                    assert!(!body.contains("future_search"));
                }
            }
            assert_eq!(execution.trace.client_statuses(), [200]);
            assert!(!execution.trace.is_cancelled());
        }
    }
}

#[tokio::test]
async fn buffered_recovery_delivers_deltas_but_keeps_a_nonempty_terminal_authoritative() {
    for native in [false, true] {
        for authoritative in [false, true] {
            let output = if authoritative {
                json!([message("terminal")])
            } else {
                json!([])
            };
            let events = vec![
                created(),
                text_delta("hello"),
                text_delta(" world"),
                completed(output),
            ];
            let execution = scripted(Script::new(events));
            let response = recovery_request(Arc::clone(&execution), native, false).await;
            assert_eq!(response.status(), StatusCode::OK);
            let value: Value = serde_json::from_str(&read_text(response).await).expect("JSON");
            let text = if native {
                assert_eq!(value["output"][0]["id"], "msg_1");
                &value["output"][0]["content"][0]["text"]
            } else {
                &value["choices"][0]["message"]["content"]
            };
            assert_eq!(
                text,
                if authoritative {
                    "terminal"
                } else {
                    "hello world"
                }
            );
            assert_eq!(execution.trace.client_statuses(), [200]);
            assert!(!execution.trace.is_cancelled());
        }
    }
}

#[tokio::test]
async fn buffered_recovery_keeps_mixed_items_and_native_terminal_without_status() {
    for native in [false, true] {
        for missing_status in [false, true] {
            if missing_status && !native {
                continue;
            }
            let mut final_response = json!({
                "id":"resp_chat","model":"model-a","status":"completed","output":[]
            });
            if missing_status {
                final_response.as_object_mut().unwrap().remove("status");
            }
            let events = vec![
                created(),
                text_delta("delta-only"),
                wire(
                    "response.output_item.done",
                    json!({"type":"response.output_item.done","output_index":1,"item":{
                        "type":"message","id":"second","role":"assistant",
                        "content":[{"type":"output_text","text":" raw"}]
                    }}),
                ),
                wire(
                    "response.completed",
                    json!({"type":"response.completed","response":final_response}),
                ),
            ];
            let execution = scripted(Script::new(events));
            let response = recovery_request(execution, native, false).await;
            assert_eq!(response.status(), StatusCode::OK);
            let value: Value = serde_json::from_str(&read_text(response).await).expect("JSON");
            if native {
                assert_eq!(value["output"].as_array().unwrap().len(), 2);
                assert_eq!(value["output"][0]["content"][0]["text"], "delta-only");
                assert_eq!(value["output"][1]["content"][0]["text"], " raw");
                assert_eq!(value.get("status").is_none(), missing_status);
            } else {
                assert_eq!(value["choices"][0]["message"]["content"], "delta-only raw");
            }
        }
    }
}

#[tokio::test]
async fn buffered_recovery_never_assigns_other_call_arguments_or_finishes_without_terminal() {
    let events = vec![
        created(),
        wire(
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":0,"item":{
                "type":"function_call","id":"fc-a","call_id":"call-a","name":"lookup","arguments":""
            }}),
        ),
        wire(
            "response.function_call_arguments.delta",
            json!({"type":"response.function_call_arguments.delta","item_id":"fc-a",
                "output_index":0,"delta":"must stay with A"}),
        ),
        wire(
            "response.output_item.done",
            json!({"type":"response.output_item.done","output_index":0,"item":{
                "type":"function_call","id":"fc-b","call_id":"call-b","name":"lookup","arguments":""
            }}),
        ),
    ];
    for native in [false, true] {
        for has_terminal in [false, true] {
            let mut events = events.clone();
            if has_terminal {
                events.push(completed(json!([])));
            }
            let execution = scripted(Script::new(events));
            let response = recovery_request(execution, native, false).await;
            assert_eq!(
                response.status(),
                if has_terminal {
                    StatusCode::OK
                } else if native {
                    StatusCode::INTERNAL_SERVER_ERROR
                } else {
                    StatusCode::BAD_GATEWAY
                }
            );
            let value: Value = serde_json::from_str(&read_text(response).await).expect("JSON");
            if has_terminal {
                let (call, arguments) = if native {
                    (
                        &value["output"][0]["call_id"],
                        &value["output"][0]["arguments"],
                    )
                } else {
                    let tool = &value["choices"][0]["message"]["tool_calls"][0];
                    (&tool["id"], &tool["function"]["arguments"])
                };
                assert_eq!(call, "call-b");
                assert_eq!(arguments, "");
            } else {
                assert!(value["error"].is_object());
                assert!(value.get("choices").is_none());
                assert!(value.get("output").is_none());
            }
        }
    }
}

#[tokio::test]
async fn websocket_message_too_big_remains_413_before_delivery_for_both_formats() {
    let error_body = json!({"error":{
        "message":"upstream websocket message too big",
        "code":"message_too_big",
        "type":"invalid_request_error"
    }});
    for native in [false, true] {
        for streaming in [false, true] {
            let error = EngineError::Provider(
                ProviderError::new(
                    ProviderErrorKind::InvalidRequest,
                    UpstreamSendState::Ambiguous,
                )
                .with_status(413)
                .with_client_visible_upstream_response(ClientVisibleUpstreamResponse::new(
                    413,
                    Some(b"application/json".to_vec()),
                    Bytes::from(error_body.to_string()),
                ))
                .with_client_visible_upstream_error(
                    ClientVisibleUpstreamError::new(
                        "upstream websocket message too big",
                        Some("message_too_big".to_owned()),
                        Some("invalid_request_error".to_owned()),
                    ),
                ),
            );
            let mut script = Script::new(Vec::new());
            if streaming {
                script.steps = Some(vec![NextStep::Error(error)]);
            } else {
                script.collect_error = Some(error);
            }
            let execution = scripted(script);
            let response = recovery_request(Arc::clone(&execution), native, streaming).await;
            assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
            assert!(
                response.headers()[CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with("application/json")
            );
            let body = read_text(response).await;
            let value: Value = serde_json::from_str(&body).expect("pre-delivery JSON error");
            assert_eq!(value["error"]["code"], "message_too_big");
            assert_eq!(value["error"]["type"], "invalid_request_error");
            assert_eq!(value["error"]["message"], error_body["error"]["message"]);
            assert!(value.get("choices").is_none());
            assert!(value.get("output").is_none());
            assert!(!body.contains("tool_calls"));
            assert!(!body.contains("response.completed"));
            assert!(!body.contains("[DONE]"));
            assert_eq!(execution.trace.client_statuses(), [413]);
            assert!(!execution.trace.snapshot().contains(&"commit"));
        }
    }
}
