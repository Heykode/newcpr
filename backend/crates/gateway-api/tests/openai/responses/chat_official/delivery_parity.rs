//! Endpoint delivery contracts from the pinned official Chat implementation.

use std::{sync::atomic::Ordering, time::Duration};

use axum::{
    body::{Body, Bytes, to_bytes},
    http::{Request, StatusCode},
};
use futures::StreamExt;
use gateway_core::{
    engine::{CommitRequirement, EngineError},
    error::{
        ClientVisibleUpstreamResponse, GatewayError, GatewayErrorKind, ProviderError,
        ProviderErrorKind,
    },
    event::ProviderResponseHeader,
    upstream::UpstreamSendState,
};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::super::http::{NextStep, delivery_provider};
use super::{
    chat_request, completed, created,
    http::{Execution, Fixture, KEY, Session, json_body},
    message,
    response::sse_body,
    wire,
};

fn upstream_failure(kind: ProviderErrorKind, status: u16, body: Bytes) -> EngineError {
    EngineError::Provider(
        ProviderError::new(kind, UpstreamSendState::Sent).with_client_visible_upstream_response(
            ClientVisibleUpstreamResponse::new(
                status,
                Some(b"application/problem+json; charset=utf-8".to_vec()),
                body,
            )
            .with_headers(vec![
                ProviderResponseHeader::new("retry-after", Bytes::from_static(b"7")),
                ProviderResponseHeader::new("x-request-id", Bytes::from_static(b"req-failure")),
                ProviderResponseHeader::new("x-future-error", Bytes::from_static(b"first")),
                ProviderResponseHeader::new("x-future-error", Bytes::from_static(b"second")),
            ]),
        ),
    )
}

fn capacity_event(kind: &str, code: &str) -> gateway_core::event::ProviderEvent {
    let error = json!({"type":"server_error","code":code,"message":"synthetic capacity failure"});
    wire(
        kind,
        if kind == "response.failed" {
            json!({"response":{"id":"resp_chat_test","status":"failed","error":error}})
        } else {
            json!({"error":error})
        },
    )
}

// Pinned official buffered delivery collects the terminal before projecting Chat.
// A failed wire event is not the same boundary as an upstream HTTP failure.
#[tokio::test]
async fn buffered_wire_failure_uses_official_collection_error() {
    for kind in ["error", "response.failed"] {
        for code in ["slow_down", "server_is_overloaded", "rate_limit_exceeded"] {
            let fixture = Fixture::new(Session::new(vec![capacity_event(kind, code)])).await;
            let response = fixture.post(chat_request(false)).await;
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(response.headers()["content-type"], "application/json");
            assert!(!response.headers().contains_key("retry-after"));
            assert_eq!(
                json_body(response).await,
                json!({"error":{
                    "type":"server_error",
                    "code":"invalid_upstream_response",
                    "message":"The gateway could not forward the upstream response."
                }})
            );
            assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.trace.calls.client_statuses(), vec![500]);
            assert_eq!(
                fixture
                    .trace
                    .calls
                    .snapshot()
                    .iter()
                    .filter(|call| **call == "delivery_failed")
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn buffered_terminal_is_not_overridden_by_a_separate_capacity_scan() {
    for kind in ["error", "response.failed"] {
        for code in ["slow_down", "server_is_overloaded"] {
            let fixture = Fixture::new(Session::new(vec![
                capacity_event(kind, code),
                completed(json!([message("completed result")])),
            ]))
            .await;
            let response = fixture.post(chat_request(false)).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["content-type"], "application/json");
            assert!(!response.headers().contains_key("retry-after"));
            let body = json_body(response).await;
            assert_eq!(body["choices"][0]["message"]["content"], "completed result");
            assert_eq!(body["choices"][0]["finish_reason"], "stop");
            assert_eq!(body["usage"]["total_tokens"], 18);
            assert!(body.get("error").is_none());
            assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn streamed_capacity_failure_keeps_official_error_projection() {
    for kind in ["error", "response.failed"] {
        for code in ["slow_down", "server_is_overloaded"] {
            let fixture = Fixture::new(Session::new(vec![capacity_event(kind, code)])).await;
            let response = fixture.post(chat_request(true)).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["content-type"], "text/event-stream");
            let (text, chunks) = sse_body(response).await;
            assert_eq!(
                chunks,
                vec![json!({"error":{
                    "type":"server_error", "code":"server_error", "message":"synthetic capacity failure"
                }})]
            );
            assert_eq!(text.matches("data: [DONE]").count(), 1);
            assert!(!text.contains("response.failed"));
            assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn missing_stream_terminal_uses_official_delivery_error() {
    let fixture = Fixture::new(Session::new(vec![created()])).await;
    let (_, chunks) = sse_body(fixture.post(chat_request(true)).await).await;
    let errors: Vec<_> = chunks
        .iter()
        .filter(|chunk| chunk.get("error").is_some())
        .collect();
    assert_eq!(
        errors,
        vec![&json!({"error":{
            "type":"server_error", "code":"invalid_upstream_response",
            "message":"The gateway could not deliver the Chat response."
        }})]
    );
    assert!(!chunks.iter().any(
        |chunk| chunk["usage"].is_object() || chunk["choices"][0]["finish_reason"].is_string()
    ));
    assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn conversion_failure_uses_official_projection_error_before_and_after_commit() {
    let expected = json!({"error":{
        "type":"server_error", "code":"invalid_upstream_response",
        "message":"The gateway could not project the upstream response to Chat Completions."
    }});
    for mode in 0..3 {
        let mut events = Vec::new();
        if mode == 2 {
            events.push(created());
        }
        events.push(completed(
            json!([{"type":"function_call","arguments":"{}"}]),
        ));
        let fixture = Fixture::new(Session::new(events)).await;
        let response = fixture.post(chat_request(mode != 0)).await;
        if mode == 2 {
            let (_, chunks) = sse_body(response).await;
            let errors: Vec<_> = chunks
                .iter()
                .filter(|chunk| chunk.get("error").is_some())
                .collect();
            assert_eq!(errors, vec![&expected]);
            assert!(!chunks.iter().any(|chunk| chunk["usage"].is_object()
                || chunk["choices"][0]["finish_reason"].is_string()));
        } else {
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            assert_eq!(response.headers()["content-type"], "application/json");
            assert_eq!(json_body(response).await, expected);
        }
        assert_eq!(
            fixture.trace.commits.load(Ordering::SeqCst),
            usize::from(mode == 2)
        );
        assert_eq!(fixture.trace.calls.delivery_errors().len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn first_event_wait_keeps_chat_and_native_responses_alive() {
    for chat in [true, false] {
        let mut session = Session::new(Vec::new());
        session.steps = Some(vec![NextStep::DelayedEvent(
            Duration::from_secs(240),
            delivery_provider(
                completed(json!([message("delayed success")])),
                CommitRequirement::CommitBeforeDelivery,
            ),
        )]);
        let fixture = Fixture::new(session).await;
        let request = Request::post(if chat {
            "/v1/chat/completions"
        } else {
            "/v1/responses"
        })
        .header("authorization", format!("Bearer {KEY}"))
        .header("content-type", "application/json")
        .body(Body::from(if chat {
            chat_request(true).to_string()
        } else {
            json!({"model":"model-a","input":"hello","stream":true}).to_string()
        }))
        .unwrap();
        let start = tokio::time::Instant::now();
        let response = tokio::time::timeout(
            Duration::from_secs(20),
            fixture.router.clone().oneshot(request),
        )
        .await
        .expect("headers must arrive before the delayed first event")
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let mut body = response.into_body().into_data_stream();
        for heartbeat in 1..16 {
            let bytes = tokio::time::timeout(Duration::from_secs(20), body.next())
                .await
                .expect("first-event waiting must remain alive")
                .unwrap()
                .unwrap();
            assert_eq!(bytes, Bytes::from_static(b": keep-alive\n\n"));
            assert_eq!(start.elapsed(), Duration::from_secs(15 * heartbeat));
            assert_eq!(fixture.trace.calls.snapshot(), vec!["wait_event"]);
            assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 0);
        }
        let mut remainder = Vec::new();
        while let Some(bytes) = body.next().await {
            remainder.extend_from_slice(&bytes.unwrap());
        }
        let text = String::from_utf8(remainder).unwrap();
        assert!(text.contains("delayed success"), "{text}");
        assert_eq!(text.matches("data: [DONE]").count(), 1, "{text}");
        assert!(!text.contains("response.failed"), "{text}");
        assert_eq!(start.elapsed(), Duration::from_secs(240));
        assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 1);
        assert!(!fixture.trace.calls.is_cancelled());
        assert_eq!(
            fixture.trace.calls.snapshot(),
            vec!["wait_event", "next_event", "commit", "next_end"]
        );
    }
}

#[tokio::test(start_paused = true)]
async fn error_after_admission_keepalive_uses_chat_error_encoding() {
    let mut session = Session::new(Vec::new());
    session.steps = Some(vec![NextStep::DelayedError(
        Duration::from_secs(31),
        EngineError::Deadline,
    )]);
    let fixture = Fixture::new(session).await;
    let response = fixture.post(chat_request(true)).await;
    let (body, chunks) = sse_body(response).await;
    assert!(body.starts_with(": keep-alive\n\n"));
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0]["error"]["code"], "request_timeout");
    assert!(!body.contains("response.failed"));
    assert_eq!(body.matches("data: [DONE]").count(), 1);
    assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.trace.calls.client_statuses(), vec![200]);
    assert_eq!(
        fixture.trace.calls.snapshot(),
        vec!["wait_error", "next_error"]
    );
}

#[tokio::test]
async fn precommit_http_failures_preserve_original_bytes_and_headers() {
    for stream in [false, true] {
        for raw in [
            Bytes::from(
                json!({"error":{"message":"synthetic failure","code":"rate_limit_exceeded",
                "recovery":{"action":"retry","future":[1,2]}},"top_level":17})
                .to_string(),
            ),
            Bytes::from_static(b"not-json\xffstill-upstream"),
        ] {
            let error = upstream_failure(ProviderErrorKind::RateLimited, 429, raw.clone());
            let mut session = Session::new(Vec::new());
            if stream {
                session.steps = Some(vec![NextStep::Error(error)]);
            } else {
                session.collect_error = Some(error);
            }
            session.headers = vec![
                ProviderResponseHeader::new("x-request-id", Bytes::from_static(b"req-opening")),
                ProviderResponseHeader::new(
                    "x-codex-turn-state",
                    Bytes::from_static(b"fixture-state"),
                ),
            ];
            let fixture = Fixture::new(session).await;
            let response = fixture.post(chat_request(stream)).await;
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(
                response.headers()["content-type"],
                "application/problem+json; charset=utf-8"
            );
            assert_eq!(response.headers()["retry-after"], "7");
            assert_eq!(response.headers()["x-request-id"], "req-failure");
            assert_eq!(response.headers()["x-codex-turn-state"], "fixture-state");
            assert_eq!(
                response.headers().get_all("x-future-error").iter().count(),
                2
            );
            assert_eq!(
                to_bytes(response.into_body(), usize::MAX).await.unwrap(),
                raw
            );
            assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.trace.calls.client_statuses(), vec![429]);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn capacity_projection_keeps_extensions_before_and_after_keepalive() {
    for code in ["slow_down", "server_is_overloaded"] {
        for mode in 0..3 {
            let mut expected = json!({"error":{"type":"server_error","code":code,
                "message":"synthetic capacity error","recovery":{"action":"retry","future":[1,2]}},
                "top_level":17,"status":429});
            let error = upstream_failure(
                ProviderErrorKind::UpstreamCapacityUnavailable,
                429,
                Bytes::from(expected.to_string()),
            );
            expected["error"]["code"] = json!("server_error");
            expected["status"] = json!(503);
            let mut session = Session::new(Vec::new());
            match mode {
                0 => session.collect_error = Some(error),
                1 => session.steps = Some(vec![NextStep::Error(error)]),
                _ => {
                    session.steps =
                        Some(vec![NextStep::DelayedError(Duration::from_secs(31), error)])
                }
            }
            let fixture = Fixture::new(session).await;
            let response = fixture.post(chat_request(mode != 0)).await;
            if mode == 2 {
                let (text, chunks) = sse_body(response).await;
                assert!(text.starts_with(": keep-alive\n\n"));
                assert_eq!(chunks.len(), 1);
                expected["error"]["headers"] = json!({"retry-after":"7"});
                assert_eq!(chunks[0]["error"], expected["error"]);
                assert_eq!(text.matches("data: [DONE]").count(), 1);
                assert!(!text.contains("response.failed"));
                assert_eq!(fixture.trace.calls.client_statuses(), vec![200]);
            } else {
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(response.headers()["retry-after"], "7");
                assert_eq!(json_body(response).await, expected);
                assert_eq!(fixture.trace.calls.client_statuses(), vec![503]);
            }
            assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn late_http_failure_keeps_recovery_fields_and_plaintext_fallback() {
    for raw in [
        Bytes::from(
            json!({"error":{"type":"invalid_request_error","code":"fixture_policy",
            "message":"synthetic policy failure","recovery":{"action":"retry"},
            "headers":{"retry-after":"11"},"extension":{"opaque":true}}})
            .to_string(),
        ),
        Bytes::from_static(b"synthetic plain upstream failure"),
    ] {
        let original = serde_json::from_slice::<Value>(&raw).ok();
        let error = upstream_failure(ProviderErrorKind::InvalidRequest, 400, raw);
        let mut session = Session::new(Vec::new());
        session.steps = Some(vec![NextStep::DelayedError(Duration::from_secs(31), error)]);
        let fixture = Fixture::new(session).await;
        let (text, chunks) = sse_body(fixture.post(chat_request(true)).await).await;
        assert!(text.starts_with(": keep-alive\n\n"));
        assert_eq!(chunks.len(), 1);
        match original {
            Some(original) => assert_eq!(chunks[0]["error"], original["error"]),
            None => {
                assert_eq!(
                    chunks[0]["error"]["message"],
                    "synthetic plain upstream failure"
                );
                assert_eq!(chunks[0]["error"]["headers"]["retry-after"], "7");
            }
        }
        assert!(!text.contains("response.failed"));
        assert_eq!(text.matches("data: [DONE]").count(), 1);
        assert_eq!(fixture.trace.calls.client_statuses(), vec![200]);
    }
}

#[tokio::test(start_paused = true)]
async fn dropping_initial_keepalive_cancels_the_original_execution_once() {
    let mut session = Session::new(Vec::new());
    session.final_step = NextStep::FinalizeCancelled;
    session.steps = Some(vec![NextStep::DelayedEvent(
        Duration::from_secs(240),
        delivery_provider(
            completed(json!([])),
            CommitRequirement::CommitBeforeDelivery,
        ),
    )]);
    let fixture = Fixture::new(session).await;
    let response = tokio::time::timeout(Duration::from_secs(20), fixture.post(chat_request(true)))
        .await
        .expect("initial heartbeat");
    let mut body = response.into_body().into_data_stream();
    assert_eq!(
        body.next().await.unwrap().unwrap(),
        Bytes::from_static(b": keep-alive\n\n")
    );
    drop(body);
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(fixture.trace.calls.is_cancelled());
    assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture.trace.calls.snapshot(),
        vec!["wait_event", "cancel_finalize"]
    );
}

#[tokio::test(start_paused = true)]
async fn admission_failure_never_becomes_a_successful_chat_stream() {
    let mut execution = Execution::new(Session::new(Vec::new()));
    execution.start_delay = Some(Duration::from_secs(31));
    execution.start_error = Some(GatewayError::new(
        GatewayErrorKind::PolicyDenied,
        "synthetic admission denial",
    ));
    let fixture = Fixture::with_execution(execution).await;
    let request = chat_request(true);
    let pending = fixture.post(request);
    tokio::pin!(pending);
    assert!(
        tokio::time::timeout(Duration::from_secs(20), &mut pending)
            .await
            .is_err()
    );
    let response = pending.await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(json_body(response).await["error"]["code"], "policy_denied");
    assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 0);
    assert!(fixture.trace.calls.snapshot().is_empty());
}
