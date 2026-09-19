use gateway_core::operation::CompactRequest;

use super::*;

const COMPACT_RESPONSE: &[u8] = br#"{ "id":"cmp_contract","object":"response.compaction","output":[{"type":"message","role":"user","content":[{"type":"input_text","text":"history"}]},{"type":"compaction","encrypted_content":"opaque-state","future":9007199254740993}],"usage":{"input_tokens":139,"output_tokens":438,"total_tokens":577,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":0},"output_tokens_details":{"reasoning_tokens":64}},"future":1.2345678901234567890123456789 }"#;

fn compact_operation(body: Bytes, context: Map<String, Value>) -> Operation {
    Operation::Compact(CompactRequest::from_raw_json(
        RawJsonPayload::new("openai", body)
            .expect("compact payload")
            .with_context(context),
    ))
}

#[tokio::test]
async fn compact_uses_model_aware_http_json_and_preserves_output_and_usage() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let server = MockServer::start().await;
    let request_body = br#"{ "model":"gpt-5.4","input":[{"type":"message","role":"user","content":"history"}],"instructions":"retain context","tools":[],"parallel_tool_calls":false,"reasoning":{"effort":"high"},"service_tier":"default","prompt_cache_key":"compact-cache","text":{"verbosity":"low"} }"#;
    Mock::given(method("POST"))
        .and(path("/backend-api/codex/responses/compact"))
        .and(header("authorization", "Bearer at-acct_provider_contract"))
        .and(header(
            "chatgpt-account-id",
            "chatgpt-acct_provider_contract",
        ))
        .and(header("content-type", "application/json"))
        .and(body_bytes(request_body.as_slice()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(COMPACT_RESPONSE, "application/json")
                .insert_header("x-request-id", "compact-request")
                .insert_header("x-codex-turn-state", "opaque-turn")
                .insert_header("connection", "close"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider_with_base_url(&store, format!("{}/backend-api", server.uri()));
    let mut stream = provider
        .execute(
            planned_request(
                "openai",
                compact_operation(Bytes::from_static(request_body), Map::new()),
            ),
            context("req_compact_wire", CancellationToken::new()),
        )
        .await
        .expect("compact selection");
    assert_eq!(stream.metadata().transport().as_str(), "http_json");
    assert_eq!(
        stream
            .metadata()
            .upstream_model()
            .map(UpstreamModelId::as_str),
        Some("gpt-5.4")
    );
    let mut facts = Vec::new();
    let mut raw = None;
    while let Some(event) = stream.next().await {
        let (canonical, wire) = event.expect("compact event").into_parts();
        facts.extend(canonical);
        if let Some(body) = wire.and_then(|wire| wire.into_raw_json_body()) {
            assert!(raw.replace(body).is_none());
        }
    }
    assert_eq!(raw.as_deref(), Some(COMPACT_RESPONSE));
    assert!(
        facts
            .iter()
            .any(|fact| matches!(fact, GatewayEvent::Usage(usage)
        if usage.input_tokens == Some(139) && usage.output_tokens == Some(438)
        && usage.total_tokens == Some(577) && usage.reasoning_tokens == Some(64)))
    );
    assert!(
        facts
            .iter()
            .any(|fact| matches!(fact, GatewayEvent::CalculatedCost(_)))
    );
    assert!(
        facts
            .iter()
            .any(|fact| matches!(fact, GatewayEvent::Completed(_)))
    );
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(
        requests.len(),
        1,
        "no automatic response.create or extra compaction"
    );
    assert!(requests[0].headers.contains_key("user-agent"));
    assert!(!requests[0].headers.contains_key("upgrade"));
}

#[tokio::test]
async fn compact_missing_or_partial_usage_is_not_filled_with_zero() {
    for usage in [None, Some(json!({"input_tokens":12}))] {
        let store = Arc::new(MemoryAccountStore::default());
        // A fixture must not inherit pooled HTTP clients from a dropped runtime.
        let account_id = format!("acct_compact_usage_{}", uuid::Uuid::new_v4());
        create_account(&store, &account_id).await;
        let server = MockServer::start().await;
        let mut response = json!({"output":[{"type":"compaction","encrypted_content":"opaque"}]});
        if let Some(usage) = &usage {
            response["usage"] = usage.clone();
        }
        Mock::given(method("POST"))
            .and(path("/codex/responses/compact"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .expect(1)
            .mount(&server)
            .await;
        let provider = provider_with_base_url(&store, server.uri());
        let mut stream = provider
            .execute(
                planned_request(
                    "openai",
                    compact_operation(
                        Bytes::from_static(br#"{"model":"gpt-5.4","input":[]}"#),
                        Map::new(),
                    ),
                ),
                context("req_compact_usage", CancellationToken::new()),
            )
            .await
            .expect("compact");
        let mut observed_usage = None;
        while let Some(event) = stream.next().await {
            for fact in event.expect("event").canonical_facts() {
                match fact {
                    GatewayEvent::Usage(usage) => observed_usage = Some(usage.clone()),
                    GatewayEvent::CalculatedCost(_) => panic!("unknown usage must not be priced"),
                    _ => {}
                }
            }
        }
        match usage {
            Some(_) => assert_eq!(
                observed_usage,
                Some(Usage {
                    input_tokens: Some(12),
                    ..Usage::new()
                })
            ),
            None => assert!(observed_usage.is_none()),
        }
    }
}

#[tokio::test]
async fn compact_rejects_previous_response_and_unscoped_routes_before_selection() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let leases = Arc::new(TestLeaseCoordinator::default());
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        Arc::new(MemorySessionAffinity::default()),
        server.uri(),
        Arc::clone(&leases),
    );
    for previous in [json!("resp_other_client"), json!(""), json!(false)] {
        let body = json!({"model":"gpt-5.4","input":[],"previous_response_id":previous});
        let error = provider
            .execute(
                planned_request(
                    "openai",
                    compact_operation(
                        Bytes::from(serde_json::to_vec(&body).expect("JSON")),
                        Map::new(),
                    ),
                ),
                context("req_compact_previous", CancellationToken::new()),
            )
            .await
            .err()
            .expect("cannot guess previous response owner");
        assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
        assert_eq!(error.send_state(), UpstreamSendState::NotSent);
    }
    let error = provider
        .execute(
            planned_provider_endpoint_request(
                "openai",
                compact_operation(
                    Bytes::from_static(br#"{"model":"gpt-5.4","input":[]}"#),
                    Map::new(),
                ),
            ),
            context("req_compact_unscoped", CancellationToken::new()),
        )
        .await
        .err()
        .expect("model-less endpoint must not bypass routing");
    assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
    assert!(leases.requests.lock().expect("leases").is_empty());
    assert!(
        server
            .received_requests()
            .await
            .expect("requests")
            .is_empty()
    );
}

#[tokio::test]
async fn compact_respects_account_scope_and_client_session_affinity() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_outside_scope").await;
    let server = MockServer::start().await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        Arc::clone(&affinity),
        server.uri(),
        Arc::clone(&leases),
    );
    let operation = compact_operation(
        Bytes::from_static(br#"{"model":"gpt-5.4","input":[]}"#),
        Map::from_iter([("session_id".to_owned(), json!("compact-session"))]),
    );
    assert!(
        provider
            .execute(
                planned_request("openai", operation.clone()),
                context("req_compact_scope", CancellationToken::new()),
            )
            .await
            .is_err()
    );
    assert!(leases.requests.lock().expect("leases").is_empty());

    create_account(&store, "acct_affinity_switch_a").await;
    create_account(&store, "acct_affinity_switch_b").await;
    let key = ClientApiKeyId::new("key_openai_contract").expect("key");
    let observation = provider.request_observation(&operation, &key);
    assert!(observation.compact);
    assert_eq!(
        observation.continuation.affinity_hash,
        provider
            .request_observation(
                &Operation::Generate(generate_with_session_context("compact-session", None, None)),
                &key,
            )
            .continuation
            .affinity_hash
    );
    assert_ne!(
        observation.continuation.affinity_hash,
        provider
            .request_observation(
                &operation,
                &ClientApiKeyId::new("other_client").expect("key"),
            )
            .continuation
            .affinity_hash,
    );
    affinity.seed_binding(
        &ProviderKind::new("openai").expect("provider"),
        observation
            .continuation
            .affinity_hash
            .as_deref()
            .expect("affinity"),
        ProviderAccountId::new("acct_affinity_switch_b").expect("account"),
    );
    let stream = provider
        .execute(
            planned_request("openai", operation),
            context("req_compact_affinity", CancellationToken::new()),
        )
        .await
        .expect("compact selection");
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_affinity_switch_b"
    );
    drop(stream);
    assert!(
        server
            .received_requests()
            .await
            .expect("requests")
            .is_empty()
    );
}

#[tokio::test]
async fn compact_propagates_upstream_rejection_and_rejects_false_success() {
    for (status, response) in [
        (400, br#"{"error":{"type":"invalid_request_error","message":"compact unavailable","code":"compact_unavailable"}}"#.as_slice()),
        (200, br#"{"output":[]}"#.as_slice()),
        (200, br#"{"output":[{"type":"compaction","encrypted_content":""}]}"#.as_slice()),
        (200, b"not JSON".as_slice()),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        let account_id = format!("acct_compact_error_{}", uuid::Uuid::new_v4());
        create_account(&store, &account_id).await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/codex/responses/compact"))
            .respond_with(ResponseTemplate::new(status).set_body_raw(response, "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        let provider = provider_with_base_url(&store, server.uri());
        let mut stream = provider.execute(
            planned_request("openai", compact_operation(
                Bytes::from_static(br#"{"model":"gpt-5.4","input":[]}"#),
                Map::new(),
            )),
            context("req_compact_error", CancellationToken::new()),
        ).await.expect("compact selection");
        let error = loop {
            match stream.next().await {
                Some(Err(error)) => break error,
                Some(Ok(event)) => assert!(!event.canonical_facts().iter().any(|fact|
                    matches!(fact, GatewayEvent::Completed(_))
                )),
                None => panic!("compact must not report success"),
            }
        };
        if status == 400 {
            let raw = error.client_visible_upstream_response().expect("raw error");
            assert_eq!(raw.status(), status);
            assert_eq!(raw.body().as_ref(), response);
        } else {
            assert_eq!(error.kind(), ProviderErrorKind::Protocol);
            assert_eq!(error.send_state(), UpstreamSendState::Sent);
        }
    }
}

#[tokio::test]
async fn compact_publishes_capability_and_forwards_models_missing_from_discovery() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "models":[{"slug":"gpt-5.3-codex","display_name":"Codex"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/codex/responses/compact"))
        .and(header("authorization", "Bearer at-acct_provider_contract"))
        .and(body_bytes(br#"{"model":"gpt-5.4","input":[]}"#.as_slice()))
        .respond_with(ResponseTemplate::new(200).set_body_raw(COMPACT_RESPONSE, "application/json"))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider_with_base_url(&store, server.uri());
    let models = provider.query_model_capabilities().await.expect("catalog");
    assert_eq!(models.len(), 1);
    assert!(
        models[0]
            .capabilities()
            .match_requirements(&CapabilityRequirements::new(OperationKind::Compact))
            .is_some()
    );
    assert!(!provider.model_catalog_is_exhaustive());
    let mut stream = provider
        .execute(
            planned_request(
                "openai",
                compact_operation(
                    Bytes::from_static(br#"{"model":"gpt-5.4","input":[]}"#),
                    Map::new(),
                ),
            ),
            context("req_compact_catalog", CancellationToken::new()),
        )
        .await
        .expect("discovery does not reject an unlisted compact model");
    let mut completed = false;
    let mut raw = None;
    while let Some(event) = stream.next().await {
        let (facts, wire) = event.expect("compact result").into_parts();
        completed |= facts
            .iter()
            .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
        if let Some(body) = wire.and_then(|wire| wire.into_raw_json_body()) {
            raw = Some(body);
        }
    }
    assert!(completed);
    assert_eq!(raw.as_deref(), Some(COMPACT_RESPONSE));
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 2, "one discovery and one compact request");
}

#[tokio::test]
async fn compact_result_can_be_replayed_without_automatic_compaction() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses/compact"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(COMPACT_RESPONSE, "application/json"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("connection", "close")
                .set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider_with_base_url(&store, server.uri());
    let mut stream = provider
        .execute(
            planned_request(
                "openai",
                compact_operation(
                    Bytes::from_static(
                        br#"{"model":"public-alias","input":[],"future":9007199254740993}"#,
                    ),
                    Map::new(),
                ),
            ),
            context("req_compact_replay", CancellationToken::new()),
        )
        .await
        .expect("compact");
    let mut output = None;
    while let Some(event) = stream.next().await {
        if let Some(raw) = event
            .expect("compact event")
            .into_parts()
            .1
            .and_then(|wire| wire.into_raw_json_body())
        {
            let parsed: Value = serde_json::from_slice(&raw).expect("compact JSON");
            output = Some(parsed["output"].clone());
        }
    }
    drop(stream);
    let output = output.expect("compacted output");
    let generate = Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object(
            "openai",
            json!({"model":"gpt-5.4","input":output})
                .as_object()
                .expect("object")
                .clone(),
        )
        .expect("payload")
        .with_context(Map::from_iter([("use_websocket".to_owned(), json!(false))])),
    ));
    let mut stream = provider
        .execute(
            planned_request("openai", generate),
            context("req_after_compact", CancellationToken::new()),
        )
        .await
        .expect("generate");
    while let Some(event) = stream.next().await {
        event.expect("generate event");
    }
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 2);
    let compact = captured_request_body(&requests[0]);
    assert_eq!(compact["model"], "gpt-5.4", "mapped model is sent upstream");
    assert_eq!(compact["future"], json!(9_007_199_254_740_993_u64));
    let generate = captured_request_body(&requests[1]);
    assert_eq!(
        generate["input"], output,
        "opaque compaction items remain intact"
    );
}
