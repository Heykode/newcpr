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
async fn native_catalog_membership_precedes_weight_affinity_pin_and_model_policy() {
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
            .expect(3)
            .mount(&server)
            .await;
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        let affinity = Arc::new(MemorySessionAffinity::default());
        let provider = super::scheduling::waiting_provider_with_affinity_and_limit(
            &store,
            leases.clone(),
            server.uri(),
            Arc::default(),
            affinity.clone(),
            NonZeroU32::MIN,
        );
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
                account_policy(),
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
            let error = provider
                .execute(
                    planned_request_with_model("openai", operation(model, false), model),
                    context,
                )
                .await
                .err()
                .expect("catalog and model policy must both allow account");
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
async fn unknown_plan_does_not_borrow_shared_catalog_entitlements() {
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
            .expect(1)
            .mount(&server)
            .await;
        let provider = provider(&store, Arc::default(), server.uri());
        provider.query_model_capabilities().await.unwrap();
        let mut stream = provider
            .execute(
                planned_request_with_model("openai", operation("gpt-future", false), "gpt-future"),
                attempt(wait),
            )
            .await
            .unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        server.verify().await;
    }
}
#[tokio::test]
async fn excel_exemption_follows_the_actual_route_not_the_account_setting() {
    for wait in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        store.set_responses_upstream("acct_scope_old", ResponsesUpstream::Excel);
        store.set_excel_encrypted_content_policy(
            "acct_scope_old",
            false,
            vec!["gpt-5.4".to_owned()],
        );
        store.set_scheduling("acct_scope_old", None, AccountWeight::new(100).unwrap());
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
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        let provider = provider(&store, leases.clone(), server.uri());
        provider.query_model_capabilities().await.unwrap();
        let error = provider
            .execute(
                planned_request_with_model("openai", operation("gpt-future", false), "gpt-future"),
                attempt(wait),
            )
            .await
            .err()
            .expect("native route still requires native catalog eligibility");
        assert_eq!(error.kind(), ProviderErrorKind::NoEligibleAccount);
        assert!(leases.requests.lock().unwrap().is_empty());
        let stream = provider
            .execute(
                planned_request_with_model("openai", operation("gpt-5.4", false), "gpt-5.4"),
                attempt(wait),
            )
            .await
            .expect("configured Excel route retains its own model contract");
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
    }
}

#[tokio::test]
async fn failed_native_catalog_cannot_turn_excel_synthetic_models_into_refusal_evidence() {
    for wait in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        store.set_responses_upstream("acct_scope_old", ResponsesUpstream::Excel);
        store.set_excel_encrypted_content_policy(
            "acct_scope_old",
            false,
            vec!["gpt-5.4".to_owned()],
        );
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
        let provider = provider(&store, Arc::default(), server.uri());
        provider.query_model_capabilities().await.unwrap();
        let mut stream = provider
            .execute(
                planned_request_with_model("openai", operation("gpt-future", false), "gpt-future"),
                attempt(wait),
            )
            .await
            .expect("missing native evidence permits upstream validation");
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        server.verify().await;
    }
}
