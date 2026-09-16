//! 通过公开 Provider 观测、Store 端口及初始化路径验证补充亲和，不开放私有实现。

use gateway_core::engine::provider::Provider;
use gateway_core::operation::CompactRequest;
use gateway_core::provider_ports::ProviderStorePorts;
use gateway_protocol::openai::OpenAiSchedulingSessionHint;

use super::*;

fn hint_context(source: &str, id: &str) -> Map<String, Value> {
    Map::from_iter([(
        "openai_scheduling_session_hint".to_owned(),
        OpenAiSchedulingSessionHint::new(source, id)
            .expect("hint")
            .to_context_value(),
    )])
}

fn generation(body: Value, context: Map<String, Value>) -> Operation {
    Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", body.as_object().expect("body").clone())
            .expect("payload")
            .with_context(context),
    ))
}

fn affinity(provider: &dyn Provider, operation: &Operation, client: &str) -> Option<String> {
    provider
        .request_observation(operation, &ClientApiKeyId::new(client).expect("client key"))
        .continuation
        .affinity_hash
}

fn raw_operations(body: Value, context: Map<String, Value>) -> [Operation; 3] {
    let payload = RawJsonPayload::new(
        "openai",
        Bytes::from(serde_json::to_vec(&body).expect("raw JSON")),
    )
    .expect("raw payload")
    .with_context(context);
    [
        Operation::Compact(CompactRequest::from_raw_json(payload.clone())),
        Operation::Search(StandaloneSearchRequest::from_raw_json(payload.clone())),
        Operation::GenerateImage(ImageRequest::from_raw_json(
            ImageRequestKind::Generation,
            payload,
        )),
    ]
}

async fn selection_keys(
    provider: &dyn Provider,
    store: &MemorySessionAffinity,
    operation: Operation,
    attempt: AttemptContext,
) -> Vec<String> {
    let before = store.lookup_keys().len();
    let stream = provider
        .execute(planned_request("openai", operation), attempt)
        .await
        .expect("prepare without sending upstream");
    drop(stream);
    store.lookup_keys()[before..].to_vec()
}

#[tokio::test]
async fn scheduling_hint_is_stable_across_content_changes_but_keeps_legacy_migration() {
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_scope_old").await;
    let bindings = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    let provider =
        provider_with_affinity_and_base_url(&accounts, Arc::clone(&bindings), server.uri());
    let body = json!({"input":"first","instructions":"old","tools":[]});
    let legacy = generation(body.clone(), Map::new());
    let hinted = generation(body, hint_context("x-session-id", "root"));
    let changed = generation(
        json!({"input":"changed","instructions":"new",
            "tools":[{"type":"function","name":"read_file"}]}),
        hint_context("x-session-id", "root"),
    );
    let client = "key_openai_contract";
    let new_key = affinity(&provider, &hinted, client).expect("hint key");
    // Content fallback includes the routed model; the observation model is not a routing key.
    let old_keys = selection_keys(
        &provider,
        &bindings,
        legacy.clone(),
        context("req_hint_legacy", CancellationToken::new()),
    )
    .await;
    assert_eq!(old_keys.len(), 1);
    let old_key = old_keys[0].clone();
    assert_ne!(new_key, old_key);
    assert_eq!(Some(new_key.clone()), affinity(&provider, &changed, client));
    let keys = selection_keys(
        &provider,
        &bindings,
        hinted.clone(),
        context("req_hint_migration", CancellationToken::new()),
    )
    .await;
    assert_eq!(keys, [new_key, old_key]);
    let Operation::Generate(legacy) = legacy else {
        unreachable!()
    };
    let Operation::Generate(hinted) = hinted else {
        unreachable!()
    };
    let legacy =
        provider_openai::encode_generate_request(&legacy, "gpt-5.4", &Default::default()).unwrap();
    let hinted =
        provider_openai::encode_generate_request(&hinted, "gpt-5.4", &Default::default()).unwrap();
    assert_eq!(hinted.client_session_id, None);
    assert_eq!(
        serde_json::to_value(&hinted).unwrap(),
        serde_json::to_value(&legacy).unwrap()
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn scheduling_hints_are_isolated_by_client_source_and_explicit_thread_retains_priority() {
    let provider = provider(&Arc::new(MemoryAccountStore::default()));
    let body = json!({"input":"hello"});
    let base = generation(body.clone(), hint_context("x-session-id", "root"));
    let root = affinity(&provider, &base, "client-a").expect("root");
    assert_ne!(Some(root.clone()), affinity(&provider, &base, "client-b"));
    for (source, id) in [("x-conversation-id", "root"), ("x-session-id", "other")] {
        assert_ne!(
            Some(root.clone()),
            affinity(
                &provider,
                &generation(body.clone(), hint_context(source, id)),
                "client-a"
            )
        );
    }
    let mut children = Vec::new();
    for thread in ["child-a", "child-b"] {
        let mut explicit = Map::from_iter([("thread_id".to_owned(), json!(thread))]);
        let baseline = generation(body.clone(), explicit.clone());
        explicit.extend(hint_context("x-session-id", "changed-root"));
        let hinted = generation(body.clone(), explicit);
        let key = affinity(&provider, &baseline, "client-a").expect("thread");
        assert_eq!(Some(key.clone()), affinity(&provider, &hinted, "client-a"));
        children.push(key);
    }
    assert_ne!(children[0], children[1]);
}

#[tokio::test]
async fn scheduling_hint_does_not_override_original_explicit_identity_or_previous_response() {
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_scope_old").await;
    let bindings = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    let provider =
        provider_with_affinity_and_base_url(&accounts, Arc::clone(&bindings), server.uri());
    for body in [
        json!({"input":"hello","session_id":"body-session"}),
        json!({"input":"hello","client_metadata":{"session_id":"metadata-session"}}),
        json!({"input":"hello","prompt_cache_key":"cache"}),
        json!({"input":[{"role":"user","content":"hello"}],"previous_response_id":"resp_old"}),
    ] {
        let baseline = generation(body.clone(), Map::new());
        let hinted = generation(body, hint_context("x-session-id", "new-hint"));
        assert_eq!(
            affinity(&provider, &baseline, "client"),
            affinity(&provider, &hinted, "client")
        );
    }
    for field in ["session_id", "conversation_id", "thread_id"] {
        let mut explicit = Map::from_iter([(field.to_owned(), json!("original"))]);
        let baseline = generation(json!({"input":"hello"}), explicit.clone());
        explicit.extend(hint_context("x-session-id", "new-hint"));
        let hinted = generation(json!({"input":"hello"}), explicit);
        let expected = affinity(&provider, &baseline, "key_openai_contract").expect("explicit");
        assert_eq!(
            Some(expected.clone()),
            affinity(&provider, &hinted, "key_openai_contract")
        );
        let keys = selection_keys(
            &provider,
            &bindings,
            hinted,
            context("req_explicit_hint", CancellationToken::new()),
        )
        .await;
        assert_eq!(
            keys,
            [expected],
            "explicit identity must not read a migration key"
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn scheduling_hint_works_without_a_content_anchor_and_ignores_wire_body_spoofing() {
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_scope_old").await;
    let bindings = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    let provider =
        provider_with_affinity_and_base_url(&accounts, Arc::clone(&bindings), server.uri());
    let hinted = generation(json!({}), hint_context("x-session-id", "root"));
    let keys = selection_keys(
        &provider,
        &bindings,
        hinted,
        context("req_empty_hint", CancellationToken::new()),
    )
    .await;
    assert_eq!(keys.len(), 1, "no content anchor means no migration lookup");
    let spoofed = generation(
        json!({"input":[{"role":"user","content":"hello"}],"context":{
            "openai_scheduling_session_hint":{"source":"x-session-id","id":"spoof"}}}),
        Map::new(),
    );
    let baseline = generation(
        json!({"input":[{"role":"user","content":"hello"}]}),
        Map::new(),
    );
    assert_eq!(
        affinity(&provider, &spoofed, "client"),
        affinity(&provider, &baseline, "client")
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn scheduling_hint_cannot_override_compact_cache_or_search_identity() {
    let provider = provider(&Arc::new(MemoryAccountStore::default()));
    for body in [
        json!({"session_id":"existing","prompt_cache_key":"cache"}),
        json!({"session_id":"existing","thread_id":"child","prompt_cache_key":"cache"}),
        json!({"prompt_cache_key":"cache"}),
        json!({"prompt_cache_key":"cache","thread_id":"thread-only"}),
    ] {
        let [baseline, _, _] = raw_operations(body.clone(), Map::new());
        let [hinted, _, _] = raw_operations(body, hint_context("x-session-id", "root"));
        let expected = affinity(&provider, &baseline, "client").expect("compact identity");
        assert_eq!(Some(expected), affinity(&provider, &hinted, "client"));
    }
    let body = json!({"id":"search-session"});
    let [_, baseline, _] = raw_operations(body.clone(), Map::new());
    let [_, hinted, _] = raw_operations(body, hint_context("x-session-id", "root"));
    let expected = affinity(&provider, &baseline, "client").expect("search identity");
    assert_eq!(Some(expected), affinity(&provider, &hinted, "client"));
}

#[test]
fn scheduling_hint_keeps_standalone_endpoint_threads_on_their_original_path() {
    let provider = provider(&Arc::new(MemoryAccountStore::default()));
    for (body, mut context) in [
        (json!({"thread_id":"thread-only"}), Map::new()),
        (
            json!({}),
            Map::from_iter([("thread_id".to_owned(), json!("thread-only"))]),
        ),
    ] {
        let baseline = raw_operations(body.clone(), context.clone());
        context.extend(hint_context("x-session-id", "new-root"));
        let hinted = raw_operations(body, context);
        for operation in baseline.into_iter().chain(hinted) {
            assert_eq!(affinity(&provider, &operation, "client"), None);
        }
    }
}

#[test]
fn scheduling_hint_uses_one_namespace_across_generate_and_raw_endpoints() {
    let provider = provider(&Arc::new(MemoryAccountStore::default()));
    let context = hint_context("x-session-id", "root");
    let generate = generation(json!({"input":"hello"}), context.clone());
    let expected = affinity(&provider, &generate, "client").expect("generate affinity");
    for operation in raw_operations(json!({"prompt":"image"}), context) {
        assert_eq!(
            affinity(&provider, &operation, "client"),
            Some(expected.clone())
        );
    }
}

fn initialized_ports(
    accounts: Arc<MemoryAccountStore>,
    bindings: Arc<MemorySessionAffinity>,
) -> ProviderStorePorts {
    let ports = crate::admin::provider_ports();
    ProviderStorePorts::new(
        accounts,
        Arc::new(TestLeaseCoordinator::default()),
        bindings,
        ports.session_exclusions(),
        catalog_cache(),
        ports.artifact_profiles(),
        ports.credential_state(),
        ports.cooldowns(),
        ports.runtime_policy(),
        ports.oauth_pending(),
    )
}

#[tokio::test]
async fn scheduling_hint_preserves_persisted_local_identity_and_account_projection() {
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_scope_old").await;
    let server = MockServer::start().await;
    // Canonical completion requires a matching response lifecycle start, not only wire delivery.
    let response = format!(
        "event: response.created\ndata: {}\n\n{CAPTURE_COMPLETED_SSE}",
        json!({
            "type": "response.created",
            "response": {
                "id": "resp_scope_capture",
                "model": "gpt-5.4",
                "status": "in_progress"
            }
        })
    );
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("connection", "close")
                .set_body_raw(response, "text/event-stream"),
        )
        .expect(6)
        .mount(&server)
        .await;
    let runtime = tempfile::tempdir().expect("isolated persisted identity");
    let mut config = crate::admin::valid_config();
    config.config.api.base_url = server.uri();
    config.config.resolve_and_validate(runtime.path()).unwrap();
    let body = json!({
        "input":[{"role":"user","content":"hello"}],
        "instructions":"stable prefix",
        "client_metadata":{"x-codex-turn-state":"unowned-state"}
    });
    let mut old_key = None;
    let mut local_identity = None;
    for hinted in [false, true] {
        let bindings = Arc::new(MemorySessionAffinity::default());
        let bundle = provider_openai::initialize(
            config.config.clone(),
            initialized_ports(Arc::clone(&accounts), Arc::clone(&bindings)),
        )
        .await
        .expect("initialize or reload real persisted identity");
        let provider = bundle.core_provider();
        for owner in [None, Some("acct_scope_old"), Some("acct_scope_new")] {
            let mut hints = Map::from_iter([("use_websocket".to_owned(), json!(false))]);
            if hinted {
                hints.extend(hint_context("x-session-id", "local-only"));
            }
            let attempt = owner.map_or_else(
                || context("req_persisted_hint", CancellationToken::new()),
                |owner| context_with_state_owner("req_persisted_hint", owner),
            );
            let before = bindings.lookup_keys().len();
            let mut stream = provider
                .execute(
                    planned_request("openai", generation(body.clone(), hints)),
                    attempt,
                )
                .await
                .expect("prepare account projection");
            if owner.is_none() {
                let keys = bindings.lookup_keys();
                if hinted {
                    assert_eq!(
                        keys.len() - before,
                        2,
                        "new key miss reads actual lc_ legacy"
                    );
                    assert_ne!(Some(keys[before].clone()), old_key);
                    assert_eq!(Some(keys[before + 1].clone()), old_key);
                } else {
                    assert_eq!(keys.len() - before, 1);
                    old_key = Some(keys[before].clone());
                }
            }
            let conversation = timeout(Duration::from_secs(10), async {
                let mut completed = false;
                let mut conversation = None;
                while let Some(event) = stream.next().await {
                    let event = event.expect("local completed response");
                    completed |= event
                        .canonical_facts()
                        .iter()
                        .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
                    if let Some(state) = event.session_update() {
                        conversation = state.payload()["conversation_id"]
                            .as_str()
                            .map(str::to_owned);
                    }
                }
                assert!(completed);
                conversation.expect("persisted conversation identity")
            })
            .await
            .expect("bounded local response");
            assert!(conversation.starts_with("lc_"));
            if let Some(expected) = &local_identity {
                assert_eq!(&conversation, expected);
            } else {
                local_identity = Some(conversation);
            }
        }
    }
    assert!(runtime.path().join("identity_hmac_secret").is_file());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 6);
    for (baseline, hinted) in requests[..3].iter().zip(&requests[3..]) {
        assert_eq!(baseline.body, hinted.body);
        for name in [
            "session_id",
            "session-id",
            "conversation_id",
            "conversation-id",
            "user-agent",
            "originator",
            "version",
            "chatgpt-account-id",
            "x-codex-installation-id",
            "x-codex-turn-state",
            "x-codex-turn-metadata",
        ] {
            assert_eq!(
                baseline.headers.get(name),
                hinted.headers.get(name),
                "{name}"
            );
        }
        assert!(!String::from_utf8_lossy(&hinted.body).contains("local-only"));
    }
    server.verify().await;
}
