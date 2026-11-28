//! 核对 GenAI 原生路由、归属边界、原始字节及晚到用量的交付

mod contract;
mod models;
use std::sync::{Arc, Mutex, atomic::Ordering};

use axum::{
    body::{Body, Bytes, to_bytes},
    http::{Request, StatusCode},
};
use futures::future::BoxFuture;
use gateway_core::{
    engine::execution::{
        AuthenticatedClient, ClientAuthenticationError, ExecutionService, StartExecution,
        StartProviderExecution, StartedExecution,
    },
    error::GatewayError,
    event::{GatewayEvent, ProtocolWireEvent, ProviderEvent, ResponseMeta},
    operation::Operation,
    routing::{ProviderCatalogUnavailable, PublicModelDescriptor, PublicModelId},
};
use gateway_protocol::genai::GENAI_PROTOCOL;
use serde_json::{Value, json};
use tower::ServiceExt as _;

use super::session::{Finish, Session};
use super::{api_router, authenticated_client};

const KEY: &str = "sk_genai_test";
const SHORT: &str = "/v1beta/models/model-a";
const BODY: &str = r#"{ "contents":[{"role":"user","parts":[{"text":"Read record 42"}]}],"tools":[{"functionDeclarations":[{"name":"read","parameters":{"type":"OBJECT"}}]}],"future":{"a":1} }"#;

#[derive(Default)]
struct Execution {
    client: Option<AuthenticatedClient>,
    catalog: Option<Result<Vec<PublicModelDescriptor>, ProviderCatalogUnavailable>>,
    session: Mutex<Option<Session>>,
    seen: Mutex<Option<StartExecution>>,
}

impl ExecutionService for Execution {
    fn authenticate(&self, key: &str) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        if key == KEY {
            Ok(self
                .client
                .clone()
                .unwrap_or_else(|| authenticated_client(key)))
        } else {
            Err(ClientAuthenticationError::InvalidKey)
        }
    }
    fn public_models(&self, _: &AuthenticatedClient) -> Vec<PublicModelId> {
        vec![]
    }
    fn client_model_catalog<'a>(
        &'a self,
        client: &'a AuthenticatedClient,
        protocol: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Vec<PublicModelDescriptor>, ProviderCatalogUnavailable>> {
        use gateway_core::routing::{ModelPresentation, PublicModelProfile};
        assert_eq!(protocol, GENAI_PROTOCOL);
        Box::pin(async {
            self.catalog.clone().unwrap_or_else(|| {
                Ok(client
                    .snapshot()
                    .public_models_for_scope(client.policy().account_scope())
                    .into_iter()
                    .map(|id| {
                        PublicModelDescriptor::Adapted(PublicModelProfile::new(
                            id,
                            ModelPresentation::default(),
                        ))
                    })
                    .collect())
            })
        })
    }
    fn contains_public_model(&self, _: &AuthenticatedClient, _: &PublicModelId) -> bool {
        true
    }
    fn start(
        &self,
        request: StartExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move {
            assert!(match &request.operation {
                Operation::Generate(generate) => generate.requires_native_protocol(),
                Operation::CountTokens(count) => count.requires_native_protocol(),
                _ => false,
            });
            let stream = request.metadata.stream;
            *self.seen.lock().unwrap() = Some(request);
            Ok(StartedExecution {
                request_id: gateway_core::engine::ModelRequestId::new("req_genai").unwrap(),
                created_at: std::time::SystemTime::now(),
                stream,
                session: Box::new(
                    self.session
                        .lock()
                        .unwrap()
                        .take()
                        .expect("expected inference"),
                ),
            })
        })
    }
    fn start_provider_endpoint(
        &self,
        _: StartProviderExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        panic!("GenAI requires model authorization");
    }
}

fn request(url: &str) -> Request<Body> {
    Request::post(url)
        .header("authorization", format!("Bearer {KEY}"))
        .header("user-agent", "fixture-client/1.0")
        .header("x-goog-api-client", "fixture-transport/1.0")
        .header("x-goog-user-project", "untrusted")
        .body(Body::from(BODY))
        .unwrap()
}

fn wire_frame(frame: &'static str) -> ProviderEvent {
    let value = serde_json::from_str(frame.strip_prefix("data: ").unwrap().trim()).unwrap();
    ProviderEvent::wire(
        ProtocolWireEvent::json_with_raw_sse_metadata(
            GENAI_PROTOCOL,
            None,
            value,
            Bytes::from_static(frame.as_bytes()),
            None,
            None,
        )
        .unwrap(),
    )
}
