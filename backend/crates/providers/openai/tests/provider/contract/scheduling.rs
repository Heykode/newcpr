use super::*;
use gateway_core::provider_ports::{ProviderSessionAffinityKey, ProviderSessionAffinityPort};

#[tokio::test]
async fn managed_state_gates_both_selectors_by_account_model_and_restores_when_disabled() {
    use crate::provider::turn_state::ProbeStates;
    use gateway_core::provider_ports::{
        OpaqueTurnState, ProviderTurnStateCandidate, ProviderTurnStatePort, ProviderTurnStateSlot,
        ProviderTurnStateValue,
    };
    use gateway_core::routing::OpenAiTurnStatePolicy;
    use gateway_core::runtime::RequestTuningHandle;
    use provider_openai::credential::CodexCredentialAdmin;

    for wait in [false, true] {
        let accounts = Arc::new(MemoryAccountStore::default());
        let mut imported = CodexCredentialAdmin
            .prepare_import(ImportCodexOAuthCredential {
                account_id: "acct_provider_contract".to_owned(),
                name: "Managed test".to_owned(),
                secret: secret("managed-test"),
                verified_account: profile("managed-owner"),
                next_refresh_at: None,
                enabled: true,
            })
            .unwrap();
        imported.account = imported.account.with_turn_state_injection_enabled(true);
        let account = imported.account.clone();
        accounts.create_account(imported).await.unwrap();
        let states = Arc::new(ProbeStates::default());
        let ports =
            crate::admin::provider_ports_with_accounts(accounts).with_turn_states(states.clone());
        let server = local_server().await;
        let mut config = crate::admin::valid_config();
        config.config.api.base_url = server.uri();
        let tuning = RequestTuningHandle::default();
        let model = UpstreamModelId::new("gpt-5.4").unwrap();
        tuning.publish_openai_turn_state_policy(OpenAiTurnStatePolicy::new(
            true,
            [model.clone()].into(),
        ));
        tuning.account_concurrency().publish(
            ConfigRevision::new(1).unwrap(),
            [(
                account.id().as_str().to_owned(),
                NonZeroU32::new(3).unwrap(),
            )]
            .into(),
        );
        let bundle =
            provider_openai::initialize_with_request_tuning(config.config, ports, tuning.clone())
                .await
                .unwrap();
        let provider = bundle.core_provider();
        let attempt = || {
            context("req_managed_gate", CancellationToken::new()).with_request_tuning(
                gateway_core::routing::RequestTuning {
                    account_busy_wait_enabled: wait,
                    ..Default::default()
                },
            )
        };
        let candidate = |model: UpstreamModelId, issued: SystemTime| ProviderTurnStateCandidate {
            account_id: account.id().clone(),
            expected_revision: account.revision(),
            expected_active_version: None,
            upstream_model: model,
            normal_length: 292,
            slot: ProviderTurnStateSlot::Active,
            value: ProviderTurnStateValue::new(
                OpaqueTurnState::new("s".repeat(292)),
                issued,
                issued + Duration::from_secs(3600),
            ),
            observed_at: SystemTime::now(),
        };
        states
            .put_candidate(candidate(
                UpstreamModelId::new("other-model").unwrap(),
                SystemTime::now(),
            ))
            .await
            .unwrap();
        let result = provider
            .execute(
                planned_request("openai", http_generate_operation()),
                attempt(),
            )
            .await;
        assert!(
            result.is_err(),
            "a different model's state must not permit scheduling"
        );
        assert!(server.received_requests().await.unwrap().is_empty());
        states
            .put_candidate(candidate(model.clone(), SystemTime::now()))
            .await
            .unwrap();
        let mut stream = provider
            .execute(
                planned_request("openai", http_generate_operation()),
                attempt(),
            )
            .await
            .unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        drop(stream);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].headers.get("x-codex-turn-state").unwrap(),
            "s".repeat(292).as_str()
        );
        states.records.lock().unwrap().clear();
        let result = provider
            .execute(
                planned_request("openai", http_generate_operation()),
                attempt(),
            )
            .await;
        assert!(
            result.is_err(),
            "lost active must immediately exclude the account"
        );
        tuning.publish_openai_turn_state_policy(OpenAiTurnStatePolicy::default());
        // Preserve the original scheduling policy's 10ms interval between business requests.
        tokio::time::sleep(Duration::from_millis(15)).await;
        let mut stream = provider
            .execute(
                planned_request("openai", http_generate_operation()),
                attempt(),
            )
            .await
            .unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(!requests[1].headers.contains_key("x-codex-turn-state"));
    }
}

fn hint_context(id: &str) -> Map<String, Value> {
    Map::from_iter([(
        "openai_scheduling_session_hint".to_owned(),
        json!({"source":"x-session-id","id":id}),
    )])
}

fn generation(hint: Option<&str>, input: &str) -> GenerateRequest {
    let mut context = Map::from_iter([("use_websocket".to_owned(), json!(false))]);
    if let Some(id) = hint {
        context.extend(hint_context(id));
    }
    GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object(
            "openai",
            Map::from_iter([
                ("input".to_owned(), json!([{"role":"user","content":input}])),
                ("model".to_owned(), json!("gpt-5.4")),
            ]),
        )
        .expect("payload")
        .with_context(context),
    )
}

async fn prepare(provider: &CodexProvider, generation: GenerateRequest, attempt: AttemptContext) {
    let stream = provider
        .execute(
            planned_request("openai", Operation::Generate(generation)),
            attempt,
        )
        .await
        .expect("prepare");
    drop(stream);
}

async fn bound(affinity: &MemorySessionAffinity, key: &str) -> ProviderAccountId {
    affinity
        .load(
            &ProviderKind::new("openai").expect("provider"),
            &ProviderSessionAffinityKey::try_new(key).expect("key"),
        )
        .await
        .expect("load")
        .expect("bound")
}

async fn local_server() -> MockServer {
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
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn scheduling_hint_migrates_once_without_rewriting_the_old_binding() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    create_account(&store, "acct_scope_new").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = local_server().await;
    let provider = provider_with_affinity_and_base_url(&store, Arc::clone(&affinity), server.uri());
    prepare(
        &provider,
        generation(None, "hello"),
        context("req_legacy", CancellationToken::new()),
    )
    .await;
    let old_key = affinity.lookup_keys()[0].clone();
    let kind = ProviderKind::new("openai").expect("provider");
    let old_account = ProviderAccountId::new("acct_scope_old").expect("account");
    let other_account = ProviderAccountId::new("acct_scope_new").expect("account");
    affinity.seed_binding(&kind, &old_key, old_account.clone());
    prepare(
        &provider,
        generation(Some("session-a"), "hello"),
        context("req_migrate", CancellationToken::new()),
    )
    .await;
    let lookups = affinity.lookup_keys();
    assert_eq!(lookups.len(), 3, "primary miss and one legacy lookup");
    let new_key = lookups[1].clone();
    assert_ne!(new_key, old_key);
    assert_eq!(lookups[2], old_key);
    assert_eq!(bound(&affinity, &new_key).await, old_account);
    assert_eq!(bound(&affinity, &old_key).await, old_account);

    affinity.seed_binding(&kind, &old_key, other_account.clone());
    let before = affinity.lookup_keys().len();
    prepare(
        &provider,
        generation(Some("session-a"), "different later input"),
        context("req_hit", CancellationToken::new()),
    )
    .await;
    assert_eq!(
        affinity.lookup_keys().len(),
        before + 1,
        "hit must not consult legacy"
    );
    assert_eq!(bound(&affinity, &new_key).await, old_account);
    assert_eq!(bound(&affinity, &old_key).await, other_account);
    assert!(
        server
            .received_requests()
            .await
            .expect("requests")
            .is_empty(),
        "preparation must not send upstream"
    );
}

#[tokio::test]
async fn scheduling_hint_migration_rechecks_disabled_scope_exclusion_quota_and_capacity() {
    for state in ["disabled", "outside-scope", "excluded", "quota", "busy"] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_new").await;
        let old_id = if state == "outside-scope" {
            "acct_outside_hint_scope"
        } else {
            "acct_scope_old"
        };
        create_account_with_enabled(&store, old_id, state != "disabled").await;
        let affinity = Arc::new(MemorySessionAffinity::default());
        let leases = Arc::new(TestLeaseCoordinator::default());
        let server = local_server().await;
        let provider = provider_with_affinity_and_base_url_and_leases(
            &store,
            Arc::clone(&affinity),
            server.uri(),
            Arc::clone(&leases),
        );
        prepare(
            &provider,
            generation(None, "hello"),
            context("req_old", CancellationToken::new()),
        )
        .await;
        let old_key = affinity.lookup_keys()[0].clone();
        let old_account = ProviderAccountId::new(old_id).expect("old account");
        affinity.seed_binding(
            &ProviderKind::new("openai").expect("provider"),
            &old_key,
            old_account.clone(),
        );
        if state == "busy" {
            leases
                .busy_accounts
                .lock()
                .expect("busy")
                .insert(old_account.clone());
        }
        if state == "quota" {
            let account = store.account(old_id).expect("account");
            store
                .apply_quota_access(QuotaAccessChange {
                    account_id: account.id().clone(),
                    expected_revision: account.revision(),
                    state: QuotaState::exhausted(
                        QuotaEvidence::UsageLimitReached,
                        SystemTime::now(),
                        None,
                    ),
                })
                .await
                .expect("quota");
        }
        let exclusions = if state == "excluded" {
            BTreeSet::from([old_account.clone()])
        } else {
            BTreeSet::new()
        };
        let attempt = AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_new").expect("request"),
                ClientApiKeyId::new("key_openai_contract").expect("client"),
            ),
            NonZeroU32::new(1).expect("attempt"),
            SystemTime::now() + Duration::from_secs(30),
            account_policy(),
            AccountAttemptContext::new(exclusions, None, None)
                .with_account_scope(contract_account_scope()),
            None,
            CancellationToken::new(),
        );
        prepare(&provider, generation(Some("session"), "hello"), attempt).await;
        let new_key = affinity.lookup_keys()[1].clone();
        assert_eq!(
            bound(&affinity, &new_key).await.as_str(),
            "acct_scope_new",
            "{state}"
        );
        assert_eq!(
            bound(&affinity, &old_key).await,
            old_account,
            "{state}: old key unchanged"
        );
    }
}

#[tokio::test]
async fn scheduling_hint_lookup_error_does_not_read_a_legacy_candidate() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_new").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = local_server().await;
    let provider = provider_with_affinity_and_base_url(&store, Arc::clone(&affinity), server.uri());
    affinity.fail_next_lookup();
    prepare(
        &provider,
        generation(Some("session"), "hello"),
        context("req_error", CancellationToken::new()),
    )
    .await;
    assert_eq!(
        affinity.lookup_keys().len(),
        1,
        "unavailable is not a missing binding"
    );
}

#[tokio::test]
async fn scheduling_hint_survives_an_ordinary_retry_with_an_account_state_owner() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = local_server().await;
    let provider = provider_with_affinity_and_base_url(&store, Arc::clone(&affinity), server.uri());
    prepare(
        &provider,
        generation(Some("same-session"), "hello"),
        context("req_first", CancellationToken::new()),
    )
    .await;
    let new_key = affinity.lookup_keys()[0].clone();
    let before = affinity.lookup_keys().len();
    prepare(
        &provider,
        generation(Some("same-session"), "hello"),
        context_with_state_owner("req_retry", "acct_scope_old"),
    )
    .await;
    assert_eq!(&affinity.lookup_keys()[before..], &[new_key]);
}

#[tokio::test]
async fn scheduling_hint_preserves_http_body_device_and_upstream_session_projection() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = local_server().await;
    let provider = provider_with_affinity_and_base_url(&store, affinity, server.uri());
    for hint in [None, Some("local-only-hint")] {
        let mut stream = provider
            .execute(
                planned_request(
                    "openai",
                    Operation::Generate(generation(hint, "same input")),
                ),
                context("req_wire", CancellationToken::new()),
            )
            .await
            .expect("prepare");
        while let Some(event) = stream.next().await {
            event.expect("response");
        }
    }
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].body, requests[1].body);
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
    ] {
        assert_eq!(
            requests[0].headers.get(name),
            requests[1].headers.get(name),
            "{name}"
        );
    }
    assert!(!String::from_utf8_lossy(&requests[1].body).contains("local-only-hint"));
}

#[tokio::test]
async fn scheduling_hint_concurrent_claims_keep_one_binding() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    create_account(&store, "acct_scope_new").await;
    let affinity = Arc::new(MemorySessionAffinity::with_claim_barrier(24));
    let server = local_server().await;
    let provider = provider_with_affinity_and_base_url(&store, Arc::clone(&affinity), server.uri());
    let operations = (0..24).map(|index| {
        let provider = &provider;
        async move {
            prepare(
                provider,
                generation(Some("shared"), "hello"),
                context(&format!("req_parallel_{index}"), CancellationToken::new()),
            )
            .await;
        }
    });
    timeout(
        Duration::from_secs(10),
        futures::future::join_all(operations),
    )
    .await
    .expect("all first claims finish after racing at the barrier");
    assert_eq!(affinity.binding_count(), 1);
}

#[tokio::test]
async fn scheduling_hint_migrated_sessions_fail_over_independently() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    create_account(&store, "acct_scope_new").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    let server = local_server().await;
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        Arc::clone(&affinity),
        server.uri(),
        Arc::clone(&leases),
    );
    prepare(
        &provider,
        generation(None, "hello"),
        context("req_old", CancellationToken::new()),
    )
    .await;
    let old_key = affinity.lookup_keys()[0].clone();
    let old_account = ProviderAccountId::new("acct_scope_old").unwrap();
    affinity.seed_binding(
        &ProviderKind::new("openai").unwrap(),
        &old_key,
        old_account.clone(),
    );
    let mut new_keys = Vec::new();
    for id in ["session-a", "session-b"] {
        let before = affinity.lookup_keys().len();
        prepare(
            &provider,
            generation(Some(id), "hello"),
            context("req_migrate", CancellationToken::new()),
        )
        .await;
        let key = affinity.lookup_keys()[before].clone();
        assert_eq!(bound(&affinity, &key).await, old_account);
        new_keys.push(key);
    }
    assert_ne!(new_keys[0], new_keys[1]);
    leases
        .busy_accounts
        .lock()
        .unwrap()
        .insert(old_account.clone());
    let retry = generation(Some("session-a"), "hello");
    let mut retry_context = retry.protocol_payload().context().clone();
    retry_context.insert("turn_state".to_owned(), json!("old-account-private-turn"));
    let retry = GenerateRequest::from_protocol_payload(
        retry.protocol_payload().clone().with_context(retry_context),
    );
    let mut stream = provider
        .execute(
            planned_request("openai", Operation::Generate(retry)),
            context_with_state_owner("req_failover", "acct_scope_old"),
        )
        .await
        .expect("escape busy account");
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_scope_new"
    );
    let mut completed = false;
    while let Some(event) = stream.next().await {
        let event = event.expect("successful response updates only session-a");
        completed |= event
            .canonical_facts()
            .iter()
            .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
    }
    assert!(
        completed,
        "the fixture must produce a canonical successful completion"
    );
    assert_eq!(
        bound(&affinity, &new_keys[0]).await.as_str(),
        "acct_scope_new"
    );
    assert_eq!(bound(&affinity, &new_keys[1]).await, old_account);
    assert_eq!(bound(&affinity, &old_key).await, old_account);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(captured_header_values(&requests[0], "x-codex-turn-state").is_empty());
    assert!(
        !captured_request_body(&requests[0])
            .to_string()
            .contains("old-account-private-turn"),
        "retaining the retry's routing key must not retain another account's state"
    );
}

#[tokio::test]
async fn scheduling_hint_cannot_move_a_live_websocket_continuation_to_another_account() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    create_account(&store, "acct_scope_new").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (release, released) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = accept_codex_test_websocket(socket).await;
        let mut bodies = Vec::new();
        for id in ["resp_owned_first", "resp_owned_second"] {
            let frame = timeout(Duration::from_secs(10), socket.next())
                .await
                .expect("same socket receives the next frame")
                .unwrap()
                .unwrap();
            bodies.push(serde_json::from_str::<Value>(frame.to_text().unwrap()).unwrap());
            socket
                .send(Message::Text(
                    json!({
                        "type":"response.completed",
                        "response":{"id":id,"model":"gpt-5.4","status":"completed","output":[],
                            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        }
        released.await.unwrap();
        bodies
    });
    let provider = provider_with_affinity_and_base_url(&store, Arc::clone(&affinity), base_url);
    prepare(
        &provider,
        generation(Some("conflicting"), "hello"),
        context("req_seed_hint", CancellationToken::new()),
    )
    .await;
    let hint_key = affinity.lookup_keys()[0].clone();
    affinity.seed_binding(
        &ProviderKind::new("openai").unwrap(),
        &hint_key,
        ProviderAccountId::new("acct_scope_new").unwrap(),
    );
    let generation = |hint: &str, previous: Option<&str>, state: ProviderSessionState| {
        let mut body = Map::from_iter([
            ("model".to_owned(), json!("gpt-5.4")),
            (
                "input".to_owned(),
                json!([{"role":"user","content":"hello"}]),
            ),
            ("store".to_owned(), json!(false)),
        ]);
        if let Some(previous) = previous {
            body.insert("previous_response_id".to_owned(), json!(previous));
        }
        let mut context = hint_context(hint);
        context.insert("use_websocket".to_owned(), json!(true));
        context.insert(
            "downstream_websocket_connection_id".to_owned(),
            json!("ws_owned_lane"),
        );
        GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object("openai", body)
                .unwrap()
                .with_context(context),
        )
        .with_provider_session_state(state)
    };
    let initial_state = ProviderSessionState::new(
        "openai",
        Map::from_iter([
            ("account_id".to_owned(), json!("acct_scope_old")),
            (
                "conversation_id".to_owned(),
                json!("lc_existing_owned_socket"),
            ),
            ("continuation_scope".to_owned(), json!("persisted")),
        ]),
    )
    .unwrap();
    let first = generation("first-hint", None, initial_state);
    let before = affinity.lookup_keys().len();
    prepare(
        &provider,
        first.clone(),
        context("req_seed_owner", CancellationToken::new()),
    )
    .await;
    let owner_key = affinity.lookup_keys()[before].clone();
    affinity.seed_binding(
        &ProviderKind::new("openai").unwrap(),
        &owner_key,
        ProviderAccountId::new("acct_scope_old").unwrap(),
    );
    let mut stream = provider
        .execute(
            planned_request("openai", Operation::Generate(first)),
            context("req_ws_first", CancellationToken::new()),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_scope_old"
    );
    let mut state = None;
    while let Some(event) = stream.next().await {
        if let Some(update) = event.unwrap().session_update() {
            state = Some(update.clone());
        }
    }
    let state = state.expect("real WS completion carries continuation state");
    assert_eq!(
        state.payload()["continuation_scope"],
        json!("connection_local")
    );
    let second = generation("conflicting", Some("resp_owned_first"), state);
    let mut stream = provider
        .execute(
            planned_request("openai", Operation::Generate(second)),
            pinned_continuation_context(
                "req_ws_second",
                "acct_scope_old",
                "resp_owned_first",
                "resp_owned_first",
                1,
                ContinuationAttempt::Native,
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_scope_old"
    );
    while let Some(event) = stream.next().await {
        event.expect("exact continuation keeps its live owner");
    }
    release.send(()).unwrap();
    let bodies = timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bodies[1]["previous_response_id"], json!("resp_owned_first"));
    assert!(
        !serde_json::to_string(&bodies)
            .unwrap()
            .contains("conflicting")
    );
    assert_eq!(bound(&affinity, &hint_key).await.as_str(), "acct_scope_new");
    assert_eq!(
        bound(&affinity, &owner_key).await.as_str(),
        "acct_scope_old"
    );
}

fn waiting_provider(
    store: &Arc<MemoryAccountStore>,
    leases: Arc<TestLeaseCoordinator>,
    base_url: String,
) -> CodexProvider {
    waiting_provider_with_exclusions(
        store,
        leases,
        base_url,
        Arc::new(MemorySessionExclusions::default()),
    )
}

fn waiting_provider_with_exclusions(
    store: &Arc<MemoryAccountStore>,
    leases: Arc<TestLeaseCoordinator>,
    base_url: String,
    exclusions: Arc<MemorySessionExclusions>,
) -> CodexProvider {
    let profile = wire_profile();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let catalog = Arc::new(CodexCredentialCatalogService::new(
        store.repository(),
        profile.clone(),
        http.clone(),
        base_url.clone(),
        catalog_cache(),
    ));
    let quota = Arc::new(CodexCredentialQuotaService::new(
        store.repository(),
        profile.clone(),
        http.clone(),
        base_url.clone(),
        Arc::new(MemoryCooldownPort::new()),
    ));
    let feedback = Arc::new(AccountFeedbackStats::default());
    let selector = CodexCredentialSelector::new(
        ProviderKind::new("openai").unwrap(),
        store.repository(),
        leases,
        Arc::new(MemorySessionAffinity::default()),
        exclusions,
        Arc::clone(&quota),
        Arc::clone(&feedback),
        CodexCookiePolicy::official().unwrap(),
    )
    .with_account_concurrency(gateway_core::runtime::AccountConcurrencyHandle::new(
        ConfigRevision::new(1).unwrap(),
        BTreeMap::from([
            ("acct_scope_old".to_owned(), NonZeroU32::MIN),
            ("acct_scope_new".to_owned(), NonZeroU32::MIN),
        ]),
    ));
    CodexProvider::new(
        Arc::new(selector),
        catalog,
        quota,
        feedback,
        http,
        profile,
        base_url,
        Arc::new(CodexWebSocketPool::default()),
        u32::try_from(DEFAULT_STREAM_MAX_RETRIES).unwrap(),
    )
    .unwrap()
}

fn waiting_attempt(attempt: AttemptContext) -> AttemptContext {
    attempt.with_request_tuning(gateway_core::routing::RequestTuning {
        account_busy_wait_enabled: true,
        ..Default::default()
    })
}

async fn provider_wait_queued(leases: &TestLeaseCoordinator) {
    timeout(Duration::from_secs(2), async {
        while leases
            .capacity
            .waiting
            .load(std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider reaches selector account wait");
}

#[tokio::test]
async fn discovery_catalog_missing_model_allows_immediate_and_queued_requests() {
    use std::sync::atomic::Ordering;
    for queued in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        let leases = Arc::new(TestLeaseCoordinator::default());
        let server = local_server().await;
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"models":[{"slug":"other-model","display_name":"Other"}]}),
                ),
            )
            .mount(&server)
            .await;
        let provider = waiting_provider(&store, Arc::clone(&leases), server.uri());
        let discovered = provider.query_model_capabilities().await.unwrap();
        assert!(!provider.model_catalog_is_exhaustive());
        assert!(
            discovered
                .iter()
                .all(|model| model.upstream_model().as_str() != "gpt-5.4")
        );
        let mut attempt = context("req_unlisted_model", CancellationToken::new());
        if queued {
            leases.capacity.enabled.store(true, Ordering::SeqCst);
            leases.capacity.set_load("acct_scope_old", 1);
            attempt = waiting_attempt(attempt);
        }
        let selected = provider.execute(
            planned_request("openai", Operation::Generate(generation(None, "hello"))),
            attempt,
        );
        tokio::pin!(selected);
        if queued {
            tokio::select! {
                result = &mut selected => panic!("must reach wait: {:?}", result.err()),
                () = provider_wait_queued(&leases) => {}
            }
            assert!(
                server
                    .received_requests()
                    .await
                    .unwrap()
                    .iter()
                    .all(|request| request.method == "GET")
            );
            leases.capacity.set_load("acct_scope_old", 0);
        }
        let mut stream = timeout(Duration::from_secs(3), selected)
            .await
            .unwrap()
            .expect("undiscovered model remains eligible");
        while let Some(event) = stream.next().await {
            event.expect("upstream response");
        }
        let sent = server.received_requests().await.unwrap();
        assert_eq!(
            sent.iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
        assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn capacity_wait_provider_responses_images_search_compact_do_not_send_or_commit_while_waiting()
 {
    use gateway_core::operation::CompactRequest;
    use std::sync::atomic::Ordering;
    for operation in [
        Operation::Generate(generation(None, "hello")),
        Operation::GenerateImage(ImageRequest::from_raw_json(
            ImageRequestKind::Generation,
            RawJsonPayload::new("openai", Bytes::from_static(br#"{"prompt":"image"}"#)).unwrap(),
        )),
        Operation::Search(StandaloneSearchRequest::from_raw_json(
            RawJsonPayload::new("openai", Bytes::from_static(br#"{"query":"test"}"#)).unwrap(),
        )),
        Operation::Compact(CompactRequest::from_raw_json(
            RawJsonPayload::new(
                "openai",
                Bytes::from_static(br#"{"model":"gpt-5.4","input":"hello"}"#),
            )
            .unwrap(),
        )),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        leases.capacity.set_load("acct_scope_old", 1);
        let server = local_server().await;
        let provider = waiting_provider(&store, Arc::clone(&leases), server.uri());
        let attempt = waiting_attempt(context("req_endpoint_wait", CancellationToken::new()));
        let cancellation = attempt.cancellation().clone();
        let planned = match operation {
            Operation::GenerateImage(_) | Operation::Search(_) => {
                planned_provider_endpoint_request("openai", operation)
            }
            _ => planned_request("openai", operation),
        };
        let selected = provider.execute(planned, attempt);
        tokio::pin!(selected);
        tokio::select! {
            result = &mut selected => panic!("endpoint returned before capacity: {:?}", result.err()),
            () = provider_wait_queued(&leases) => {}
        }
        assert!(server.received_requests().await.unwrap().is_empty());
        assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 1);
        cancellation.cancel();
        let error = selected.await.err().expect("cancelled selection");
        assert_eq!(error.kind(), ProviderErrorKind::Cancelled);
        assert_eq!(error.send_state(), UpstreamSendState::NotSent);
        assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn capacity_wait_provider_native_queue_full_cannot_authorize_core_replay_any() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    leases.capacity.set_load("acct_scope_old", 1);
    leases.capacity.full.store(true, Ordering::SeqCst);
    let server = local_server().await;
    let provider = waiting_provider(&store, leases, server.uri());
    let attempt = waiting_attempt(pinned_continuation_context(
        "req_native_wait",
        "acct_scope_old",
        "previous",
        "previous",
        1,
        ContinuationAttempt::Native,
    ));
    let error = provider
        .execute(
            planned_request("openai", Operation::Generate(generation(None, "hello"))),
            attempt,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), ProviderErrorKind::AccountCapacityUnavailable);
    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
    assert_eq!(
        error.continuation_recovery_disposition(),
        Some(ContinuationRecoveryDisposition::RetryExactConnection)
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn capacity_wait_provider_opens_websocket_only_after_account_promotion() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    leases.capacity.set_load("acct_scope_old", 1);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = waiting_provider(
        &store,
        Arc::clone(&leases),
        format!("http://{}", listener.local_addr().unwrap()),
    );
    let body = GenerateRequest::from_protocol_payload(
        generation(None, "hello")
            .protocol_payload()
            .clone()
            .with_context(Map::from_iter([
                ("use_websocket".to_owned(), json!(true)),
                (
                    "downstream_websocket_connection_id".to_owned(),
                    json!("ws_capacity_lane"),
                ),
            ])),
    )
    .with_provider_session_state(
        ProviderSessionState::new(
            "openai",
            Map::from_iter([
                ("account_id".to_owned(), json!("acct_scope_old")),
                (
                    "conversation_id".to_owned(),
                    json!("lc_capacity_owned_socket"),
                ),
                ("continuation_scope".to_owned(), json!("persisted")),
            ]),
        )
        .unwrap(),
    );
    let selected = provider.execute(
        planned_request("openai", Operation::Generate(body)),
        waiting_attempt(context("req_ws_capacity", CancellationToken::new())),
    );
    tokio::pin!(selected);
    tokio::select! {
        result = &mut selected => panic!("must wait: {:?}", result.err()),
        () = provider_wait_queued(&leases) => {}
    }
    assert!(
        timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err(),
        "no socket is held by an account waiter"
    );
    leases.capacity.set_load("acct_scope_old", 0);
    let mut stream = selected.await.unwrap();
    let server = async {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = accept_codex_test_websocket(socket).await;
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(
                json!({
                    "type":"response.created",
                    "response":{"id":"resp_after_capacity","model":"gpt-5.4","status":"in_progress"}
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        socket.send(Message::Text(json!({
            "type":"response.completed",
            "response":{"id":"resp_after_capacity","model":"gpt-5.4","status":"completed","output":[],
                "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}
        }).to_string().into())).await.unwrap();
        socket
    };
    let consume = async {
        let mut completed = false;
        let mut state = None;
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            if let Some(update) = event.session_update() {
                state = Some(update.clone());
            }
            completed |= event
                .canonical_facts()
                .iter()
                .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
        }
        assert!(completed);
        state.unwrap()
    };
    let (mut socket, state) = timeout(Duration::from_secs(10), async {
        tokio::join!(server, consume)
    })
    .await
    .unwrap();
    drop(stream);
    assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
    tokio::time::sleep(Duration::from_millis(15)).await;
    leases.capacity.set_load("acct_scope_old", 1);
    let continuation = GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object(
            "openai",
            Map::from_iter([
                ("model".to_owned(), json!("gpt-5.4")),
                ("input".to_owned(), json!("next")),
                (
                    "previous_response_id".to_owned(),
                    json!("resp_after_capacity"),
                ),
            ]),
        )
        .unwrap()
        .with_context(Map::from_iter([
            ("use_websocket".to_owned(), json!(true)),
            (
                "downstream_websocket_connection_id".to_owned(),
                json!("ws_capacity_lane"),
            ),
        ])),
    )
    .with_provider_session_state(state);
    let selected = provider.execute(
        planned_request("openai", Operation::Generate(continuation)),
        waiting_attempt(pinned_continuation_context(
            "req_native_capacity",
            "acct_scope_old",
            "resp_after_capacity",
            "resp_after_capacity",
            1,
            ContinuationAttempt::Native,
        )),
    );
    tokio::pin!(selected);
    tokio::select! {
        result = &mut selected => panic!("native owner must wait: {:?}", result.err()),
        () = provider_wait_queued(&leases) => {}
    }
    assert_eq!(
        leases.capacity.waits.lock().unwrap().last().unwrap().mode(),
        gateway_core::engine::AccountWaitMode::Sticky
    );
    let early_frame = timeout(Duration::from_millis(30), socket.next()).await;
    assert!(
        early_frame.is_err(),
        "no business frame during account wait: {early_frame:?}"
    );
    leases.capacity.set_load("acct_scope_old", 0);
    let mut stream = selected.await.unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_scope_old"
    );
    let serve_continuation = async {
        let frame = socket.next().await.unwrap().unwrap();
        let body: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
        assert_eq!(body["previous_response_id"], json!("resp_after_capacity"));
        socket.send(Message::Text(json!({
            "type":"response.completed",
            "response":{"id":"resp_after_native_wait","model":"gpt-5.4","status":"completed","output":[],
                "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}
        }).to_string().into())).await.unwrap();
    };
    let consume = async {
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
    };
    timeout(Duration::from_secs(10), async {
        tokio::join!(serve_continuation, consume)
    })
    .await
    .unwrap();
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err(),
        "native continuation reused its exact physical socket"
    );
    drop(stream);
    assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn capacity_wait_provider_reloads_cyber_exclusions_without_losing_frozen_core_exclusions() {
    use gateway_core::provider_ports::ProviderSessionExclusionPort;
    use std::sync::atomic::Ordering;
    for change in ["queued", "promoted", "read-failure"] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        create_account(&store, "acct_scope_new").await;
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.set_load("acct_scope_old", 1);
        let exclusions = Arc::new(MemorySessionExclusions::default());
        let server = local_server().await;
        let provider = waiting_provider_with_exclusions(
            &store,
            Arc::clone(&leases),
            server.uri(),
            Arc::clone(&exclusions),
        );
        let attempt = AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_wait_exclusions").unwrap(),
                ClientApiKeyId::new("key_openai_contract").unwrap(),
            ),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(30),
            account_policy(),
            AccountAttemptContext::new(
                BTreeSet::from([ProviderAccountId::new("acct_scope_new").unwrap()]),
                None,
                None,
            )
            .with_account_scope(contract_account_scope()),
            None,
            CancellationToken::new(),
        );
        let selected = provider.execute(
            planned_request(
                "openai",
                Operation::Generate(generate_with_session_context(
                    "cyber-wait-session",
                    None,
                    None,
                )),
            ),
            waiting_attempt(attempt),
        );
        tokio::pin!(selected);
        tokio::select! {
            result = &mut selected => panic!("must wait for permitted account: {:?}", result.err()),
            () = provider_wait_queued(&leases) => {}
        }
        let key = exclusions.lookups.lock().unwrap().last().unwrap().clone();
        let exclude = {
            let exclusions = Arc::clone(&exclusions);
            move || {
                futures::executor::block_on(exclusions.record_failure(
                    &ProviderKind::new("openai").unwrap(),
                    &key,
                    &ProviderAccountId::new("acct_scope_old").unwrap(),
                    Duration::from_secs(60),
                ))
                .unwrap();
            }
        };
        match change {
            "queued" => exclude(),
            "promoted" => *leases.capacity.after_acquire.lock().unwrap() = Some(Box::new(exclude)),
            "read-failure" => exclusions.fail_reads.store(true, Ordering::SeqCst),
            _ => unreachable!(),
        }
        leases.capacity.set_load("acct_scope_old", 0);
        let error = selected
            .await
            .err()
            .expect("updated exclusion prevents delivery");
        assert_eq!(
            error.kind(),
            if change == "read-failure" {
                ProviderErrorKind::ProviderInfrastructureUnavailable
            } else {
                ProviderErrorKind::NoEligibleAccount
            },
            "{change}"
        );
        assert_eq!(error.send_state(), UpstreamSendState::NotSent);
        assert!(server.received_requests().await.unwrap().is_empty());
        assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
        assert_eq!(
            leases.capacity.signals.lock().unwrap()
                [&ProviderAccountId::new("acct_scope_old").unwrap()]
                .in_flight,
            0
        );
        assert!(
            leases
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| request.account_id().as_str() == "acct_scope_old")
        );
    }
}
