use super::*;

#[tokio::test]
async fn cache_fingerprints_are_opt_in_bounded_and_never_retain_prompt_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
        )
        .expect(6)
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let provider = provider_with_base_url_and_retry_budget(&store, server.uri(), 0);
    let mut summaries = Vec::new();
    for index in 0..6 {
        let instruction = if index == 3 {
            "synthetic_changed"
        } else {
            "synthetic_private_instruction"
        };
        let tools = if index == 4 {
            json!([{"type":"function", "name":"synthetic", "description": "x".repeat(1_000_000)}])
        } else {
            json!([{"type":"function", "name":"synthetic", "description":"synthetic_private_tool"}])
        };
        let mut body = json!({
            "model":"gpt-5.4", "input":[{"role":"user","content":"synthetic_private_prompt"}],
            "instructions":instruction, "tools":tools, "prompt_cache_key":"synthetic_cache",
        });
        if index == 5 {
            body["text"] = serde_json::from_str(&"9".repeat(80_000)).unwrap();
        }
        let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
                .unwrap()
                .with_context(Map::from_iter([("use_websocket".to_owned(), json!(false))])),
        ));
        let trace = if index > 0 {
            gateway_core::diagnostics::TraceContext::new("req_cache_diagnostics")
        } else {
            gateway_core::diagnostics::TraceContext::default()
        };
        let attempt = AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_cache_diagnostics").unwrap(),
                ClientApiKeyId::new("key_openai_contract").unwrap(),
            )
            .with_trace(trace),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(30),
            account_policy(),
            AccountAttemptContext::new(BTreeSet::new(), None, None)
                .with_account_scope(contract_account_scope()),
            None,
            CancellationToken::new(),
        );
        let mut stream = provider
            .execute(planned_request("openai", operation), attempt)
            .await
            .unwrap();
        let mut summary = None;
        while let Some(event) = timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
        {
            let event = event.unwrap();
            if let Some(metadata) = event
                .response_observation()
                .and_then(|o| o.provider_metadata())
            {
                let value: Value = serde_json::from_str(metadata.as_json()).unwrap();
                summary = Some(value["requestSummary"].clone());
            }
        }
        let summary = summary.unwrap();
        for raw in [
            "synthetic_private_instruction",
            "synthetic_private_tool",
            "synthetic_private_prompt",
            "synthetic_changed",
        ] {
            assert!(!summary.to_string().contains(raw));
        }
        assert!(summary.to_string().len() < 4096);
        summaries.push(summary);
    }
    assert!(summaries[0].get("cacheFingerprints").is_none());
    let fingerprints = |i: usize| &summaries[i]["cacheFingerprints"];
    assert_eq!(
        fingerprints(1)["instructions"],
        fingerprints(2)["instructions"]
    );
    assert_ne!(
        fingerprints(2)["instructions"],
        fingerprints(3)["instructions"]
    );
    assert_eq!(
        fingerprints(1)["inputPrefix"],
        fingerprints(2)["inputPrefix"]
    );
    assert!(
        fingerprints(1)["text"].is_null(),
        "missing values remain unknown"
    );
    assert_eq!(fingerprints(4)["tools"]["omitted"], "inspection_limit");
    assert_eq!(fingerprints(4)["maxInspectedBytes"], 65536);
    assert_eq!(fingerprints(5)["text"]["omitted"], "inspection_limit");
}
