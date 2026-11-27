//! NewCPR execution ownership around the official Chat conversion.

use std::{io::Write, sync::atomic::Ordering, time::Duration};

use axum::{
    body::{Body, Bytes, to_bytes},
    http::{Request, StatusCode},
};
use gateway_core::{
    engine::{CommitRequirement, CoordinatedEvent, EngineError},
    event::{ProtocolWireEvent, ProviderEvent, ProviderResponseHeader},
};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::super::http::{NextStep, delivery_provider};
use super::{
    chat_request, completed, created,
    delivery::sse_body,
    http::{Execution, Fixture, KEY, Session, json_body},
};
use crate::openai::authenticated_client_with_response_limit;

const PATH: &str = "/v1/chat/completions";

pub(super) fn request(body: Body) -> Request<Body> {
    Request::post(PATH)
        .header("authorization", format!("Bearer {KEY}"))
        .header("content-type", "application/json")
        .body(body)
        .unwrap()
}

fn delta(text: &str) -> ProviderEvent {
    super::chunk(json!({"content":text}), Value::Null)
}

fn assert_failed(chunks: &[Value]) {
    assert_eq!(
        chunks
            .iter()
            .filter(|chunk| chunk.get("error").is_some())
            .count(),
        1,
        "{chunks:?}"
    );
    assert!(
        !chunks
            .iter()
            .any(|chunk| chunk["choices"][0]["finish_reason"].is_string()),
        "{chunks:?}"
    );
    assert!(
        !chunks.iter().any(|chunk| chunk["usage"].is_object()),
        "{chunks:?}"
    );
}

#[tokio::test]
async fn rejects_missing_authentication_before_decoding_or_execution() {
    let fixture = Fixture::new(Session::new(Vec::new())).await;
    for authorization in [None, Some("Bearer sk_invalid")] {
        let mut request = Request::post(PATH).header("content-encoding", "gzip");
        if let Some(authorization) = authorization {
            request = request.header("authorization", authorization);
        }
        let response = fixture
            .router
            .clone()
            .oneshot(request.body(Body::from("broken gzip")).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert!(fixture.execution.captured.lock().unwrap().is_none());
}

#[tokio::test]
async fn chat_enters_execution_with_original_chat_body() {
    for stream in [false, true] {
        let fixture = Fixture::new(Session::new(vec![created(), completed("ok")])).await;
        let mut body = chat_request(stream);
        body["use_websocket"] = json!(false);
        body["reasoning_effort"] = json!("low");
        let response = fixture.post(body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let _ = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let captured = fixture.captured();
        assert_eq!(captured.endpoint, PATH);
        assert_eq!(
            captured.transport,
            if stream { "HttpSse" } else { "HttpJson" }
        );
        assert_eq!(captured.body["messages"][0]["role"], "user");
        assert_eq!(captured.body["reasoning_effort"], "low");
        assert!(captured.body.get("store").is_none());
        assert!(captured.body.get("input").is_none());
        assert_eq!(captured.body["use_websocket"], false);
        assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn gzip_and_headers_reuse_the_responses_ingress_contract() {
    let mut session = Session::new(vec![created(), completed("ok")]);
    session.headers = vec![
        ProviderResponseHeader::new("x-future-multi", Bytes::from_static(b"first")),
        ProviderResponseHeader::new("x-future-multi", Bytes::from_static(b"second")),
        ProviderResponseHeader::new("connection", Bytes::from_static(b"x-hop-only")),
        ProviderResponseHeader::new("x-hop-only", Bytes::from_static(b"removed")),
    ];
    let fixture = Fixture::new(session).await;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder
        .write_all(chat_request(false).to_string().as_bytes())
        .unwrap();
    let compressed = encoder.finish().unwrap();
    let request = Request::post(PATH)
        .header("authorization", format!("Bearer {KEY}"))
        .header("content-type", "application/json")
        .header("content-encoding", "gzip")
        .header("content-length", compressed.len())
        .header("session_id", "chat-session")
        .header("x-future-input", "first")
        .header("x-future-input", "second")
        .header("x-real-ip", "203.0.113.42")
        .header("user-agent", "chat-fixture")
        .body(Body::from(compressed))
        .unwrap();
    let response = fixture.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get_all("x-future-multi")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(!response.headers().contains_key("x-hop-only"));
    assert!(!response.headers().contains_key("content-encoding"));
    assert_eq!(
        json_body(response).await["choices"][0]["message"]["content"],
        "ok"
    );
    let captured = fixture.captured();
    assert_eq!(captured.client_ip.unwrap().to_string(), "203.0.113.42");
    assert_eq!(captured.user_agent.as_deref(), Some("chat-fixture"));
    assert_eq!(captured.context["session_id"], "chat-session");
    let headers = captured.context["opaque_request_headers"]
        .as_array()
        .unwrap();
    assert_eq!(
        headers
            .iter()
            .filter(|header| header[0] == "x-future-input")
            .count(),
        2
    );
    assert!(!headers.iter().any(|header| matches!(
        header[0].as_str(),
        Some("content-encoding" | "content-length" | "authorization")
    )));
}

#[tokio::test]
async fn corrupt_or_oversized_compressed_chat_never_starts_execution() {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder
        .write_all(chat_request(false).to_string().as_bytes())
        .unwrap();
    for bytes in [b"broken gzip".to_vec(), encoder.finish().unwrap()] {
        let mut execution = Execution::new(Session::new(Vec::new()));
        execution.client = authenticated_client_with_response_limit(KEY, "openai", 16);
        let fixture = Fixture::with_execution(execution).await;
        let mut request = request(Body::from(bytes));
        request
            .headers_mut()
            .insert("content-encoding", "gzip".parse().unwrap());
        let response = fixture.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(fixture.execution.captured.lock().unwrap().is_none());
    }
}

#[tokio::test]
async fn optional_nulls_follow_official_defaults() {
    let fixture = Fixture::new(Session::new(vec![created(), completed("ok")])).await;
    let mut body = chat_request(false);
    for field in [
        "stream",
        "stream_options",
        "tools",
        "tool_choice",
        "response_format",
        "reasoning_effort",
        "max_completion_tokens",
        "temperature",
    ] {
        body[field] = Value::Null;
    }
    assert_eq!(fixture.post(body).await.status(), StatusCode::OK);
    let body = fixture.captured().body;
    assert!(body["stream"].is_null());
    assert!(body["tools"].is_null());
    assert!(body["reasoning_effort"].is_null());
}

#[tokio::test]
async fn custom_and_builtin_chat_tools_reach_the_existing_execution_path() {
    for tool in [
        json!({"type":"custom","custom":{"name":"code"}}),
        json!({"type":"web_search"}),
        json!({"type":"image_generation"}),
    ] {
        let fixture = Fixture::new(Session::new(vec![created(), completed("ok")])).await;
        let mut body = chat_request(false);
        body["tools"] = json!([tool.clone()]);
        let response = fixture.post(body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let _ = json_body(response).await;
        let expected = tool;
        assert_eq!(fixture.captured().body["tools"], json!([expected]));
        assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn commit_failure_never_delivers_buffered_or_streamed_success() {
    for streaming in [false, true] {
        let mut session = Session::new(vec![created(), completed("must not leak")]);
        session.fail_commit = true;
        let fixture = Fixture::new(session).await;
        let response = fixture.post(chat_request(streaming)).await;
        assert!(response.status().is_server_error());
        let body = json_body(response).await;
        assert!(body.get("choices").is_none());
        assert!(!body.to_string().contains("must not leak"));
        assert_eq!(fixture.trace.calls.delivery_errors().len(), 1);
        assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn finalization_failure_never_leaks_success_finish_or_usage() {
    for already_started in [false, true] {
        let mut session = if already_started {
            Session::new(vec![created(), completed("finished")])
        } else {
            Session::new(Vec::new()).with_batch(vec![created(), completed("finished")])
        };
        session.final_step = NextStep::Error(EngineError::Cancelled);
        let fixture = Fixture::new(session).await;
        let mut body = chat_request(true);
        body["stream_options"] = json!({"include_usage":true});
        let (_, chunks) = sse_body(fixture.post(body).await).await;
        assert_failed(&chunks);
        assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.trace.calls.client_statuses(), [200]);
    }
}

#[tokio::test]
async fn business_events_after_terminal_reject_the_whole_terminal_batch() {
    for first_batch in [false, true] {
        let events = if first_batch {
            Vec::new()
        } else {
            vec![created()]
        };
        let mut batch = vec![completed("must not leak"), delta("late")];
        if first_batch {
            batch.insert(0, created());
        }
        let fixture = Fixture::new(Session::new(events).with_batch(batch)).await;
        let response = fixture.post(chat_request(true)).await;
        if first_batch {
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            assert!(json_body(response).await.get("choices").is_none());
        } else {
            let (text, chunks) = sse_body(response).await;
            assert_failed(&chunks);
            assert!(!text.contains("must not leak"));
        }
        assert_eq!(fixture.trace.calls.delivery_errors().len(), 1);
    }
}

#[tokio::test]
async fn malformed_trailing_raw_sse_is_not_accepted_after_success() {
    let raw = ProviderEvent::wire(
        ProtocolWireEvent::raw_sse("chat_completions", Bytes::from_static(b"data: {broken"))
            .unwrap(),
    );
    let fixture = Fixture::new(Session::new(vec![created(), completed("ok"), raw])).await;
    let (_, chunks) = sse_body(fixture.post(chat_request(true)).await).await;
    assert_failed(&chunks);
}

#[tokio::test]
async fn wire_failure_then_engine_failure_emits_exactly_one_error() {
    let error = ProviderEvent::wire(ProtocolWireEvent::json("chat_completions", None,
        json!({"error":{"type":"rate_limit_error","code":"rate_limit_exceeded","message":"retry later"}})).unwrap());
    let mut session = Session::new(vec![created(), error]);
    session.final_step = NextStep::Error(EngineError::Cancelled);
    let fixture = Fixture::new(session).await;
    let (_, chunks) = sse_body(fixture.post(chat_request(true)).await).await;
    assert_failed(&chunks);
    assert!(
        chunks
            .iter()
            .any(|chunk| chunk["error"]["code"] == "rate_limit_exceeded")
    );
}

#[tokio::test]
async fn dropping_chat_body_cancels_and_finalizes_the_original_execution() {
    let mut session = Session::new(vec![created()]);
    session.final_step = NextStep::FinalizeCancelled;
    let fixture = Fixture::new(session).await;
    let response = fixture.post(chat_request(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(fixture.trace.calls.is_cancelled());
    assert_eq!(
        fixture
            .trace
            .calls
            .snapshot()
            .iter()
            .filter(|event| **event == "cancel_finalize")
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn keepalive_does_not_cancel_a_pending_chat_event() {
    let mut session = Session::new(Vec::new());
    session.steps = Some(vec![
        NextStep::Event(delivery_provider(
            created(),
            CommitRequirement::CommitBeforeDelivery,
        )),
        NextStep::DelayedEvent(
            Duration::from_secs(31),
            CoordinatedEvent::try_batch(
                vec![delta("ok"), completed("ok")],
                CommitRequirement::AlreadyCommitted,
            )
            .unwrap(),
        ),
    ]);
    let fixture = Fixture::new(session).await;
    let (text, chunks) = sse_body(fixture.post(chat_request(true)).await).await;
    assert!(text.contains(": keep-alive"));
    assert!(!chunks.iter().any(|chunk| chunk.get("error").is_some()));
    assert_eq!(
        fixture
            .trace
            .calls
            .snapshot()
            .iter()
            .filter(|event| **event == "wait_event")
            .count(),
        1
    );
    assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 1);
}
