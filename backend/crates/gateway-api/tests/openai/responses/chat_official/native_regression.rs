//! 原生 Responses 交付不受 Chat 分层变化影响。
use super::super::http::NextStep;
use super::http::{Fixture, Session, json_body};
use super::lifecycle::request;
use axum::{
    body::{Body, to_bytes},
    http::StatusCode,
};
use gateway_core::{
    engine::EngineError,
    event::{ProtocolWireEvent, ProviderEvent},
};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use tower::ServiceExt;

fn wire(kind: &str, mut value: Value) -> ProviderEvent {
    value["type"] = json!(kind);
    ProviderEvent::wire(ProtocolWireEvent::json("openai", Some(kind.to_owned()), value).unwrap())
}

pub(super) fn completed(output: Value) -> ProviderEvent {
    wire(
        "response.completed",
        json!({"response":{
            "id":"resp_chat_test","object":"response","model":"upstream-model",
            "status":"completed","output":output,
            "usage":{"input_tokens":11,"output_tokens":7,"total_tokens":18,
                "input_tokens_details":{"cached_tokens":3},
                "output_tokens_details":{"reasoning_tokens":2}}
        }}),
    )
}

pub(super) fn message(text: &str) -> Value {
    json!({"id":"msg_chat_test","type":"message","role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":text,"annotations":[]}]})
}

fn created() -> ProviderEvent {
    wire(
        "response.created",
        json!({"response":{
            "id":"resp_chat_test","object":"response","model":"upstream-model",
            "status":"in_progress","output":[]
        }}),
    )
}

fn delta(text: &str) -> ProviderEvent {
    wire(
        "response.output_text.delta",
        json!({"output_index":0,"content_index":0,"item_id":"msg_chat_test","delta":text}),
    )
}

#[tokio::test]
async fn native_responses_keeps_its_existing_delta_recovery() {
    let fixture = Fixture::new(Session::new(vec![
        created(),
        delta("native recovered"),
        completed(json!([])),
    ]))
    .await;
    let mut request = request(Body::from(
        json!({"model":"model-a","input":"hello","stream":false}).to_string(),
    ));
    *request.uri_mut() = "/v1/responses".parse().unwrap();
    let response = fixture.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["output"][0]["content"][0]["text"],
        "native recovered"
    );
}

#[tokio::test]
async fn same_fixture_keeps_native_responses_extensions_and_one_commit() {
    for stream in [false, true] {
        let output = json!([
            {"type":"custom_tool_call","id":"ct","call_id":"call","name":"code","input":"raw"},
            {"type":"image_generation_call","id":"img","result":"fixture-image"},
            {"type":"web_search_call","id":"ws","action":{"type":"search","query":"fixture"}}
        ]);
        let fixture = Fixture::new(Session::new(vec![created(), completed(output.clone())])).await;
        let mut request = request(Body::from(
            json!({"model":"model-a","input":"hello","stream":stream}).to_string(),
        ));
        *request.uri_mut() = "/v1/responses".parse().unwrap();
        let response = fixture.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        if stream {
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(text.contains("event: response.completed"));
            assert!(text.contains("fixture-image"));
            assert_eq!(text.matches("data: [DONE]").count(), 1);
        } else {
            assert_eq!(json_body(response).await["output"], output);
        }
        assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn native_batch_keeps_partial_output_when_finalization_fails() {
    let mut session = Session::new(vec![created()]).with_batch(vec![
        delta("partial-native"),
        completed(json!([message("partial-native")])),
    ]);
    session.final_step = NextStep::Error(EngineError::Cancelled);
    let fixture = Fixture::new(session).await;
    let mut request = request(Body::from(
        json!({"model":"model-a","input":"hello","stream":true}).to_string(),
    ));
    *request.uri_mut() = "/v1/responses".parse().unwrap();
    let response = fixture.router.clone().oneshot(request).await.unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("partial-native"), "{text}");
    assert!(text.contains("event: response.failed"), "{text}");
    assert!(!text.contains("event: response.completed"), "{text}");
    assert_eq!(text.matches("data: [DONE]").count(), 1);
    assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 1);
}
