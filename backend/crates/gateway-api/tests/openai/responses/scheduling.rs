use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Request, StatusCode, header::AUTHORIZATION},
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt, future::BoxFuture};
use gateway_api::openai::responses::{
    OpenAiRequestHeaders, decode_request_with_headers, decode_response_create_with_context,
};
use gateway_core::{
    engine::execution::{
        AuthenticatedClient, ClientAuthenticationError, ExecutionService, StartExecution,
        StartProviderExecution, StartedExecution,
    },
    error::{GatewayError, GatewayErrorKind},
    operation::Operation,
    routing::PublicModelId,
};
use gateway_protocol::openai::{
    OPENAI_SCHEDULING_SESSION_HINT_CONTEXT_KEY, OPENAI_SCHEDULING_SESSION_HINT_HEADERS,
    OpenAiSchedulingSessionHint, codex_session_id, codex_thread_id,
    is_openai_scheduling_session_hint_header,
};
use serde_json::{Map, Value, json};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tower::ServiceExt;

use super::{openai_protocol_context, openai_wire_body};
use crate::openai::{api_router, authenticated_client};

fn assert_no_forwarded_hint_headers(context: &Map<String, Value>) {
    if let Some(headers) = context
        .get("opaque_request_headers")
        .and_then(Value::as_array)
    {
        for entry in headers {
            let name = entry[0].as_str().expect("opaque header name");
            assert!(!is_openai_scheduling_session_hint_header(name), "{name}");
        }
    }
}

#[test]
fn scheduling_hint_header_priority_is_independent_of_insertion_order() {
    let body = br#"{"model":"model-a","input":"hello"}"#;
    for (index, expected_source) in OPENAI_SCHEDULING_SESSION_HINT_HEADERS.iter().enumerate() {
        let mut headers = HeaderMap::new();
        for source in OPENAI_SCHEDULING_SESSION_HINT_HEADERS[index..].iter().rev() {
            headers.insert(*source, HeaderValue::from_static(" \tsession-1 "));
        }
        let decoded = decode_request_with_headers(body, &headers).expect("decode request");
        let context = openai_protocol_context(&decoded);
        let hint = OpenAiSchedulingSessionHint::from_context(context).expect("transport hint");
        assert_eq!(hint.source(), *expected_source);
        assert_eq!(hint.id(), "session-1");
        assert_eq!(context.len(), 1);
        assert!(!openai_wire_body(&decoded).contains_key("session_id"));
        assert_no_forwarded_hint_headers(context);
    }
}

#[test]
fn scheduling_hint_invalid_and_repeated_headers_fall_through_without_forwarding() {
    let invalid_values = [
        HeaderValue::from_static(""),
        HeaderValue::from_static(" \t "),
        HeaderValue::from_static("contains spaces"),
        HeaderValue::from_static("contains\ttab"),
        HeaderValue::from_bytes(b"\x80\xff").expect("opaque non-ASCII header"),
        HeaderValue::from_str(&"x".repeat(1025)).expect("oversized header"),
    ];
    for invalid in invalid_values {
        let mut headers = HeaderMap::new();
        headers.insert("x-session-affinity", invalid);
        headers.append("x-session-id", HeaderValue::from_static("repeated"));
        headers.append("x-session-id", HeaderValue::from_static("repeated"));
        headers.insert("x-opencode-session", HeaderValue::from_static("fallback"));
        let decoded =
            decode_request_with_headers(br#"{"model":"model-a","input":"hello"}"#, &headers)
                .expect("invalid supplemental headers do not invalidate the request");
        let context = openai_protocol_context(&decoded);
        let hint = OpenAiSchedulingSessionHint::from_context(context).expect("fallback hint");
        assert_eq!(hint.source(), "x-opencode-session");
        assert_eq!(hint.id(), "fallback");
        assert_no_forwarded_hint_headers(context);

        headers.remove("x-opencode-session");
        let decoded =
            decode_request_with_headers(br#"{"model":"model-a","input":"hello"}"#, &headers)
                .expect("no valid hint");
        assert!(openai_protocol_context(&decoded).is_empty());
    }

    let mut headers = HeaderMap::new();
    headers.append("x-session-id", HeaderValue::from_static("first"));
    headers.append("x-session-id", HeaderValue::from_static("different"));
    let decoded = decode_request_with_headers(br#"{"model":"model-a"}"#, &headers)
        .expect("conflicting repeated hint is ignored");
    assert!(openai_protocol_context(&decoded).is_empty());
}

#[test]
fn scheduling_hint_does_not_change_standard_identity_or_body_metadata_priority() {
    let body = json!({
        "model":"model-a","input":"hello","session_id":"body-session","thread_id":"body-thread",
        "prompt_cache_key":"body-cache",
        "client_metadata":{"session_id":"metadata-session","thread_id":"metadata-thread"},
        "context":{"session_id":"context-session","thread_id":"context-thread"}
    });
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-session-id",
        HeaderValue::from_static("supplemental-session"),
    );
    for with_standard_headers in [false, true] {
        if with_standard_headers {
            headers.insert("session-id", HeaderValue::from_static("header-session"));
            headers.insert("thread-id", HeaderValue::from_static("header-thread"));
            headers.insert(
                "conversation-id",
                HeaderValue::from_static("header-conversation"),
            );
        }
        let decoded = decode_request_with_headers(body.to_string().as_bytes(), &headers)
            .expect("decode identities");
        let context = openai_protocol_context(&decoded);
        assert_eq!(Value::Object(openai_wire_body(&decoded).clone()), body);
        assert_eq!(
            codex_session_id(openai_wire_body(&decoded), context).as_deref(),
            Some(if with_standard_headers {
                "header-session"
            } else {
                "body-session"
            })
        );
        assert_eq!(
            codex_thread_id(openai_wire_body(&decoded), context).as_deref(),
            Some(if with_standard_headers {
                "header-thread"
            } else {
                "body-thread"
            })
        );
        assert_eq!(
            OpenAiSchedulingSessionHint::from_context(context)
                .expect("hint")
                .id(),
            "supplemental-session"
        );
        assert_no_forwarded_hint_headers(context);
    }
}

#[test]
fn scheduling_hint_cannot_be_invented_by_http_body_or_websocket_frame_context() {
    let forged = json!({"source":"x-session-id","id":"body-controlled"});
    let body = json!({
        "model":"model-a","input":"hello",
        "openai_scheduling_session_hint":forged,
        "context":{"openai_scheduling_session_hint":forged},
        "client_metadata":{"openai_scheduling_session_hint":forged},
        "protocol_context":{"openai_scheduling_session_hint":forged}
    });
    for transport_hint in [None, Some("transport-controlled")] {
        let mut headers = HeaderMap::new();
        if let Some(id) = transport_hint {
            headers.insert("x-session-id", HeaderValue::from_static(id));
        }
        let request_headers = OpenAiRequestHeaders::from_headers(&headers);
        let http = decode_request_with_headers(body.to_string().as_bytes(), &headers)
            .expect("HTTP request");
        let mut frame = body.clone();
        frame["type"] = json!("response.create");
        let websocket = decode_response_create_with_context(&frame.to_string(), &request_headers)
            .expect("WebSocket frame");
        for decoded in [http, websocket] {
            let context = openai_protocol_context(&decoded);
            assert_eq!(
                OpenAiSchedulingSessionHint::from_context(context)
                    .as_ref()
                    .map(OpenAiSchedulingSessionHint::id),
                transport_hint
            );
            assert_eq!(Value::Object(openai_wire_body(&decoded).clone()), body);
            assert_no_forwarded_hint_headers(context);
        }
    }
}

#[derive(Clone)]
struct CapturedRequest {
    endpoint: String,
    body: Value,
    raw_body: Option<Bytes>,
    context: Map<String, Value>,
}

struct HintCaptureExecution {
    client: AuthenticatedClient,
    captured: Mutex<Vec<CapturedRequest>>,
}

impl HintCaptureExecution {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            client: authenticated_client("sk_hint_test"),
            captured: Mutex::new(Vec::new()),
        })
    }

    fn capture(&self, operation: &Operation, endpoint: String) -> GatewayError {
        let (body, raw_body, context) = match operation {
            Operation::Generate(request) => {
                let payload = request.protocol_payload();
                (
                    Value::Object(payload.body().clone()),
                    None,
                    payload.context().clone(),
                )
            }
            _ => {
                let payload = match operation {
                    Operation::Search(request) => request.payload(),
                    Operation::GenerateImage(request) => request.payload(),
                    Operation::Compact(request) => request.payload(),
                    _ => panic!("unexpected scheduling ingress operation"),
                };
                (
                    serde_json::from_slice(payload.body()).expect("test JSON body"),
                    Some(payload.body().clone()),
                    payload.context().clone(),
                )
            }
        };
        self.captured
            .lock()
            .expect("capture lock")
            .push(CapturedRequest {
                endpoint,
                body,
                raw_body,
                context,
            });
        GatewayError::new(
            GatewayErrorKind::Internal,
            "scheduling ingress capture completed",
        )
    }
}

impl ExecutionService for HintCaptureExecution {
    fn authenticate(
        &self,
        plaintext: &str,
    ) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        (plaintext == "sk_hint_test")
            .then(|| self.client.clone())
            .ok_or(ClientAuthenticationError::InvalidKey)
    }

    fn public_models(&self, _: &AuthenticatedClient) -> Vec<PublicModelId> {
        vec![PublicModelId::new("model-a").expect("model")]
    }

    fn contains_public_model(&self, _: &AuthenticatedClient, model: &PublicModelId) -> bool {
        model.as_str() == "model-a"
    }

    fn start(
        &self,
        request: StartExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move { Err(self.capture(&request.operation, request.metadata.endpoint)) })
    }

    fn start_provider_endpoint(
        &self,
        request: StartProviderExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move { Err(self.capture(&request.operation, request.metadata.endpoint)) })
    }
}

#[tokio::test]
async fn scheduling_hint_reaches_responses_and_chat_execution_without_becoming_wire_identity() {
    let execution = HintCaptureExecution::new();
    let app = api_router(execution.clone()).await;
    for endpoint in ["/v1/responses", "/v1/chat/completions"] {
        for standard_identity in [false, true] {
            let mut body = if endpoint == "/v1/responses" {
                json!({"model":"model-a","input":"hello","stream":false})
            } else {
                json!({"model":"model-a","messages":[{"role":"user","content":"hello"}]})
            };
            let mut request = Request::post(endpoint)
                .header(AUTHORIZATION, "Bearer sk_hint_test")
                .header("content-type", "application/json");
            for source in OPENAI_SCHEDULING_SESSION_HINT_HEADERS {
                request = request.header(source, "local-hint");
            }
            if standard_identity {
                request = request.header("session-id", "standard-session");
                body["prompt_cache_key"] = json!("standard-cache");
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body.to_string())).expect("request"))
                .await
                .expect("route request");
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let captured = execution.captured.lock().expect("capture lock");
            let captured = captured.last().expect("execution received the request");
            assert_eq!(captured.endpoint, endpoint);
            let hint = OpenAiSchedulingSessionHint::from_context(&captured.context)
                .expect("trusted hint context");
            assert_eq!(hint.source(), "x-session-affinity");
            assert_eq!(hint.id(), "local-hint");
            assert_eq!(
                captured.context.get("session_id"),
                standard_identity
                    .then(|| json!("standard-session"))
                    .as_ref()
            );
            assert_eq!(
                captured.body.get("prompt_cache_key"),
                standard_identity.then(|| json!("standard-cache")).as_ref()
            );
            assert!(
                !captured
                    .body
                    .as_object()
                    .expect("wire body")
                    .contains_key("session_id")
            );
            assert!(
                !captured
                    .body
                    .as_object()
                    .expect("wire body")
                    .contains_key(OPENAI_SCHEDULING_SESSION_HINT_CONTEXT_KEY)
            );
            assert_no_forwarded_hint_headers(&captured.context);
        }
    }
    assert_eq!(execution.captured.lock().expect("capture lock").len(), 4);
}

#[tokio::test]
async fn scheduling_hint_reaches_standalone_context_without_changing_raw_bodies() {
    let execution = HintCaptureExecution::new();
    let app = api_router(execution.clone()).await;
    let raw = br#"{ "model":"model-a", "input":"hello", "id":"search-body-id", "prompt":"image prompt", "context":{"openai_scheduling_session_hint":{"source":"x-session-id","id":"forged"}} }"#;
    for endpoint in [
        "/v1/alpha/search",
        "/v1/images/generations",
        "/v1/images/edits",
        "/v1/responses/compact",
    ] {
        for transport_hint in [false, true] {
            let mut request = Request::post(endpoint)
                .header(AUTHORIZATION, "Bearer sk_hint_test")
                .header("content-type", "application/json")
                .header("session-id", "standard-session")
                .header("thread-id", "child-thread")
                .header(
                    "x-codex-turn-metadata",
                    r#"{"session_id":"metadata-session"}"#,
                );
            if transport_hint {
                request = request.header("x-session-id", "transport-hint");
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::from(raw.to_vec())).expect("request"))
                .await
                .expect("route standalone request");
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let captured = execution.captured.lock().expect("capture lock");
            let captured = captured.last().expect("standalone request was executed");
            assert_eq!(captured.endpoint, endpoint);
            assert_eq!(captured.raw_body.as_deref(), Some(raw.as_slice()));
            assert_eq!(captured.context["session_id"], "standard-session");
            assert_eq!(captured.context["thread_id"], "child-thread");
            assert_eq!(
                captured.context["turn_metadata"],
                r#"{"session_id":"metadata-session"}"#
            );
            assert_eq!(
                OpenAiSchedulingSessionHint::from_context(&captured.context)
                    .as_ref()
                    .map(OpenAiSchedulingSessionHint::id),
                transport_hint.then_some("transport-hint")
            );
            assert!(!captured.context.contains_key("opaque_request_headers"));
        }
    }
    assert_eq!(execution.captured.lock().expect("capture lock").len(), 8);
}

#[tokio::test]
async fn scheduling_hint_from_websocket_opening_is_reused_without_trusting_frame_overrides() {
    let execution = HintCaptureExecution::new();
    let app = api_router(execution.clone()).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("listen address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });
    let mut request = format!("ws://{address}/v1/responses")
        .into_client_request()
        .expect("WebSocket request");
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer sk_hint_test"),
    );
    request
        .headers_mut()
        .insert("x-session-id", HeaderValue::from_static("opening-hint"));
    let (mut socket, response) = connect_async(request).await.expect("upgrade WebSocket");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    for input in ["first", "second"] {
        socket.send(Message::Text(json!({
            "type":"response.create","model":"model-a","input":input,
            "context":{"openai_scheduling_session_hint":{"source":"x-session-id","id":input}}
        }).to_string().into())).await.expect("send frame");
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .expect("response timeout")
                .expect("socket remains open")
                .expect("valid frame");
            if let Message::Text(text) = message {
                let event: Value = serde_json::from_str(&text).expect("JSON event");
                if event["type"] == "error" {
                    break;
                }
            }
        }
    }
    {
        let captured = execution.captured.lock().expect("capture lock");
        assert_eq!(captured.len(), 2);
        let connection_id = captured[0].context["downstream_websocket_connection_id"].clone();
        assert!(
            connection_id
                .as_str()
                .expect("connection ID")
                .starts_with("ws_")
        );
        for (captured, input) in captured.iter().zip(["first", "second"]) {
            let hint = OpenAiSchedulingSessionHint::from_context(&captured.context).expect("hint");
            assert_eq!(hint.source(), "x-session-id");
            assert_eq!(hint.id(), "opening-hint");
            assert_eq!(captured.body["input"], input);
            assert_eq!(
                captured.body["context"][OPENAI_SCHEDULING_SESSION_HINT_CONTEXT_KEY]["id"],
                input
            );
            assert_eq!(
                captured.context["downstream_websocket_connection_id"],
                connection_id
            );
            assert!(!captured.context.contains_key("session_id"));
            assert!(
                !captured
                    .body
                    .as_object()
                    .expect("wire body")
                    .contains_key("session_id")
            );
            assert_no_forwarded_hint_headers(&captured.context);
        }
    }
    socket.close(None).await.expect("close WebSocket");
    server.abort();
}
