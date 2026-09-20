//! Wire-level ownership checks across official request normalization.

use gateway_core::provider_ports::ProviderStorePorts;

use super::*;

struct Capture {
    headers: reqwest::header::HeaderMap,
    body: Value,
    state: Value,
    affinity: Option<String>,
}

async fn capture(
    accounts: &Arc<MemoryAccountStore>,
    account_id: &str,
    runtime: &std::path::Path,
    body: Value,
    websocket: bool,
    same_account: bool,
) -> Capture {
    let http = MockServer::start().await;
    let (url, websocket_server) = if websocket {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let headers = Arc::new(Mutex::new(None));
            let captured = Arc::clone(&headers);
            let mut socket = crate::transport::accept_codex_test_websocket_with(
                stream,
                move |request, _response| {
                    *captured.lock().unwrap() = Some(request.headers().clone());
                },
            )
            .await;
            let frame = socket.next().await.unwrap().unwrap();
            let body: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
            super::generate_compat::send_completion(&mut socket, "resp_alignment").await;
            let headers = headers.lock().unwrap().take().unwrap();
            (headers, body)
        });
        (url, Some(server))
    } else {
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(format!(
                        "event: response.created\ndata: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_scope_capture\",\"model\":\"gpt-5.4\"}}}}\n\n{CAPTURE_COMPLETED_SSE}"
                    )),
            )
            .expect(1)
            .mount(&http)
            .await;
        (http.uri(), None)
    };
    let original = body.as_object().unwrap().clone();
    let raw_metadata = r#"{"installation_id":"old-device","workspaces":{"/tmp/\u4e2d\u6587":{"label":"caf\u00e9"}}}"#;
    let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", original.clone())
            .unwrap()
            .with_context(Map::from_iter([
                ("use_websocket".into(), json!(websocket)),
                ("turn_metadata".into(), json!(raw_metadata)),
                (
                    "opaque_request_headers".into(),
                    json!([["x-codex-turn-metadata", STANDARD.encode(raw_metadata)]]),
                ),
            ])),
    ));
    let ports = crate::admin::provider_ports();
    let ports = ProviderStorePorts::new(
        accounts.clone(),
        Arc::new(TestLeaseCoordinator::default()),
        Arc::new(MemorySessionAffinity::default()),
        ports.session_exclusions(),
        catalog_cache(),
        ports.artifact_profiles(),
        ports.credential_state(),
        ports.cooldowns(),
        ports.runtime_policy(),
        ports.oauth_pending(),
    );
    let mut config = crate::admin::valid_config().config;
    config.api.base_url = url;
    config.resolve_and_validate(runtime).unwrap();
    let bundle = provider_openai::initialize(config, ports).await.unwrap();
    let provider = bundle.core_provider();
    let affinity = provider
        .request_observation(
            &operation,
            &ClientApiKeyId::new("key_openai_contract").unwrap(),
        )
        .continuation
        .affinity_hash;
    let scope = Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([(
            ProviderAccountId::new(account_id).unwrap(),
            RuntimeAccount::new(ProviderKind::new("openai").unwrap(), BTreeSet::new()),
        )]))),
        ClientRoutingScope::all_accounts(),
    ));
    let owner = ProviderAccountStateOwner::new(
        ProviderKind::new("openai").unwrap(),
        ProviderAccountId::new(if same_account {
            account_id
        } else {
            "acct_scope_old"
        })
        .unwrap(),
    );
    let context = AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_alignment").unwrap(),
            ClientApiKeyId::new("key_openai_contract").unwrap(),
        ),
        NonZeroU32::new(1).unwrap(),
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::new(BTreeSet::new(), None, Some(owner)).with_account_scope(scope),
        None,
        CancellationToken::new(),
    );
    let mut stream = provider
        .execute(planned_request("openai", operation.clone()), context)
        .await
        .unwrap();
    let mut state = Value::Null;
    let mut completed = false;
    while let Some(event) = timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
    {
        let event = event.unwrap();
        completed |= event
            .canonical_facts()
            .iter()
            .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
        if let Some(update) = event.session_update() {
            state = Value::Object(update.payload().clone());
        }
    }
    assert!(completed);
    assert_eq!(state["account_id"], account_id);
    assert_eq!(state["client_api_key_id"], "key_openai_contract");
    for name in ["session", "thread"] {
        assert!(
            state["identity_seed"][name]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
    }
    assert!(
        state["conversation_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("lc_"))
    );
    assert!(affinity.is_some());
    let Operation::Generate(request) = operation else {
        unreachable!()
    };
    assert_eq!(request.protocol_payload().body(), &original);
    let (headers, body) = if let Some(server) = websocket_server {
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
    } else {
        let requests = http.received_requests().await.unwrap();
        (
            requests[0].headers.clone(),
            captured_request_body(&requests[0]),
        )
    };
    assert!(
        body["prompt_cache_key"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    for name in [
        "authorization",
        "chatgpt-account-id",
        "x-codex-installation-id",
        "session_id",
    ] {
        assert!(!headers[name].is_empty(), "{name}");
    }
    Capture {
        headers,
        body,
        state,
        affinity,
    }
}

fn stable_facts(capture: &Capture) -> Value {
    let headers: Map<_, _> = [
        "authorization",
        "chatgpt-account-id",
        "x-codex-installation-id",
        "session-id",
        "session_id",
        "thread-id",
        "x-client-request-id",
        "x-codex-window-id",
        "user-agent",
        "x-codex-routing-hint",
    ]
    .into_iter()
    .map(|name| {
        (
            name.to_owned(),
            capture
                .headers
                .get(name)
                .map(|value| json!(value.to_str().unwrap()))
                .unwrap_or(Value::Null),
        )
    })
    .collect();
    json!({
        "headers":headers, "affinity":capture.affinity,
        "input":capture.body["input"], "cache":capture.body["prompt_cache_key"],
        "account":capture.state["account_id"], "seed":capture.state["identity_seed"],
        "conversation":capture.state["conversation_id"],
        "continuation":capture.state["continuation_scope"]
    })
}

#[tokio::test]
async fn string_and_message_inputs_keep_wire_identity_affinity_and_cache() {
    let accounts = Arc::new(MemoryAccountStore::default());
    let account_id = format!("acct_alignment_{}", uuid::Uuid::new_v4());
    create_account(&accounts, &account_id).await;
    let runtime = tempfile::tempdir().unwrap();
    for websocket in [false, true] {
        for text in ["hello", "", " \n ", "\u{4e2d}\u{6587}\ntext"] {
            for explicit in [false, true] {
                let mut body = json!({
                    "model":"gpt-5.4", "input":text, "instructions":"stable instructions",
                    "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]
                });
                if explicit {
                    body["session_id"] = json!("alignment-session");
                    body["thread_id"] = json!("alignment-thread");
                    body["prompt_cache_key"] = json!("alignment-cache");
                }
                let string = capture(
                    &accounts,
                    &account_id,
                    runtime.path(),
                    body.clone(),
                    websocket,
                    true,
                )
                .await;
                body["input"] = json!([{
                    "type":"message","role":"user","content":[{"type":"input_text","text":text}]
                }]);
                let message = capture(
                    &accounts,
                    &account_id,
                    runtime.path(),
                    body,
                    websocket,
                    true,
                )
                .await;
                assert_eq!(
                    stable_facts(&string),
                    stable_facts(&message),
                    "ws={websocket},explicit={explicit},text={text:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn installation_alias_shape_keeps_account_fingerprint_routing_and_cache() {
    let accounts = Arc::new(MemoryAccountStore::default());
    let account_id = format!("acct_alignment_{}", uuid::Uuid::new_v4());
    create_account(&accounts, &account_id).await;
    let runtime = tempfile::tempdir().unwrap();
    for websocket in [false, true] {
        for same_account in [false, true] {
            let base = json!({
                "model":"gpt-5.4", "input":"hello", "instructions":"stable",
                "metadata":{"installation_id":"business-not-device"}
            });
            let clean = capture(
                &accounts,
                &account_id,
                runtime.path(),
                base.clone(),
                websocket,
                same_account,
            )
            .await;
            let device = clean.headers["x-codex-installation-id"].to_str().unwrap();
            assert_eq!(
                clean.body["client_metadata"]["x-codex-installation-id"],
                device
            );
            assert!(
                clean.body["client_metadata"]
                    .get("installation_id")
                    .is_none()
            );
            assert!(
                clean.body["client_metadata"]
                    .get("installationId")
                    .is_none()
            );
            assert!(clean.body.get("installation_id").is_none());
            for alias in [
                "installation_id",
                "installationId",
                "x-codex-installation-id",
            ] {
                for old in [
                    json!("old-device"),
                    Value::Null,
                    json!({"invalid":"device"}),
                ] {
                    let mut body = base.clone();
                    body[alias] = old.clone();
                    body["client_metadata"] = json!({alias:old, "business":"preserve"});
                    let result = capture(
                        &accounts,
                        &account_id,
                        runtime.path(),
                        body,
                        websocket,
                        same_account,
                    )
                    .await;
                    assert_eq!(result.body[alias], device);
                    assert_eq!(result.body["client_metadata"][alias], device);
                    assert_eq!(
                        result.body["client_metadata"]["x-codex-installation-id"],
                        device
                    );
                    assert_eq!(result.body["metadata"], base["metadata"]);
                    assert_eq!(result.body["client_metadata"]["business"], "preserve");
                    assert_eq!(
                        stable_facts(&clean),
                        stable_facts(&result),
                        "ws={websocket},same={same_account},alias={alias}"
                    );
                    let metadata = result.headers["x-codex-turn-metadata"].to_str().unwrap();
                    assert!(metadata.is_ascii());
                    let parsed: Value = serde_json::from_str(metadata).unwrap();
                    assert_eq!(parsed["installation_id"], device);
                    assert_eq!(
                        parsed["workspaces"]["/tmp/\u{4e2d}\u{6587}"]["label"],
                        "caf\u{e9}"
                    );
                }
            }
        }
    }
}
