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
async fn unsupported_reasoning_value_preserves_account_eligibility_across_transports() {
    for transport in ["http", "sse", "websocket"] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let leases = Arc::new(TestLeaseCoordinator::default());
        let (provider, _, cooldowns) = super::scheduling::waiting_provider_with_catalog(
            &store,
            leases,
            format!("http://{}", listener.local_addr().unwrap()),
            Arc::default(),
            Arc::default(),
            NonZeroU32::MIN,
        );
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let error = json!({"code":"unsupported_value","type":"invalid_request_error",
                    "param":"reasoning.effort","message":"Unsupported value: none is not supported with this model."});
                if transport == "websocket" {
                    let mut socket = accept_codex_test_websocket(socket).await;
                    socket.next().await.unwrap().unwrap();
                    socket
                        .send(Message::Text(
                            json!({"type":"error","status":400,"error":error})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                } else {
                    read_http_request(&mut socket).await;
                    let (status, content_type, body) = if transport == "sse" {
                        (
                            "200 OK",
                            "text/event-stream",
                            format!(
                                "event: response.failed\ndata: {}\n\n",
                                json!({"type":"response.failed","response":{"id":"resp_bad_effort","status":"failed","output":[],"error":error}})
                            ),
                        )
                    } else {
                        (
                            "400 Bad Request",
                            "application/json",
                            json!({"error":error}).to_string(),
                        )
                    };
                    socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
            }
        });
        for _ in 0..2 {
            let error = timeout(Duration::from_secs(3), async {
                let mut stream = provider
                    .execute(
                        planned_request_with_model(
                            "openai",
                            operation("gpt-6-astra", transport == "websocket"),
                            "gpt-6-astra",
                        ),
                        attempt(false),
                    )
                    .await
                    .unwrap();
                while let Some(event) = stream.next().await {
                    if let Err(error) = event {
                        return error;
                    }
                }
                panic!("expected upstream request error");
            })
            .await
            .unwrap();
            assert_eq!(
                error.kind(),
                ProviderErrorKind::InvalidRequest,
                "{transport}"
            );
            assert!(!error.replay_is_safe());
        }
        assert!(cooldowns.cooldowns.lock().unwrap().is_empty());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn grok_never_acquires_native_oauth_in_either_selector_or_transport() {
    for wait in [false, true] {
        for excel_enabled in [false, true] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_scope_old").await;
            if excel_enabled {
                store.seed_excel_credential("acct_scope_old", true);
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
                    if excel_enabled {
                        if let Ok(stream) = result {
                            assert_eq!(stream.metadata().transport().as_str(), "excel_http_sse");
                            // BPS access validates its actual catalog before sending payloads.
                            drop(stream);
                        }
                    } else {
                        let error = result.err().expect("native OAuth cannot serve Grok");
                        assert_eq!(error.kind(), ProviderErrorKind::NoEligibleAccount);
                        assert_eq!(error.send_state(), UpstreamSendState::NotSent);
                    }
                }
            }
            if !excel_enabled {
                assert!(leases.requests.lock().unwrap().is_empty());
            }
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
async fn excel_switch_prepares_all_compatible_models_in_both_selectors() {
    for wait in [false, true] {
        for websocket in [false, true] {
            for model in ["gpt-5.4", "gpt-6-astra"] {
                let store = Arc::new(MemoryAccountStore::default());
                create_account(&store, "acct_scope_old").await;
                store.seed_excel_credential("acct_scope_old", true);
                store.set_responses_upstream("acct_scope_old", ResponsesUpstream::Excel);
                store.set_excel_encrypted_content_policy(
                    "acct_scope_old",
                    false,
                    vec!["gpt-5.4".to_owned()],
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
#[tokio::test]
async fn upstream_model_rejection_cools_only_that_model_across_requests_and_transports() {
    use gateway_core::provider_ports::{ProviderCooldownScope, ProviderScopedCooldown};
    let attempt = |wait| {
        context_with_policy(
            "req_model_refusal",
            CancellationToken::new(),
            contract_account_scope(),
            gateway_core::account::AccountSelectionPolicy::new(
                gateway_core::account::RotationStrategy::Smart,
                NonZeroU32::MIN,
                Duration::ZERO,
            ),
        )
        .with_request_tuning(RequestTuning {
            account_busy_wait_enabled: wait,
            ..Default::default()
        })
    };

    for (websocket, wait) in [(false, false), (false, true), (true, false), (true, true)] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        let (provider, _, cooldowns) = super::scheduling::waiting_provider_with_catalog(
            &store,
            leases,
            server.uri(),
            Arc::default(),
            Arc::default(),
            NonZeroU32::MIN,
        );
        Mock::given(method(if websocket { "GET" } else { "POST" }))
            .and(path("/codex/responses"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error":{
                "code":"model_not_found", "type":"invalid_request_error",
                "message":"The 'gpt-6-astra' model is not supported when using Codex with a ChatGPT account."
            }}))).mount(&server).await;
        let request_operation = if websocket {
            let payload = ProtocolPayload::json_object(
                "openai",
                Map::from_iter([
                    ("model".to_owned(), json!("gpt-6-astra")),
                    ("input".to_owned(), json!("hello")),
                ]),
            )
            .unwrap();
            Operation::Generate(GenerateRequest::from_protocol_payload(payload))
        } else {
            operation("gpt-6-astra", false)
        };
        let mut stream = provider
            .execute(
                planned_request_with_model("openai", request_operation, "gpt-6-astra"),
                attempt(wait),
            )
            .await
            .unwrap();
        let error = loop {
            match stream.next().await {
                Some(Err(error)) => break error,
                Some(Ok(_)) => {}
                None => panic!("expected model rejection"),
            }
        };
        assert_eq!(error.kind(), ProviderErrorKind::Unsupported);
        assert!(error.replay_is_safe());
        drop(stream);
        let blocked = provider
            .execute(
                planned_request_with_model(
                    "openai",
                    operation("gpt-6-astra", false),
                    "gpt-6-astra",
                ),
                attempt(wait),
            )
            .await
            .err()
            .expect("model cooldown must survive the first request");
        assert_eq!(blocked.kind(), ProviderErrorKind::NoEligibleAccount);
        assert!(cooldowns.cooldowns.lock().unwrap().is_empty());
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
            )
            .expect(3)
            .mount(&server)
            .await;
        let mut other = provider
            .execute(
                planned_request_with_model("openai", operation("gpt-6-luna", false), "gpt-6-luna"),
                attempt(wait),
            )
            .await
            .unwrap();
        while let Some(event) = other.next().await {
            event.unwrap();
        }
        drop(other);
        let account = store.account("acct_scope_old").unwrap();
        let scope =
            ProviderCooldownScope::upstream_model(UpstreamModelId::new("gpt-6-astra").unwrap());
        let key = (account.id().clone(), scope.clone());
        {
            let mut scoped = cooldowns.scoped.lock().unwrap();
            let until = scoped[&key].until();
            assert!(
                (Duration::from_secs(29 * 60)..=Duration::from_secs(30 * 60))
                    .contains(&until.duration_since(SystemTime::now()).unwrap())
            );
            scoped.insert(
                key,
                ProviderScopedCooldown::new(
                    account.id().clone(),
                    account.revision(),
                    scope,
                    SystemTime::now() - Duration::from_secs(1),
                ),
            );
        }
        let mut recovered = provider
            .execute(
                planned_request_with_model(
                    "openai",
                    operation("gpt-6-astra", false),
                    "gpt-6-astra",
                ),
                attempt(wait),
            )
            .await
            .unwrap();
        while let Some(event) = recovered.next().await {
            event.unwrap();
        }
        drop(recovered);

        use gateway_core::account::{
            CredentialCasOutcome, CredentialCasUpdate, ProviderAccountUpdate,
        };
        let loaded = store.load_current_credential(account.id()).await.unwrap();
        let update = CredentialCasUpdate::new(
            account.id().clone(),
            account.revision(),
            ProviderAccountUpdate {
                account_id: account.id().clone(),
                name: account.name().to_owned(),
                email: account.email().map(str::to_owned),
                plan_type: account.plan_type().map(str::to_owned),
            },
            loaded.credential,
            false,
            None,
            None,
        )
        .unwrap();
        assert!(matches!(
            store.compare_and_swap_credential(update).await.unwrap(),
            CredentialCasOutcome::Updated(_)
        ));
        let scope =
            ProviderCooldownScope::upstream_model(UpstreamModelId::new("gpt-6-astra").unwrap());
        cooldowns.scoped.lock().unwrap().insert(
            (account.id().clone(), scope.clone()),
            ProviderScopedCooldown::new(
                account.id().clone(),
                account.revision(),
                scope,
                SystemTime::now() + Duration::from_secs(1800),
            ),
        );
        let mut rotated = provider
            .execute(
                planned_request_with_model(
                    "openai",
                    operation("gpt-6-astra", false),
                    "gpt-6-astra",
                ),
                attempt(wait),
            )
            .await
            .expect("old credential refusal does not affect new credentials");
        while let Some(event) = rotated.next().await {
            event.unwrap();
        }
    }
}

#[tokio::test]
async fn native_catalog_membership_precedes_weight_and_pin_but_explicit_allowlist_can_extend_it() {
    use gateway_core::account::{AccountModelAccess, AccountModelAccessMode};
    for wait in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        let server = MockServer::start().await;
        for (id, plan, models) in [
            ("acct_scope_old", "free", vec!["gpt-6-luna"]),
            ("acct_scope_new", "pro", vec!["gpt-6-luna", "gpt-6-astra"]),
        ] {
            let mut identity = profile(&format!("chatgpt-{id}"));
            identity.plan_type = Some(plan.to_owned());
            store
                .seed_oauth_credential(ImportCodexOAuthCredential {
                    account_id: id.to_owned(),
                    name: plan.to_owned(),
                    secret: secret("test-access"),
                    verified_account: identity,
                    next_refresh_at: Some(Utc::now() + chrono::Duration::minutes(30)),
                    enabled: true,
                })
                .await;
            Mock::given(method("GET")).and(path("/codex/models"))
                .and(header("chatgpt-account-id", format!("chatgpt-{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "models":models.iter().map(|id| json!({"slug":id,"display_name":id})).collect::<Vec<_>>()
                }))).expect(1).mount(&server).await;
        }
        store.set_scheduling("acct_scope_old", None, AccountWeight::new(100).unwrap());
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
            )
            .expect(4)
            .mount(&server)
            .await;
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        let affinity = Arc::new(MemorySessionAffinity::default());
        let (provider, catalog, _) = super::scheduling::waiting_provider_with_catalog(
            &store,
            leases.clone(),
            server.uri(),
            Arc::default(),
            affinity.clone(),
            NonZeroU32::MIN,
        );
        catalog.refresh_catalogs().await.unwrap();
        provider.query_model_capabilities().await.unwrap();
        for (model, expected) in [
            ("gpt-6-luna", "acct_scope_old"),
            ("gpt-6-astra", "acct_scope_new"),
        ] {
            let payload = ProtocolPayload::json_object(
                "openai",
                json!({
                    "model":model,"input":"hello",
                    "client_metadata":{"x-codex-turn-metadata":json!({
                        "session_id":"catalog-session","thread_id":"catalog-session"
                    }).to_string()}
                })
                .as_object()
                .unwrap()
                .clone(),
            )
            .unwrap()
            .with_context(Map::from_iter([("use_websocket".to_owned(), json!(false))]));
            let mut stream = provider
                .execute(
                    planned_request_with_model(
                        "openai",
                        Operation::Generate(GenerateRequest::from_protocol_payload(payload)),
                        model,
                    ),
                    attempt(wait),
                )
                .await
                .unwrap();
            assert_eq!(stream.metadata().provider_account_id().as_str(), expected);
            while let Some(event) = stream.next().await {
                event.unwrap();
            }
        }
        assert_eq!(affinity.binding_count(), 1);
        for (model, required, deny_supported) in [
            ("gpt-6-astra", Some("acct_scope_old"), false),
            ("custom-unlisted", None, false),
            ("gpt-6-astra", None, true),
        ] {
            let scope = if deny_supported {
                Arc::new(FrozenAccountScope::new(
                    Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
                        (
                            ProviderAccountId::new("acct_scope_old").unwrap(),
                            RuntimeAccount::new(
                                ProviderKind::new("openai").unwrap(),
                                BTreeSet::new(),
                            )
                            .with_model_access(
                                AccountModelAccess::new(
                                    AccountModelAccessMode::Allowlist,
                                    vec![model.to_owned()],
                                )
                                .unwrap(),
                            ),
                        ),
                        (
                            ProviderAccountId::new("acct_scope_new").unwrap(),
                            RuntimeAccount::new(
                                ProviderKind::new("openai").unwrap(),
                                BTreeSet::new(),
                            )
                            .with_model_access(
                                AccountModelAccess::new(
                                    AccountModelAccessMode::Denylist,
                                    vec![model.to_owned()],
                                )
                                .unwrap(),
                            ),
                        ),
                    ]))),
                    ClientRoutingScope::all_accounts(),
                ))
            } else {
                contract_account_scope()
            };
            let context = AttemptContext::new(
                RequestAttemptContext::new(
                    ModelRequestId::new("req_catalog_reject").unwrap(),
                    ClientApiKeyId::new("key_openai_contract").unwrap(),
                ),
                NonZeroU32::MIN,
                SystemTime::now() + Duration::from_secs(30),
                // These independent eligibility assertions do not test pacing.
                gateway_core::account::AccountSelectionPolicy::new(
                    gateway_core::account::RotationStrategy::Smart,
                    NonZeroU32::MIN,
                    Duration::ZERO,
                ),
                AccountAttemptContext::new(
                    BTreeSet::new(),
                    required.map(|id| ProviderAccountId::new(id).unwrap()),
                    None,
                )
                .with_account_scope(scope),
                None,
                CancellationToken::new(),
            )
            .with_request_tuning(RequestTuning {
                account_busy_wait_enabled: wait,
                ..Default::default()
            });
            let before = leases.requests.lock().unwrap().len();
            let selected = provider
                .execute(
                    planned_request_with_model("openai", operation(model, false), model),
                    context,
                )
                .await;
            if deny_supported {
                let mut stream = selected.unwrap_or_else(|error| {
                    panic!("explicit allowlist extends the missing catalog entry (wait={wait}): {error:?}")
                });
                assert_eq!(
                    stream.metadata().provider_account_id().as_str(),
                    "acct_scope_old"
                );
                while let Some(event) = stream.next().await {
                    event.unwrap();
                }
                continue;
            }
            let error = selected
                .err()
                .expect("catalog rejects models without an explicit allowlist");
            assert_eq!(error.kind(), ProviderErrorKind::NoEligibleAccount);
            assert_eq!(leases.requests.lock().unwrap().len(), before);
        }
        let before = leases.requests.lock().unwrap().len();
        let pinned = pinned_continuation_context(
            "req_catalog_native_owner",
            "acct_scope_old",
            "client-previous",
            "upstream-previous",
            1,
            ContinuationAttempt::Native,
        )
        .with_request_tuning(RequestTuning {
            account_busy_wait_enabled: wait,
            ..Default::default()
        });
        let error = provider
            .execute(
                planned_request_with_model(
                    "openai",
                    operation("gpt-6-astra", false),
                    "gpt-6-astra",
                ),
                pinned,
            )
            .await
            .err()
            .expect("ineligible native owner cannot silently switch accounts");
        assert_eq!(error.kind(), ProviderErrorKind::NoEligibleAccount);
        assert_eq!(leases.requests.lock().unwrap().len(), before);
        // Check the diagnostic catalog exception independently from fixture request pacing.
        for signal in leases.capacity.signals.lock().unwrap().values_mut() {
            assert_eq!(signal.in_flight, 0);
            signal.last_started_at = None;
        }
        let mut diagnostic = provider
            .execute(
                planned_request_with_model(
                    "openai",
                    operation("gpt-6-astra", false),
                    "gpt-6-astra",
                ),
                diagnostic_context("req_catalog_diagnostic", "acct_scope_old"),
            )
            .await
            .unwrap();
        assert_eq!(
            diagnostic.metadata().provider_account_id().as_str(),
            "acct_scope_old"
        );
        while let Some(event) = diagnostic.next().await {
            event.unwrap();
        }
        server.verify().await;
    }
}

#[tokio::test]
async fn unknown_plan_uses_its_own_catalog_entitlements() {
    for wait in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        let mut identity = profile("unknown-plan");
        identity.plan_type = None;
        store
            .seed_oauth_credential(ImportCodexOAuthCredential {
                account_id: "acct_scope_old".to_owned(),
                name: "unknown".to_owned(),
                secret: secret("test-access"),
                verified_account: identity,
                next_refresh_at: Some(Utc::now() + chrono::Duration::minutes(30)),
                enabled: true,
            })
            .await;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"models":[{"slug":"other-model","display_name":"Other"}]}),
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
            )
            .expect(0)
            .mount(&server)
            .await;
        let (provider, catalog, _) = super::scheduling::waiting_provider_with_catalog(
            &store,
            Arc::default(),
            server.uri(),
            Arc::default(),
            Arc::default(),
            NonZeroU32::MIN,
        );
        catalog.refresh_catalogs().await.unwrap();
        provider.query_model_capabilities().await.unwrap();
        let error = provider
            .execute(
                planned_request_with_model("openai", operation("gpt-future", false), "gpt-future"),
                attempt(wait),
            )
            .await
            .err()
            .expect("own catalog excludes the unknown model even without a plan");
        assert_eq!(error.kind(), ProviderErrorKind::NoEligibleAccount);
        server.verify().await;
    }
}
#[tokio::test]
async fn excel_catalog_uses_bps_access_and_legacy_model_subset_never_routes_to_codex() {
    for wait in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        store.seed_excel_credential("acct_scope_old", true);
        store.set_responses_upstream("acct_scope_old", ResponsesUpstream::Excel);
        store.set_excel_encrypted_content_policy(
            "acct_scope_old",
            false,
            vec!["gpt-5.4".to_owned()],
        );
        store.set_scheduling("acct_scope_old", None, AccountWeight::new(100).unwrap());
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/basispoints/api/responses/access"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"allowed":true,"model_catalog":{"models":[
                        {"id":"gpt-5.4","label":"GPT 5.4","model_picker_enabled":true,"policy":{"state":"enabled"},"efforts":[]}
                    ],"restricted_models":[]}}),
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        let (provider, catalog, _) = super::scheduling::waiting_provider_with_catalog(
            &store,
            leases.clone(),
            server.uri(),
            Arc::default(),
            Arc::default(),
            NonZeroU32::MIN,
        );
        catalog.refresh_catalogs().await.unwrap();
        provider.query_model_capabilities().await.unwrap();
        let stream = provider
            .execute(
                planned_request_with_model("openai", operation("gpt-future", false), "gpt-future"),
                attempt(wait),
            )
            .await
            .expect("models outside the legacy subset retain the Excel route");
        assert_eq!(stream.metadata().transport().as_str(), "excel_http_sse");
        // Live access is revalidated when the lazy stream is polled.
        drop(stream);
        for signal in leases.capacity.signals.lock().unwrap().values_mut() {
            signal.last_started_at = None;
        }
        let stream = provider
            .execute(
                planned_request_with_model("openai", operation("gpt-5.4", false), "gpt-5.4"),
                attempt(wait),
            )
            .await
            .expect("Excel route retains its own model contract");
        assert_eq!(stream.metadata().transport().as_str(), "excel_http_sse");
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_scope_old"
        );
        drop(stream);
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.method == "GET")
        );
        server.verify().await;
    }
}

#[tokio::test]
async fn failed_excel_catalog_never_falls_back_to_native_requests() {
    for wait in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        store.seed_excel_credential("acct_scope_old", true);
        store.set_responses_upstream("acct_scope_old", ResponsesUpstream::Excel);
        store.set_excel_encrypted_content_policy(
            "acct_scope_old",
            false,
            vec!["gpt-5.4".to_owned()],
        );
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/basispoints/api/responses/access"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
            )
            .expect(0)
            .mount(&server)
            .await;
        let (provider, catalog, _) = super::scheduling::waiting_provider_with_catalog(
            &store,
            Arc::default(),
            server.uri(),
            Arc::default(),
            Arc::default(),
            NonZeroU32::MIN,
        );
        assert!(catalog.refresh_catalogs().await.is_err());
        let stream = provider
            .execute(
                planned_request_with_model("openai", operation("gpt-future", false), "gpt-future"),
                attempt(wait),
            )
            .await
            .expect("catalog failure retains the Excel route for live validation");
        assert_eq!(stream.metadata().transport().as_str(), "excel_http_sse");
        drop(stream);
        server.verify().await;
    }
}

#[tokio::test]
async fn account_allowlists_override_catalog_omissions_but_weight_affinity_and_pinning_do_not() {
    use gateway_core::account::{
        AccountModelAccess,
        AccountModelAccessMode::{All, Allowlist, Denylist},
    };

    let native = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    for (id, plan, models) in [
        (
            "acct_provider_contract",
            "pro",
            vec!["gpt-6-luna", "gpt-6-astra"],
        ),
        ("acct_scope_new", "free", vec!["gpt-6-luna"]),
    ] {
        {
            let mut identity = profile(&format!("chatgpt-{id}"));
            identity.plan_type = Some(plan.to_owned());
            store
                .seed_oauth_credential(ImportCodexOAuthCredential {
                    account_id: id.to_owned(),
                    name: plan.to_owned(),
                    secret: secret("test-access"),
                    verified_account: identity,
                    next_refresh_at: Some(Utc::now() + chrono::Duration::minutes(30)),
                    enabled: true,
                })
                .await;
            Mock::given(method("GET"))
                .and(path("/codex/models"))
                .and(header("chatgpt-account-id", format!("chatgpt-{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "models": models.iter().map(|id| json!({"slug": id, "display_name": id})).collect::<Vec<_>>()
                })))
                .expect(1)
                .mount(&native).await;
        }
    }
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
        )
        .mount(&native)
        .await;
    store.set_scheduling("acct_scope_new", None, AccountWeight::new(100).unwrap());
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    let (provider, _, catalog) = provider_services_with_profile(
        &store,
        affinity.clone(),
        native.uri(),
        leases.clone(),
        u32::try_from(DEFAULT_STREAM_MAX_RETRIES).unwrap(),
        wire_profile(),
    );
    catalog.refresh_catalogs().await.unwrap();
    provider.query_model_capabilities().await.unwrap();
    // 待刷新期间继续以该账号最近成功目录过滤，权重和亲和不能覆盖资格。
    assert!(
        catalog
            .observe_response_etag(&store.account("acct_scope_new").unwrap(), "new")
            .unwrap()
    );

    for (model, expected) in [
        ("gpt-6-luna", "acct_scope_new"),
        ("gpt-6-astra", "acct_provider_contract"),
    ] {
        let request_operation = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object(
                "openai",
                json!({
                    "model": model, "input": "hello",
                    "client_metadata": {"x-codex-turn-metadata": json!({
                        "session_id":"model-session", "thread_id":"model-session"
                    }).to_string()}
                })
                .as_object()
                .unwrap()
                .clone(),
            )
            .unwrap()
            .with_context(Map::from_iter([("use_websocket".to_owned(), json!(false))])),
        ));
        let mut stream = provider
            .execute(
                planned_request_with_model("openai", request_operation, model),
                context("req_catalog_model", CancellationToken::new()),
            )
            .await
            .unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            expected,
            "{model}: native OAuth account"
        );
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
    }
    assert_eq!(affinity.binding_count(), 1);
    let primary = "acct_provider_contract";
    let secondary = "acct_scope_new";
    // 目录外许可只属于明确添加该模型的账号，不能借用同池账号的白名单
    for (model, required, primary_mode, secondary_mode, expected) in [
        ("gpt-6-astra", Some(secondary), All, All, None),
        ("custom-unlisted", None, All, All, None),
        ("gpt-6.1-sol", None, Allowlist, All, Some(primary)),
        ("gpt-6-astra", None, Denylist, Allowlist, Some(secondary)),
        (
            "custom-unlisted",
            None,
            Denylist,
            Allowlist,
            Some(secondary),
        ),
        ("gpt-6-luna", Some(primary), Denylist, Allowlist, None),
    ] {
        let accounts = [(primary, primary_mode), (secondary, secondary_mode)]
            .into_iter()
            .map(|(id, mode)| {
                let access = if mode == All {
                    AccountModelAccess::all()
                } else {
                    AccountModelAccess::new(mode, vec![model.to_owned()]).unwrap()
                };
                (
                    ProviderAccountId::new(id).unwrap(),
                    RuntimeAccount::new(ProviderKind::new("openai").unwrap(), BTreeSet::new())
                        .with_model_access(access),
                )
            })
            .collect();
        let scope = Arc::new(FrozenAccountScope::new(
            Arc::new(RuntimeAccountDirectory::new(accounts)),
            ClientRoutingScope::all_accounts(),
        ));
        let attempt = AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_account_catalog_policy").unwrap(),
                ClientApiKeyId::new("key_openai_contract").unwrap(),
            ),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(30),
            account_policy(),
            AccountAttemptContext::new(
                BTreeSet::new(),
                required.map(|id| ProviderAccountId::new(id).unwrap()),
                None,
            )
            .with_account_scope(scope),
            None,
            CancellationToken::new(),
        );
        let acquisitions = leases.requests.lock().unwrap().len();
        let result = provider
            .execute(
                planned_request_with_model("openai", operation(model, false), model),
                attempt,
            )
            .await;
        if let Some(expected) = expected {
            let mut stream = result.unwrap();
            assert_eq!(stream.metadata().provider_account_id().as_str(), expected);
            while let Some(event) = stream.next().await {
                event.unwrap();
            }
        } else {
            let error = result.err().expect("account must be excluded");
            assert_eq!(error.kind(), ProviderErrorKind::NoEligibleAccount);
            assert_eq!(leases.requests.lock().unwrap().len(), acquisitions);
        }
    }
    let sent: Vec<_> = native
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method == "POST")
        .map(|request| {
            captured_request_body(&request)["model"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(sent.len(), 5);
    assert!(sent.iter().any(|model| model == "gpt-6.1-sol"));
    assert!(sent.iter().any(|model| model == "custom-unlisted"));
}

#[tokio::test]
async fn expired_native_catalog_keeps_exclusions_until_a_successful_refresh() {
    use gateway_core::account::AccountCatalogObservation;
    for wait in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        let account = store.account("acct_scope_old").unwrap();
        let document = [
            &[0_u8][..],
            br#"{"models":[{"slug":"other-model","display_name":"Other"}]}"#,
        ]
        .concat();
        store
            .save_account_catalog(
                &AccountCatalogObservation {
                    account_id: account.id().clone(),
                    revision: account.revision(),
                    plan: account.plan_type().map(str::to_owned),
                    observed_at: SystemTime::now() - Duration::from_secs(3600),
                    etag: None,
                },
                &document,
            )
            .await
            .unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let (provider, catalog, _) = super::scheduling::waiting_provider_with_catalog(
            &store,
            Arc::default(),
            server.uri(),
            Arc::default(),
            Arc::default(),
            NonZeroU32::MIN,
        );
        catalog.restore().await.unwrap();
        // A failed periodic refresh returns the last successful snapshot.
        catalog.refresh_catalogs().await.unwrap();
        provider.query_model_capabilities().await.unwrap();
        let execute = || {
            provider.execute(
                planned_request_with_model("openai", operation("gpt-future", false), "gpt-future"),
                attempt(wait),
            )
        };
        assert_eq!(
            execute().await.err().unwrap().kind(),
            ProviderErrorKind::NoEligibleAccount
        );
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"models":[{"slug":"gpt-future","display_name":"Future"}]}),
                ),
            )
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        catalog.refresh_catalogs().await.unwrap();
        let mut stream = execute().await.unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_scope_old"
        );
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        server.verify().await;
    }
}
