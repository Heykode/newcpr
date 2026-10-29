use super::*;
use gateway_core::account::ResponsesUpstream;
use gateway_core::routing::RequestTuning;
use std::sync::atomic::Ordering;

fn operation(model: &str, websocket: bool) -> Operation {
    let payload = ProtocolPayload::json_object(
        "openai",
        Map::from_iter([
            ("model".to_owned(), json!(model)),
            ("input".to_owned(), json!("hello")),
        ]),
    )
    .unwrap()
    .with_context(Map::from_iter([(
        "use_websocket".to_owned(),
        json!(websocket),
    )]));
    Operation::Generate(GenerateRequest::from_protocol_payload(payload))
}

fn provider(
    store: &Arc<MemoryAccountStore>,
    leases: Arc<TestLeaseCoordinator>,
    base_url: String,
) -> CodexProvider {
    super::scheduling::waiting_provider_with_affinity_and_limit(
        store,
        leases,
        base_url,
        Arc::new(MemorySessionExclusions::default()),
        Arc::new(MemorySessionAffinity::default()),
        NonZeroU32::MIN,
    )
}

fn attempt(wait: bool) -> AttemptContext {
    context("req_model_eligibility", CancellationToken::new()).with_request_tuning(RequestTuning {
        account_busy_wait_enabled: wait,
        ..Default::default()
    })
}

#[tokio::test]
async fn grok_never_acquires_native_oauth_in_either_selector_or_transport() {
    for wait in [false, true] {
        for excel_enabled in [false, true] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_scope_old").await;
            if excel_enabled {
                store.set_responses_upstream("acct_scope_old", ResponsesUpstream::Excel);
                store.set_excel_encrypted_content_policy(
                    "acct_scope_old",
                    false,
                    vec!["gpt-5.4".to_owned()],
                );
            }
            let leases = Arc::new(TestLeaseCoordinator::default());
            leases.capacity.enabled.store(true, Ordering::SeqCst);
            let upstream = MockServer::start().await;
            let provider = provider(&store, leases.clone(), upstream.uri());
            for websocket in [false, true] {
                for model in [
                    "grok",
                    "grok-4.7",
                    "xai/grok-4.7",
                    "GROK-NEW",
                    "vendor/XAI/GrOk-new",
                ] {
                    let result = provider
                        .execute(
                            planned_request_with_model(
                                "openai",
                                operation(model, websocket),
                                model,
                            ),
                            attempt(wait),
                        )
                        .await;
                    let error = result.err().expect("native OAuth cannot serve Grok");
                    assert_eq!(error.kind(), ProviderErrorKind::NoEligibleAccount);
                    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
                }
            }
            assert!(leases.requests.lock().unwrap().is_empty());
            assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
            assert!(upstream.received_requests().await.unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn unlisted_native_models_still_reach_upstream_and_complete() {
    for wait in [false, true] {
        for model in ["gpt-future", "custom-unlisted", "grokish-custom"] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_scope_old").await;
            let leases = Arc::new(TestLeaseCoordinator::default());
            leases.capacity.enabled.store(true, Ordering::SeqCst);
            let upstream = MockServer::start().await;
            Mock::given(method("POST")).and(path("/codex/responses"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(
                    format!("event: response.created\ndata: {}\n\n{CAPTURE_COMPLETED_SSE}", json!({"type":"response.created","response":{"id":"resp_scope_capture","model":model,"status":"in_progress"}})),
                    "text/event-stream",
                )).expect(1).mount(&upstream).await;
            let provider = provider(&store, leases, upstream.uri());
            let mut stream = provider
                .execute(
                    planned_request_with_model("openai", operation(model, false), model),
                    attempt(wait),
                )
                .await
                .unwrap();
            let mut completed = false;
            while let Some(event) = stream.next().await {
                completed |= event
                    .unwrap()
                    .wire_event()
                    .is_some_and(|wire| wire.event_type() == Some("response.completed"));
            }
            assert!(completed);
            let requests = upstream.received_requests().await.unwrap();
            assert_eq!(captured_request_body(&requests[0])["model"], model);
            upstream.verify().await;
        }
    }
}

#[tokio::test]
async fn model_guard_preserves_configured_excel_preparation_in_both_selectors() {
    for wait in [false, true] {
        for websocket in [false, true] {
            for model in ["gpt-5.4", "grok-configured"] {
                let store = Arc::new(MemoryAccountStore::default());
                create_account(&store, "acct_scope_old").await;
                store.set_responses_upstream("acct_scope_old", ResponsesUpstream::Excel);
                store.set_excel_encrypted_content_policy(
                    "acct_scope_old",
                    false,
                    vec!["gpt-5.4".to_owned(), "grok-configured".to_owned()],
                );
                let leases = Arc::new(TestLeaseCoordinator::default());
                leases.capacity.enabled.store(true, Ordering::SeqCst);
                let upstream = MockServer::start().await;
                let provider = provider(&store, leases, upstream.uri());
                let stream = provider
                    .execute(
                        planned_request_with_model("openai", operation(model, websocket), model),
                        attempt(wait),
                    )
                    .await
                    .unwrap_or_else(|error| {
                        panic!("model={model}, wait={wait}, websocket={websocket}: {error:?}")
                    });
                assert_eq!(stream.metadata().transport().as_str(), "excel_http_sse");
                assert_eq!(
                    stream.metadata().provider_account_id().as_str(),
                    "acct_scope_old"
                );
                // Preparation is enough to verify route preservation; never poll the real Excel endpoint.
                drop(stream);
                assert!(upstream.received_requests().await.unwrap().is_empty());
            }
        }
    }
}
