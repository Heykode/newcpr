use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::AUTHORIZATION},
};
use bytes::Bytes;
use futures::future::BoxFuture;
use gateway_core::engine::ModelRequestId;
use gateway_core::engine::execution::{
    AuthenticatedClient, ClientAuthenticationError, ClientTransport, ExecutionRequestMetadata,
    ExecutionService, StartExecution, StartProviderExecution, StartedExecution,
};
use gateway_core::error::{
    ClientVisibleUpstreamResponse, GatewayError, GatewayErrorKind, ProviderError, ProviderErrorKind,
};
use gateway_core::event::ProviderResponseHeader;
use gateway_core::operation::Operation;
use gateway_core::routing::PublicModelId;
use gateway_core::upstream::UpstreamSendState;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::endpoint::BufferedJsonSession;
use super::{api_router, authenticated_client};

const COMPACT_RESPONSE: &[u8] = br#"{ "id":"cmp_test", "object":"response.compaction", "output":[{"type":"compaction","encrypted_content":"opaque","future":9007199254740993}],"usage":{"input_tokens":10,"output_tokens":2},"future":1.234567890123456789 }"#;
const COMPACT_FAILURE: &[u8] =
    br#"{ "error":{"message":"compact unavailable","code":"compact_unavailable"}, "future":9007199254740993 }"#;

#[derive(Debug, Clone)]
struct CapturedCompact {
    public_model: String,
    client_key: String,
    metadata: ExecutionRequestMetadata,
    body: Bytes,
    context: Value,
}

#[derive(Clone, Copy)]
enum Outcome {
    Success,
    UpstreamFailure,
    ModelDenied,
}

struct CompactExecution {
    client: AuthenticatedClient,
    captured: Mutex<Vec<CapturedCompact>>,
    committed_statuses: Arc<Mutex<Vec<u16>>>,
    outcome: Outcome,
}

impl CompactExecution {
    fn new(outcome: Outcome) -> Arc<Self> {
        Arc::new(Self {
            client: authenticated_client("sk_compact_test"),
            captured: Mutex::new(Vec::new()),
            committed_statuses: Arc::new(Mutex::new(Vec::new())),
            outcome,
        })
    }

    fn captured(&self) -> Vec<CapturedCompact> {
        self.captured.lock().expect("capture lock").clone()
    }
}

impl ExecutionService for CompactExecution {
    fn authenticate(
        &self,
        plaintext: &str,
    ) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        (plaintext == "sk_compact_test")
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
        Box::pin(async move {
            let Operation::Compact(compact) = request.operation else {
                panic!("compact route must use Operation::Compact");
            };
            let payload = compact.payload();
            assert_eq!(payload.protocol(), "openai");
            self.captured
                .lock()
                .expect("capture lock")
                .push(CapturedCompact {
                    public_model: request.public_model.as_str().to_owned(),
                    client_key: request.client.policy().key_id().as_str().to_owned(),
                    metadata: request.metadata,
                    body: payload.body().clone(),
                    context: Value::Object(payload.context().clone()),
                });
            if matches!(self.outcome, Outcome::ModelDenied) {
                return Err(GatewayError::new(
                    GatewayErrorKind::ModelNotFound,
                    "requested model is not available to this API key",
                ));
            }
            let session = if matches!(self.outcome, Outcome::UpstreamFailure) {
                BufferedJsonSession::failure(
                    ProviderError::new(ProviderErrorKind::InvalidRequest, UpstreamSendState::Sent)
                        .with_status(StatusCode::UNPROCESSABLE_ENTITY.as_u16())
                        .with_client_visible_upstream_response(
                            ClientVisibleUpstreamResponse::new(
                                StatusCode::UNPROCESSABLE_ENTITY.as_u16(),
                                Some(b"application/problem+json".to_vec()),
                                Bytes::from_static(COMPACT_FAILURE),
                            )
                            .with_headers(vec![
                                ProviderResponseHeader::new(
                                    "x-oai-request-id",
                                    Bytes::from_static(b"req-compact-failure"),
                                ),
                                ProviderResponseHeader::new(
                                    "x-compact-error",
                                    Bytes::from_static(b"preserved"),
                                ),
                            ]),
                        ),
                )
            } else {
                BufferedJsonSession::success(
                    Bytes::from_static(COMPACT_RESPONSE),
                    StatusCode::CREATED.as_u16(),
                    vec![ProviderResponseHeader::new(
                        "x-codex-turn-state",
                        Bytes::from_static(b"opaque-turn"),
                    )],
                    Arc::clone(&self.committed_statuses),
                )
            };
            Ok(StartedExecution {
                request_id: ModelRequestId::new("req_compact_test").expect("request ID"),
                created_at: SystemTime::now(),
                stream: false,
                session: Box::new(session),
            })
        })
    }

    fn start_provider_endpoint(
        &self,
        _: StartProviderExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        panic!("compact must never use model-less provider endpoint execution");
    }
}

#[tokio::test]
async fn compact_route_captures_model_context_metadata_and_commits_raw_response_once() {
    let execution = CompactExecution::new(Outcome::Success);
    let raw = br#"{ "model":"model-a","input":[{"role":"user","content":"history"}],"tools":[],"parallel_tool_calls":false,"future":9007199254740993 }"#;
    let turn_metadata = r#"{"session_id":"root","thread_id":"child","future":true}"#;
    let response = api_router(execution.clone())
        .await
        .oneshot(
            Request::post("/v1/responses/compact")
                .header(AUTHORIZATION, "Bearer sk_compact_test")
                .header("content-type", "application/json")
                .header("user-agent", "compact-integration-test")
                .header("x-real-ip", "198.51.100.17")
                .header("session-id", "root")
                .header("thread-id", "child")
                .header("x-codex-turn-metadata", turn_metadata)
                .body(Body::from(raw.to_vec()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(response.headers()["x-codex-turn-state"], "opaque-turn");
    assert_eq!(response.headers()["x-request-id"], "req_compact_test");
    assert_eq!(
        response.headers()["x-gateway-request-id"],
        "req_compact_test"
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body")
            .as_ref(),
        COMPACT_RESPONSE,
    );
    let captured = execution.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].public_model, "model-a");
    assert_eq!(captured[0].client_key, "key_api_test");
    assert_eq!(captured[0].body.as_ref(), raw);
    assert_eq!(
        captured[0].context,
        json!({
            "session_id":"root","thread_id":"child","turn_metadata":turn_metadata,
        })
    );
    assert_eq!(captured[0].metadata.protocol, "openai");
    assert_eq!(captured[0].metadata.endpoint, "/v1/responses/compact");
    assert_eq!(captured[0].metadata.transport, ClientTransport::HttpJson);
    assert!(!captured[0].metadata.stream);
    assert!(captured[0].metadata.previous_response_id.is_none());
    assert_eq!(
        captured[0].metadata.client_ip,
        Some("198.51.100.17".parse().expect("IP"))
    );
    assert_eq!(
        captured[0].metadata.user_agent.as_deref(),
        Some("compact-integration-test")
    );
    assert_eq!(
        *execution.committed_statuses.lock().expect("commits"),
        vec![201]
    );
}

#[tokio::test]
async fn compact_authentication_and_invalid_requests_never_start_execution() {
    let execution = CompactExecution::new(Outcome::Success);
    let router = api_router(execution.clone()).await;
    for authorization in [None, Some("Bearer sk_wrong_key")] {
        let mut request = Request::post("/v1/responses/compact");
        if let Some(authorization) = authorization {
            request = request.header(AUTHORIZATION, authorization);
        }
        let response = router
            .clone()
            .oneshot(
                request
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"model":"model-a","input":[]}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    for body in [
        "not-json",
        "[]",
        r#"{"input":[]}"#,
        r#"{"model":"","input":[]}"#,
        r#"{"model":"model-a","input":{}}"#,
        r#"{"model":"model-a","input":[],"stream":true}"#,
        r#"{"model":"model-a","input":[],"stream":false}"#,
        r#"{"model":"model-a","input":[],"use_websocket":true}"#,
        r#"{"model":"model-a","input":[],"previous_response_id":"resp_other_owner"}"#,
        r#"{"model":"model-a","input":[],"previous_response_id":""}"#,
        r#"{"model":"model-a","input":[],"previous_response_id":17}"#,
        r#"{"model":"model-a","input":[],"instructions":17}"#,
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::post("/v1/responses/compact")
                    .header(AUTHORIZATION, "Bearer sk_compact_test")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
    }
    assert!(execution.captured().is_empty());
    assert!(
        execution
            .committed_statuses
            .lock()
            .expect("commits")
            .is_empty()
    );
}

#[tokio::test]
async fn compact_gzip_decoding_preserves_raw_json_bytes() {
    let execution = CompactExecution::new(Outcome::Success);
    let raw = br#"{ "model":"model-a", "input":[], "future":9007199254740993 }"#;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(raw).expect("compress request");
    let compressed = encoder.finish().expect("finish compression");
    let response = api_router(execution.clone())
        .await
        .oneshot(
            Request::post("/v1/responses/compact")
                .header(AUTHORIZATION, "Bearer sk_compact_test")
                .header("content-type", "application/json")
                .header("content-encoding", "gzip")
                .body(Body::from(compressed))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let captured = execution.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].body.as_ref(), raw);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body")
            .as_ref(),
        COMPACT_RESPONSE,
    );
    assert_eq!(
        *execution.committed_statuses.lock().expect("commits"),
        vec![201],
    );
}

#[tokio::test]
async fn compact_rejects_corrupted_gzip_without_starting_execution() {
    let execution = CompactExecution::new(Outcome::Success);
    let response = api_router(execution.clone())
        .await
        .oneshot(
            Request::post("/v1/responses/compact")
                .header(AUTHORIZATION, "Bearer sk_compact_test")
                .header("content-type", "application/json")
                .header("content-encoding", "gzip")
                .body(Body::from(vec![0x1f, 0x8b, 0x08, 0x00]))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(execution.captured().is_empty());
    assert!(
        execution
            .committed_statuses
            .lock()
            .expect("commits")
            .is_empty()
    );
}

#[tokio::test]
async fn compact_model_denial_from_core_is_not_bypassed_or_committed() {
    let execution = CompactExecution::new(Outcome::ModelDenied);
    let response = api_router(execution.clone())
        .await
        .oneshot(
            Request::post("/v1/responses/compact")
                .header(AUTHORIZATION, "Bearer sk_compact_test")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"model-denied","input":[]}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("JSON");
    assert_eq!(body["error"]["code"], "model_not_found");
    assert_eq!(
        execution.captured().len(),
        1,
        "Core owns model permission checks"
    );
    assert!(
        execution
            .committed_statuses
            .lock()
            .expect("commits")
            .is_empty()
    );
}

#[tokio::test]
async fn compact_returns_exact_upstream_failure_without_success_commit() {
    let execution = CompactExecution::new(Outcome::UpstreamFailure);
    let response = api_router(execution.clone())
        .await
        .oneshot(
            Request::post("/v1/responses/compact")
                .header(AUTHORIZATION, "Bearer sk_compact_test")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"model-a","input":[]}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    assert_eq!(response.headers()["x-compact-error"], "preserved");
    assert_eq!(response.headers()["x-request-id"], "req-compact-failure");
    assert_eq!(
        response.headers()["x-oai-request-id"],
        "req-compact-failure"
    );
    assert_eq!(
        response.headers()["x-gateway-request-id"],
        "req_compact_test"
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body")
            .as_ref(),
        COMPACT_FAILURE,
    );
    assert_eq!(execution.captured().len(), 1);
    assert!(
        execution
            .committed_statuses
            .lock()
            .expect("commits")
            .is_empty()
    );
}
