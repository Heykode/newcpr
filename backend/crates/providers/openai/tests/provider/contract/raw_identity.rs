use super::*;
use gateway_core::operation::CompactRequest;

const ENDPOINTS: [&str; 4] = [
    "images/generations",
    "images/edits",
    "alpha/search",
    "responses/compact",
];

const CLEAN_BODY: &[u8] = br#" {
  "model":"gpt-5.4", "input":[{"type":"compaction","encrypted_content":"opaque"}],
  "id":"shared-root", "session_id":"shared-root", "conversation_id":"client-correlation",
  "prompt":"literal \u4e2d text", "commands":{"search_query":[{"q":"test","account_id":"business"}]},
  "images":[{"image_url":"data:image/png;base64,c3ludGhldGlj"}],
  "metadata":{"account_id":"business"}, "future":9007199254740993,
  "future":1.23456789012345678901234567890, "tools":[{"name":"account_id"}]
} "#;

const IDENTITY_BODY: &[u8] = br#" {
  "model":"gpt-5.4", "input":[{"type":"compaction","encrypted_content":"opaque"}],
  "id":"shared-root", "session_id":"shared-root", "conversation_id":"client-correlation",
  "prompt":"literal \u4e2d text", "commands":{"search_query":[{"q":"test","account_id":"business"}]},
  "images":[{"image_url":"data:image/png;base64,c3ludGhldGlj"}],
  "metadata":{"account_id":"business"}, "future":9007199254740993,
  "future":1.23456789012345678901234567890, "tools":[{"name":"account_id"}],
  "account_id":"discard-me", "account\u005fid":"discard-me",
  "access_token":"discard-me", "cookie":"discard-me", "conversation":"discard-me",
  "turnState":"discard-me", "installation_id":"discard-me", "installation_id":"discard-me",
  "installationId":null, "x-codex-installation-id":"discard-me",
  "client_metadata":{
    "account_id":"discard-me", "account_id":"discard-me", "response_id":"discard-me",
    "x-codex-turn-state":"discard-me", "installation_id":"discard-me",
    "installationId":"discard-me", "x-codex-installation-id":"discard-me",
    "future_meta":9007199254740993, "future_meta":1.23456789012345678901234567890
  },
  "turnMetadata":"{\"account_id\":\"discard-me\",\"installation_id\":\"discard-me\",\"future_turn\":1,\"future_turn\":2}"
} "#;

fn raw_operation(endpoint: &str, body: Bytes) -> Operation {
    let payload = RawJsonPayload::new("openai", body).expect("raw payload");
    match endpoint {
        "images/generations" => Operation::GenerateImage(ImageRequest::from_raw_json(
            ImageRequestKind::Generation,
            payload,
        )),
        "images/edits" => {
            Operation::GenerateImage(ImageRequest::from_raw_json(ImageRequestKind::Edit, payload))
        }
        "alpha/search" => Operation::Search(StandaloneSearchRequest::from_raw_json(payload)),
        "responses/compact" => Operation::Compact(CompactRequest::from_raw_json(payload)),
        _ => panic!("unexpected endpoint"),
    }
}

fn raw_planned(endpoint: &str, operation: Operation) -> ProviderRequest {
    if endpoint == "responses/compact" {
        planned_request("openai", operation)
    } else {
        planned_provider_endpoint_request("openai", operation)
    }
}

async fn mount_response(server: &MockServer, endpoint: &str, expected: u64) {
    Mock::given(method("POST"))
        .and(path(format!("/codex/{endpoint}")))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("connection", "close")
                .set_body_json(json!({
                    "output":[{"type":"compaction","encrypted_content":"synthetic-result"}]
                })),
        )
        .expect(expected)
        .mount(server)
        .await;
}

async fn execute_raw(
    provider: &CodexProvider,
    endpoint: &str,
    operation: Operation,
    attempt: AttemptContext,
) -> String {
    let mut stream = provider
        .execute(raw_planned(endpoint, operation), attempt)
        .await
        .expect("account selection");
    let account = stream.metadata().provider_account_id().as_str().to_owned();
    assert_eq!(stream.metadata().transport().as_str(), "http_json");
    let mut completed = 0;
    while let Some(event) = stream.next().await {
        completed += event
            .expect("raw endpoint response")
            .canonical_facts()
            .iter()
            .filter(|fact| matches!(fact, GatewayEvent::Completed(_)))
            .count();
    }
    assert_eq!(completed, 1);
    account
}

fn assert_business_preserved(raw: &str) {
    for part in [
        r#""input":[{"type":"compaction","encrypted_content":"opaque"}]"#,
        r#""commands":{"search_query":[{"q":"test","account_id":"business"}]}"#,
        r#""images":[{"image_url":"data:image/png;base64,c3ludGhldGlj"}]"#,
        r#""metadata":{"account_id":"business"}"#,
        r#""prompt":"literal \u4e2d text""#,
        r#""future":9007199254740993"#,
        r#""future":1.23456789012345678901234567890"#,
        r#""tools":[{"name":"account_id"}]"#,
    ] {
        assert!(raw.contains(part), "original business bytes lost: {part}");
    }
}

fn assert_scoped_request(request: &wiremock::Request, account: &str) -> String {
    assert_eq!(
        captured_header_values(request, "authorization"),
        vec![format!("Bearer at-{account}").into_bytes()]
    );
    assert_eq!(
        captured_header_values(request, "chatgpt-account-id"),
        vec![format!("chatgpt-{account}").into_bytes()]
    );
    assert!(captured_header_values(request, "x-codex-installation-id").is_empty());
    let raw = std::str::from_utf8(&request.body).expect("UTF-8 body");
    assert!(!raw.contains("discard-me"), "old account identity survived");
    assert_business_preserved(raw);
    assert!(raw.contains(r#""future_meta":9007199254740993"#));
    assert!(raw.contains(r#""future_meta":1.23456789012345678901234567890"#));
    let body: Value = serde_json::from_slice(&request.body).expect("scoped JSON");
    for name in [
        "account_id",
        "access_token",
        "cookie",
        "conversation",
        "turnState",
    ] {
        assert!(body.get(name).is_none());
    }
    assert_eq!(body["session_id"], "shared-root");
    assert_eq!(body["id"], "shared-root");
    assert_eq!(body["conversation_id"], "client-correlation");
    let device = body["installation_id"].as_str().expect("current device");
    assert!(!device.is_empty());
    assert_eq!(body["installationId"], device);
    assert_eq!(body["x-codex-installation-id"], device);
    let metadata = &body["client_metadata"];
    for name in ["account_id", "response_id", "x-codex-turn-state"] {
        assert!(metadata.get(name).is_none());
    }
    for name in [
        "installation_id",
        "installationId",
        "x-codex-installation-id",
    ] {
        assert_eq!(metadata[name], device);
    }
    let raw_turn = body["turnMetadata"].as_str().expect("turn metadata");
    assert!(raw_turn.contains(r#""future_turn":1"#));
    assert!(raw_turn.contains(r#""future_turn":2"#));
    let turn: Value = serde_json::from_str(raw_turn).expect("turn metadata JSON");
    assert!(turn.get("account_id").is_none());
    assert_eq!(turn["installation_id"], device);
    device.to_owned()
}

#[tokio::test]
async fn standalone_endpoints_keep_clean_wire_bytes_without_inventing_identity_fields() {
    for endpoint in ENDPOINTS {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let server = MockServer::start().await;
        mount_response(&server, endpoint, 1).await;
        execute_raw(
            &provider_with_base_url(&store, server.uri()),
            endpoint,
            raw_operation(endpoint, Bytes::from_static(CLEAN_BODY)),
            context("req_raw_identity_clean", CancellationToken::new()),
        )
        .await;
        let requests = server.received_requests().await.expect("requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].body, CLEAN_BODY);
        assert!(captured_header_values(&requests[0], "x-codex-installation-id").is_empty());
        server.verify().await;
    }
}

#[tokio::test]
async fn standalone_endpoints_rebuild_identity_after_account_change_without_mutating_input() {
    for endpoint in ENDPOINTS {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_affinity_switch_a").await;
        let server = MockServer::start().await;
        mount_response(&server, endpoint, 2).await;
        let provider = provider_with_base_url(&store, server.uri());
        let original = Bytes::from_static(IDENTITY_BODY);
        let operation = raw_operation(endpoint, original.clone());
        assert_eq!(
            execute_raw(
                &provider,
                endpoint,
                operation.clone(),
                context("req_raw_identity_a", CancellationToken::new()),
            )
            .await,
            "acct_affinity_switch_a"
        );
        create_account(&store, "acct_affinity_switch_b").await;
        store
            .set_enabled(
                &ProviderAccountId::new("acct_affinity_switch_a").expect("account ID"),
                false,
            )
            .await
            .expect("disable old account");
        assert_eq!(
            execute_raw(
                &provider,
                endpoint,
                operation,
                context_with_state_owner("req_raw_identity_b", "acct_affinity_switch_a"),
            )
            .await,
            "acct_affinity_switch_b"
        );
        assert_eq!(original, IDENTITY_BODY);
        let requests = server.received_requests().await.expect("requests");
        assert_eq!(requests.len(), 2, "identity repair adds no upstream probe");
        let first_device = assert_scoped_request(&requests[0], "acct_affinity_switch_a");
        let second_device = assert_scoped_request(&requests[1], "acct_affinity_switch_b");
        assert_ne!(first_device, second_device);
        server.verify().await;
    }
}

#[tokio::test]
async fn standalone_search_sends_formatted_unicode_metadata_without_losing_business_members() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let server = MockServer::start().await;
    mount_response(&server, "alpha/search", 1).await;
    let metadata = concat!(
        "{\r\n",
        r#""account_id":"discard-me", "installation_id":"discard-me","#,
        "\n",
        r#""future":9007199254740993, "future":1.23456789012345678901234567890,"#,
        "\n",
        r#""label":"中😀", "text":"keep\nline"}"#,
    );
    let payload = RawJsonPayload::new("openai", Bytes::from_static(IDENTITY_BODY))
        .expect("search payload")
        .with_context(Map::from_iter([(
            "turn_metadata".to_owned(),
            json!(metadata),
        )]));
    execute_raw(
        &provider_with_base_url(&store, server.uri()),
        "alpha/search",
        Operation::Search(StandaloneSearchRequest::from_raw_json(payload)),
        context("req_search_formatted_metadata", CancellationToken::new()),
    )
    .await;
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 1);
    let device = assert_scoped_request(&requests[0], "acct_provider_contract");
    let headers = captured_header_values(&requests[0], "x-codex-turn-metadata");
    assert_eq!(headers.len(), 1, "formatted metadata must not be omitted");
    let raw = std::str::from_utf8(&headers[0]).expect("ASCII header");
    assert!(raw.is_ascii());
    assert!(!raw.contains(['\r', '\n']));
    assert!(!raw.contains("discard-me"));
    assert!(raw.contains(r#""future":9007199254740993"#));
    assert!(raw.contains(r#""future":1.23456789012345678901234567890"#));
    assert!(raw.contains(r#""text":"keep\nline""#));
    let scoped: Value = serde_json::from_str(raw).expect("metadata JSON");
    assert_eq!(scoped["installation_id"], device);
    assert_eq!(scoped["label"], "中😀");
    assert_eq!(scoped["text"], "keep\nline");
    server.verify().await;
}
