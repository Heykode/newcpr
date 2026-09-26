use super::*;
use gateway_core::diagnostics::request_capture::{RequestCaptureFactory, RequestCaptureObserver};

#[derive(Default)]
struct Evidence {
    bodies: Mutex<Vec<(&'static str, Vec<u8>)>>,
    facts: Mutex<Vec<(&'static str, Value)>>,
}
impl RequestCaptureObserver for Evidence {
    fn body(&self, stage: &'static str, _: u32, _: Option<u64>, bytes: &[u8]) {
        self.bodies.lock().unwrap().push((stage, bytes.to_vec()));
    }
    fn fact(&self, stage: &'static str, _: u32, data: &Value) {
        self.facts.lock().unwrap().push((stage, data.clone()));
    }
}
#[derive(Default)]
struct Captures(Mutex<Vec<Arc<Evidence>>>);
impl RequestCaptureFactory for Captures {
    fn start(&self, _: &str, _: &str, _: &[&str]) -> Option<Arc<dyn RequestCaptureObserver>> {
        let evidence = Arc::new(Evidence::default());
        self.0.lock().unwrap().push(evidence.clone());
        Some(evidence)
    }
}

struct Admissions;
impl ClientAdmissionPort for Admissions {
    fn admit(
        &self,
        _: ClientAdmissionRequest,
    ) -> BoxFuture<'_, Result<ClientAdmissionDecision, ClientAdmissionError>> {
        Box::pin(async { Ok(ClientAdmissionDecision::Granted) })
    }
    fn release<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a ModelRequestId,
    ) -> BoxFuture<'a, Result<bool, ClientAdmissionError>> {
        Box::pin(async { Ok(true) })
    }
    fn restore(
        &self,
        _: ClientAdmissionRecovery,
    ) -> BoxFuture<'_, Result<ClientAdmissionRestoreResult, ClientAdmissionError>> {
        Box::pin(async { unreachable!("capture fixture has no recovery") })
    }
}

struct CaptureProvider(AtomicUsize);
#[async_trait]
impl Provider for CaptureProvider {
    fn name(&self) -> &'static str {
        "openai"
    }
    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        Default::default()
    }
    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        Ok(Vec::new())
    }
    async fn execute(
        &self,
        request: ProviderRequest,
        context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        let round = self.0.fetch_add(1, Ordering::SeqCst);
        let trace = context.trace();
        trace.record(
            "account.selection",
            json!({"selectedAccountId":"acct_api_test"}),
        );
        trace.capture(
            "upstream.request.body",
            &serde_json::to_vec(&json!({"round":round})).unwrap(),
        );
        if round % 2 == 1 {
            trace.capture(
                "upstream.error.body",
                br#"{"error":{"message":"fixture rejection"}}"#,
            );
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                UpstreamSendState::Sent,
            )
            .with_status(422));
        }
        let response = format!("resp_capture_{round}");
        let meta = ResponseMeta::new(&response, "model-a");
        let events = [
            ProviderEvent::canonical_with_wire(
                vec![GatewayEvent::Started(meta.clone())],
                ProtocolWireEvent::json("openai", Some("response.created".into()),
                    json!({"type":"response.created","response":{"id":response,"model":"model-a","status":"in_progress"}})).unwrap(),
            ),
            ProviderEvent::canonical_with_wire(
                vec![GatewayEvent::Completed(meta)],
                ProtocolWireEvent::json("openai", Some("response.completed".into()),
                    json!({"type":"response.completed","response":{"id":response,"model":"model-a","status":"completed","output":[]}})).unwrap(),
            ),
        ];
        Ok(ProviderStream::new(
            ProviderCallMetadata::new(
                request.candidate().provider().clone(),
                request.candidate().upstream_model().unwrap().clone(),
                ProviderAccountId::new("acct_api_test").unwrap(),
                UpstreamTransport::new("http_sse").unwrap(),
            ),
            futures::stream::iter(events.map(Ok)),
            (),
        ))
    }
}

async fn app(captures: Arc<Captures>, first_round: usize) -> axum::Router {
    let store = Arc::new(SettlementPorts::default());
    api_router(Arc::new(
        DefaultExecutionService::new(
            RuntimeSnapshotHandle::new(snapshot("sk_capture_fixture", "openai")),
            store.clone(),
            ProviderRegistry::new([
                Arc::new(CaptureProvider(AtomicUsize::new(first_round))) as Arc<dyn Provider>
            ])
            .unwrap(),
            Arc::new(Admissions),
            store,
            Arc::new(UnusedContinuation),
            Arc::new(IgnoredClientApiKeyUsage),
        )
        .with_captures(captures),
    ))
    .await
}

#[tokio::test]
async fn capture_real_core_http_error_keeps_body_status_and_finalization() {
    let captures = Arc::new(Captures::default());
    let response = app(captures.clone(), 1)
        .await
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(AUTHORIZATION, "Bearer sk_capture_fixture")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"model":"model-a","input":"capture input","stream":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_client_error());
    let status = response.status().as_u16();
    let downstream = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let requests = captures.0.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let bodies = requests[0].bodies.lock().unwrap();
    assert!(
        bodies
            .iter()
            .any(|(stage, _)| *stage == "client.request.body")
    );
    assert!(
        bodies
            .iter()
            .any(|(stage, body)| *stage == "downstream.body" && body == &downstream)
    );
    let facts = requests[0].facts.lock().unwrap();
    assert!(
        facts
            .iter()
            .any(|(stage, value)| *stage == "downstream.status" && value["status"] == status)
    );
    assert!(facts.iter().any(|(stage, _)| *stage == "request.finished"));
}

#[tokio::test]
async fn capture_real_core_websocket_rounds_keep_separate_bodies_and_completion_facts() {
    let captures = Arc::new(Captures::default());
    let router = app(captures.clone(), 0).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut request = format!("ws://{address}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(AUTHORIZATION, "Bearer sk_capture_fixture".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    for round in 0..3 {
        socket
            .send(ClientMessage::Text(
                json!({"type":"response.create","model":"model-a",
            "input":format!("capture round {round}")})
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let ClientMessage::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            if matches!(value["type"].as_str(), Some("response.completed" | "error")) {
                assert_eq!(
                    value["type"],
                    if round == 1 {
                        "error"
                    } else {
                        "response.completed"
                    }
                );
                break;
            }
        }
    }
    socket.close(None).await.unwrap();
    let requests = captures.0.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for (round, request) in requests.iter().enumerate() {
        let bodies = request.bodies.lock().unwrap();
        let input: Vec<_> = bodies
            .iter()
            .filter(|(stage, _)| *stage == "client.request.body")
            .collect();
        assert_eq!(input.len(), 1);
        assert_eq!(
            serde_json::from_slice::<Value>(&input[0].1).unwrap()["input"],
            format!("capture round {round}")
        );
        let downstream: Vec<_> = bodies
            .iter()
            .filter(|(stage, _)| *stage == "downstream.event")
            .map(|(_, bytes)| serde_json::from_slice::<Value>(bytes).unwrap())
            .collect();
        assert!(downstream.iter().any(|value| value["type"]
            == if round == 1 {
                "error"
            } else {
                "response.completed"
            }));
        assert_eq!(
            request
                .facts
                .lock()
                .unwrap()
                .iter()
                .any(|(stage, _)| *stage == "upstream.completed"),
            round != 1
        );
    }
    server.abort();
}
