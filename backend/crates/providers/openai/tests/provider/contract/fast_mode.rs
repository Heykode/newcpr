use super::*;
use gateway_core::account::FastMode;
use gateway_core::routing::{ModelPresentation, ModelServiceTier};

fn fast_request(operation: Operation, mode: FastMode, priority_catalog: bool) -> ProviderRequest {
    let provider = ProviderKind::new("openai").expect("provider");
    let model = UpstreamModelId::new("gpt-5.4").expect("model");
    let scope = Arc::new(
        FrozenAccountScope::new(
            Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([(
                ProviderAccountId::new("acct_provider_contract").expect("account"),
                RuntimeAccount::new(provider.clone(), BTreeSet::new()),
            )]))),
            ClientRoutingScope::all_accounts(),
        )
        .with_fast_mode(mode),
    );
    let mut provider_model = ProviderModel::new(
        provider.clone(),
        model,
        ModelCapabilities::new(BTreeSet::from([operation.kind()]), Some(32_000))
            .with_upstream_feature_validation(),
    );
    if priority_catalog {
        provider_model = provider_model.with_presentation(
            ModelPresentation::new(None, None)
                .with_service_tiers(vec![ModelServiceTier::new("priority", "Fast", "Fast")]),
        );
    }
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("revision"),
        account_policy(),
        vec![provider],
        vec![provider_model],
        Vec::new(),
    )
    .expect("snapshot");
    let plan = snapshot
        .plan(
            &PublicModelId::new("gpt-5.4").expect("public model"),
            &operation,
            scope,
            &RoutingContext::default(),
        )
        .expect("plan");
    ProviderRequest::new(operation, plan.candidates()[0].clone())
}

async fn capture_fast_request(
    mode: FastMode,
    priority_catalog: bool,
    tier: Option<Value>,
) -> (Value, Value) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!(
                    "event: response.created\ndata: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_scope_capture\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\"}}}}\n\n{CAPTURE_COMPLETED_SSE}"
                )),
        )
        .expect(2)
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let mut body = json!({
        "model": "gpt-5.4",
        "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "keep"}]}],
        "client_metadata": {"thread_id": "root", "service_tier": "nested-keep"}
    })
    .as_object()
    .expect("body")
    .clone();
    if let Some(tier) = tier {
        body.insert("service_tier".into(), tier);
    }
    let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", body)
            .expect("payload")
            .with_context(Map::from_iter([("use_websocket".to_owned(), json!(false))])),
    ));
    let provider = provider_with_base_url(&store, server.uri());
    for selected_mode in [FastMode::Default, mode] {
        let mut stream = provider
            .execute(
                fast_request(operation.clone(), selected_mode, priority_catalog),
                context("req_fast_policy", CancellationToken::new()),
            )
            .await
            .expect("provider stream");
        let mut completed = false;
        while let Some(event) = stream.next().await {
            completed |= event
                .expect("provider event")
                .canonical_facts()
                .iter()
                .any(|event| matches!(event, GatewayEvent::Completed(_)));
        }
        assert!(completed);
    }
    let requests = server.received_requests().await.expect("captured request");
    assert_eq!(requests.len(), 2);
    (
        captured_request_body(&requests[0]),
        captured_request_body(&requests[1]),
    )
}

#[tokio::test]
async fn fast_group_policy_only_changes_the_supported_top_level_service_tier() {
    for (mode, supported, before, expected) in [
        (FastMode::Default, true, None, None),
        (
            FastMode::Default,
            true,
            Some(json!("priority")),
            Some(json!("priority")),
        ),
        (FastMode::Enabled, true, None, Some(json!("priority"))),
        (
            FastMode::Enabled,
            true,
            Some(Value::Null),
            Some(json!("priority")),
        ),
        (
            FastMode::Enabled,
            true,
            Some(json!("default")),
            Some(json!("priority")),
        ),
        (FastMode::Enabled, false, None, None),
        (
            FastMode::Enabled,
            true,
            Some(json!("flex")),
            Some(json!("flex")),
        ),
        (
            FastMode::Enabled,
            true,
            Some(json!("auto")),
            Some(json!("auto")),
        ),
        (FastMode::Enabled, true, Some(json!(17)), Some(json!(17))),
        (
            FastMode::Disabled,
            true,
            Some(json!("priority")),
            Some(json!("default")),
        ),
        (
            FastMode::Disabled,
            false,
            Some(json!("fast")),
            Some(json!("default")),
        ),
        (
            FastMode::Disabled,
            true,
            Some(json!("flex")),
            Some(json!("flex")),
        ),
    ] {
        let (baseline, body) = capture_fast_request(mode, supported, before.clone()).await;
        assert_eq!(
            body.get("service_tier"),
            expected.as_ref(),
            "{mode:?}, {before:?}"
        );
        assert_eq!(body["model"], "gpt-5.4");
        assert_eq!(
            body["input"],
            json!([{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "keep"}]}])
        );
        let thread = body["client_metadata"]["thread_id"].as_str().unwrap();
        assert_ne!(thread, "root", "target account identity must remain scoped");
        assert_eq!(uuid::Uuid::parse_str(thread).unwrap().get_version_num(), 7);
        assert_eq!(body["prompt_cache_key"], thread);
        assert_eq!(body["client_metadata"]["service_tier"], "nested-keep");
        let mut baseline = baseline.as_object().unwrap().clone();
        let mut business_body = body.as_object().unwrap().clone();
        baseline.remove("service_tier");
        business_body.remove("service_tier");
        assert_eq!(
            business_body, baseline,
            "Fast may only change the top-level tier"
        );
        let (_, repeated) = capture_fast_request(mode, supported, expected.clone()).await;
        assert_eq!(repeated.get("service_tier"), expected.as_ref());
    }
}
