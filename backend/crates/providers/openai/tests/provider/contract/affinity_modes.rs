use super::*;
use gateway_core::account::AccountAffinity;
use gateway_core::provider_ports::{ProviderSessionAffinityKey, ProviderSessionAffinityPort};

fn mode_context(mode: AccountAffinity) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_affinity_mode").unwrap(),
            ClientApiKeyId::new("key_openai_contract").unwrap(),
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(30),
        account_policy().with_openai_account_affinity(mode),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope()),
        None,
        CancellationToken::new(),
    )
}

fn native_context(mode: AccountAffinity) -> AttemptContext {
    let account = ProviderAccountId::new("acct_subagent_b").unwrap();
    let provider_kind = ProviderKind::new("openai").unwrap();
    let client_key = ClientApiKeyId::new("key_openai_contract").unwrap();
    let owner = ProviderAccountStateOwner::new(provider_kind.clone(), account.clone());
    let pin = NativeContinuationPin::new(
        PreviousResponseId::new("resp_client"),
        PreviousResponseId::new("resp_upstream"),
        client_key.clone(),
        provider_kind,
        account,
    );
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_native_relaxed").unwrap(),
            client_key,
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(30),
        account_policy().with_openai_account_affinity(mode),
        AccountAttemptContext::new(BTreeSet::new(), None, Some(owner))
            .with_account_scope(contract_account_scope()),
        Some(ContinuationBinding::Pinned(pin)),
        CancellationToken::new(),
    )
    .with_continuation_attempt(ContinuationAttempt::Native)
}

fn generate(root: &str, child: Option<&str>, turn: Option<&str>) -> ProviderRequest {
    let generation = generate_with_session_context(root, child, None);
    let mut metadata = Map::from_iter([("use_websocket".to_owned(), json!(false))]);
    if let Some(turn) = turn {
        metadata.insert("turn_id".to_owned(), json!(turn));
    }
    planned_request(
        "openai",
        Operation::Generate(GenerateRequest::from_protocol_payload(
            generation.protocol_payload().clone().with_context(metadata),
        )),
    )
}

async fn fixture() -> (
    CodexProvider,
    Arc<MemorySessionAffinity>,
    Arc<TestLeaseCoordinator>,
    MockServer,
) {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    let upstream = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        affinity.clone(),
        upstream.uri(),
        leases.clone(),
    );
    let root = provider
        .execute(
            generate("root", None, None),
            mode_context(AccountAffinity::Strict),
        )
        .await
        .unwrap();
    assert_eq!(
        root.metadata().provider_account_id().as_str(),
        "acct_subagent_a"
    );
    drop(root);
    create_account(&store, "acct_subagent_b").await;
    store.set_scheduling("acct_subagent_b", None, AccountWeight::new(100).unwrap());
    (provider, affinity, leases, upstream)
}

#[tokio::test]
async fn relaxed_child_prefers_root_but_can_escape_busy_root_without_migrating_it() {
    for busy in [false, true] {
        let (provider, affinity, leases, _upstream) = fixture().await;
        if busy {
            leases
                .busy_accounts
                .lock()
                .unwrap()
                .insert(ProviderAccountId::new("acct_subagent_a").unwrap());
        }
        let stream = provider
            .execute(
                generate("root", Some("child"), None),
                mode_context(AccountAffinity::Relaxed),
            )
            .await
            .unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            if busy {
                "acct_subagent_b"
            } else {
                "acct_subagent_a"
            }
        );
        drop(stream);
        assert_eq!(
            affinity.binding_count(),
            2,
            "child owns an independent binding"
        );
        leases.busy_accounts.lock().unwrap().clear();
        let root = provider
            .execute(
                generate("root", None, None),
                mode_context(AccountAffinity::Relaxed),
            )
            .await
            .unwrap();
        assert_eq!(
            root.metadata().provider_account_id().as_str(),
            "acct_subagent_a"
        );
    }
}

#[tokio::test]
async fn relaxed_child_can_use_an_eligible_model_account_without_moving_the_root() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        affinity.clone(),
        server.uri(),
        Arc::new(TestLeaseCoordinator::default()),
    );
    drop(
        provider
            .execute(
                generate("root", None, None),
                mode_context(AccountAffinity::Strict),
            )
            .await
            .unwrap(),
    );
    let root_key = affinity.lookup_keys()[0].clone();
    let mut verified_account = profile("chatgpt-acct_subagent_b");
    verified_account.plan_type = Some("free".to_owned());
    store
        .seed_oauth_credential(ImportCodexOAuthCredential {
            account_id: "acct_subagent_b".to_owned(),
            name: "test model account".to_owned(),
            secret: secret("at-acct_subagent_b"),
            verified_account,
            next_refresh_at: Some(Utc::now() + chrono::Duration::minutes(30)),
            enabled: true,
        })
        .await;
    let scope = Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
            (
                ProviderAccountId::new("acct_subagent_a").unwrap(),
                RuntimeAccount::new(ProviderKind::new("openai").unwrap(), BTreeSet::new())
                    .with_model_access(
                        gateway_core::account::AccountModelAccess::new(
                            gateway_core::account::AccountModelAccessMode::Denylist,
                            vec!["gpt-5.4".to_owned()],
                        )
                        .unwrap(),
                    ),
            ),
            (
                ProviderAccountId::new("acct_subagent_b").unwrap(),
                RuntimeAccount::new(ProviderKind::new("openai").unwrap(), BTreeSet::new()),
            ),
        ]))),
        ClientRoutingScope::all_accounts(),
    ));
    let scoped = |mode| {
        context_with_policy(
            "req_relaxed_model_access",
            CancellationToken::new(),
            scope.clone(),
            account_policy().with_openai_account_affinity(mode),
        )
    };
    let strict = provider
        .execute(
            generate("root", Some("strict-child"), None),
            scoped(AccountAffinity::Strict),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(strict.kind(), ProviderErrorKind::NoEligibleAccount);
    let child = provider
        .execute(
            generate("root", Some("child"), None),
            scoped(AccountAffinity::Relaxed),
        )
        .await
        .unwrap();
    assert_eq!(
        child.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
    let bound = affinity
        .load(
            &ProviderKind::new("openai").unwrap(),
            &ProviderSessionAffinityKey::try_new(root_key).unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bound.as_str(), "acct_subagent_a");
    assert_eq!(affinity.binding_count(), 2);
    server.verify().await;
}

#[tokio::test]
async fn relaxed_child_turn_alias_survives_both_mode_changes_without_overwriting_its_target() {
    let (provider, affinity, leases, _upstream) = fixture().await;
    leases
        .busy_accounts
        .lock()
        .unwrap()
        .insert(ProviderAccountId::new("acct_subagent_a").unwrap());
    let child = provider
        .execute(
            generate("root", Some("child"), Some("turn-mode")),
            mode_context(AccountAffinity::Relaxed),
        )
        .await
        .unwrap();
    assert_eq!(
        child.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
    drop(child);
    leases.busy_accounts.lock().unwrap().clear();
    for (mode, expected) in [
        (AccountAffinity::Strict, "acct_subagent_a"),
        (AccountAffinity::Relaxed, "acct_subagent_b"),
    ] {
        for body in [
            json!({"prompt":"square"}),
            json!({"prompt":"square", "session_id":"root"}),
            json!({"prompt":"square", "session_id":"root", "thread_id":"child"}),
        ] {
            let image = Operation::GenerateImage(ImageRequest::from_raw_json(
                ImageRequestKind::Generation,
                RawJsonPayload::new("openai", Bytes::from(body.to_string()))
                    .unwrap()
                    .with_context(Map::from_iter([(
                        "image_turn_id".to_owned(),
                        json!("turn-mode"),
                    )])),
            ));
            let stream = provider
                .execute(
                    planned_provider_endpoint_request("openai", image),
                    mode_context(mode),
                )
                .await
                .unwrap();
            assert_eq!(stream.metadata().provider_account_id().as_str(), expected);
            drop(stream);
        }
    }
    assert_eq!(affinity.alias_count(), 1);
    assert_eq!(affinity.binding_count(), 2);
    let error = provider
        .execute(
            generate("another-root", Some("child"), Some("turn-mode")),
            mode_context(AccountAffinity::Relaxed),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
    let error = provider
        .execute(
            generate("root", Some("other-child"), Some("turn-mode")),
            mode_context(AccountAffinity::Relaxed),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
}

#[tokio::test]
async fn legacy_strict_child_turn_stays_on_root_in_relaxed_mode() {
    let (provider, affinity, leases, upstream) = fixture().await;
    drop(
        provider
            .execute(
                generate("root", Some("child"), Some("turn-legacy")),
                mode_context(AccountAffinity::Strict),
            )
            .await
            .unwrap(),
    );
    let stream = provider
        .execute(
            generate("root", Some("child"), Some("turn-legacy")),
            mode_context(AccountAffinity::Relaxed),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_a"
    );
    drop(stream);
    assert_eq!(affinity.binding_count(), 1);
    leases
        .busy_accounts
        .lock()
        .unwrap()
        .insert(ProviderAccountId::new("acct_subagent_a").unwrap());
    let error = provider
        .execute(
            generate("root", Some("child"), Some("turn-legacy")),
            mode_context(AccountAffinity::Relaxed),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), ProviderErrorKind::AccountCapacityUnavailable);
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn strict_mode_child_alias_cannot_recreate_a_lost_root() {
    let (provider, affinity, _leases, upstream) = fixture().await;
    let root = ProviderSessionAffinityKey::try_new(affinity.lookup_keys()[0].clone()).unwrap();
    drop(
        provider
            .execute(
                generate("root", Some("child"), Some("turn-lost-root")),
                mode_context(AccountAffinity::Relaxed),
            )
            .await
            .unwrap(),
    );
    affinity
        .clear(&ProviderKind::new("openai").unwrap(), &root)
        .await
        .unwrap();
    let error = provider
        .execute(
            generate("root", Some("child"), Some("turn-lost-root")),
            mode_context(AccountAffinity::Strict),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), ProviderErrorKind::NoEligibleAccount);
    assert_eq!(
        affinity.binding_count(),
        1,
        "child binding survives, root is not recreated"
    );
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn relaxed_search_and_compact_keep_the_independent_child_binding() {
    let (provider, affinity, leases, _upstream) = fixture().await;
    leases
        .busy_accounts
        .lock()
        .unwrap()
        .insert(ProviderAccountId::new("acct_subagent_a").unwrap());
    drop(
        provider
            .execute(
                generate("root", Some("child"), None),
                mode_context(AccountAffinity::Relaxed),
            )
            .await
            .unwrap(),
    );
    leases.busy_accounts.lock().unwrap().clear();
    let search = Operation::Search(StandaloneSearchRequest::from_raw_json(
        RawJsonPayload::new(
            "openai",
            Bytes::from_static(br#"{"id":"root","thread_id":"child","query":"test"}"#),
        )
        .unwrap(),
    ));
    let stream = provider
        .execute(
            planned_provider_endpoint_request("openai", search),
            mode_context(AccountAffinity::Relaxed),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
    drop(stream);
    let compact = Operation::Compact(gateway_core::operation::CompactRequest::from_raw_json(
        RawJsonPayload::new(
            "openai",
            Bytes::from_static(
                br#"{"model":"gpt-5.4","session_id":"root","thread_id":"child","input":[]}"#,
            ),
        )
        .unwrap(),
    ));
    let stream = provider
        .execute(
            planned_request("openai", compact),
            mode_context(AccountAffinity::Relaxed),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
    drop(stream);
    assert_eq!(affinity.binding_count(), 2);
}

#[tokio::test]
async fn relaxed_restored_owner_still_overrides_root_and_turn_alias() {
    let (provider, _affinity, _leases, _upstream) = fixture().await;
    drop(
        provider
            .execute(
                generate("root", Some("child"), Some("turn-restored")),
                mode_context(AccountAffinity::Strict),
            )
            .await
            .unwrap(),
    );
    let generation =
        generate_with_persisted_session_context("acct_subagent_b", "lc_restored", "root", "child");
    let payload = generation
        .protocol_payload()
        .clone()
        .with_context(Map::from_iter([
            ("turn_id".to_owned(), json!("turn-restored")),
            ("use_websocket".to_owned(), json!(false)),
        ]));
    let restored = GenerateRequest::from_protocol_payload(payload)
        .with_provider_session_state(generation.provider_session_state("openai").unwrap().clone());
    let stream = provider
        .execute(
            planned_request("openai", Operation::Generate(restored)),
            native_context(AccountAffinity::Relaxed),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
}

#[tokio::test]
async fn relaxed_native_owner_still_wins_over_a_strict_child_turn() {
    let (provider, _affinity, _leases, _upstream) = fixture().await;
    drop(
        provider
            .execute(
                generate("root", Some("child"), Some("turn-native")),
                mode_context(AccountAffinity::Strict),
            )
            .await
            .unwrap(),
    );
    let attempt = native_context(AccountAffinity::Relaxed);
    let stream = provider
        .execute(
            generate("root", Some("child"), Some("turn-native")),
            attempt,
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
}

#[tokio::test]
async fn live_turn_uses_relaxed_child_or_strict_root_without_rebinding() {
    use gateway_core::operation::{
        ProviderHttpHeader, ProviderHttpMethod, ProviderHttpRequest, RawHttpPayload,
    };
    let (provider, affinity, leases, _upstream) = fixture().await;
    leases
        .busy_accounts
        .lock()
        .unwrap()
        .insert(ProviderAccountId::new("acct_subagent_a").unwrap());
    drop(
        provider
            .execute(
                generate("root", Some("child"), Some("turn-live")),
                mode_context(AccountAffinity::Relaxed),
            )
            .await
            .unwrap(),
    );
    leases.busy_accounts.lock().unwrap().clear();
    for (mode, expected) in [
        (AccountAffinity::Relaxed, "acct_subagent_b"),
        (AccountAffinity::Strict, "acct_subagent_a"),
    ] {
        let operation = Operation::ProviderHttp(
            ProviderHttpRequest::new(
                "realtime-calls",
                ProviderHttpMethod::Post,
                None,
                vec![ProviderHttpHeader::new(
                    "x-client-turn-id",
                    Bytes::from_static(b"turn-live"),
                )],
                RawHttpPayload::new(
                    "openai",
                    Bytes::from_static(br#"{"sdp":"v=0","session":{"model":"gpt-live-1-codex"}}"#),
                )
                .unwrap(),
            )
            .unwrap(),
        );
        let provider_kind = ProviderKind::new("openai").unwrap();
        let snapshot = RuntimeSnapshot::new(
            ConfigRevision::new(1).unwrap(),
            account_policy(),
            vec![provider_kind.clone()],
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let plan = snapshot
            .plan_provider_endpoint(
                &provider_kind,
                Some(&UpstreamModelId::new("gpt-live-1-codex").unwrap()),
                &operation,
                contract_account_scope(),
                &RoutingContext::default(),
            )
            .unwrap();
        let stream = provider
            .execute(
                ProviderRequest::new(operation, plan.candidates()[0].clone()),
                mode_context(mode),
            )
            .await
            .unwrap();
        assert_eq!(stream.metadata().provider_account_id().as_str(), expected);
        drop(stream);
    }
    assert_eq!(affinity.binding_count(), 2);
}

#[tokio::test]
async fn relaxed_capacity_wait_keeps_qx_sticky_first_then_fallback_on_full_queue() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    let upstream = MockServer::start().await;
    let provider = super::scheduling::waiting_provider_with_affinity_and_limit(
        &store,
        leases.clone(),
        upstream.uri(),
        Arc::new(MemorySessionExclusions::default()),
        affinity.clone(),
        NonZeroU32::MIN,
    );
    drop(
        provider
            .execute(
                generate("root", None, None),
                mode_context(AccountAffinity::Strict),
            )
            .await
            .unwrap(),
    );
    create_account(&store, "acct_scope_new").await;
    leases.capacity.set_load("acct_scope_old", 1);
    leases.capacity.set_load("acct_scope_new", 0);
    // Exercise capacity saturation, not the seed request's interval cooldown.
    leases
        .capacity
        .signals
        .lock()
        .unwrap()
        .get_mut(&ProviderAccountId::new("acct_scope_old").unwrap())
        .unwrap()
        .last_started_at = None;
    leases.capacity.full.store(true, Ordering::SeqCst);
    let attempt = mode_context(AccountAffinity::Relaxed).with_request_tuning(
        gateway_core::routing::RequestTuning {
            account_busy_wait_enabled: true,
            ..Default::default()
        },
    );
    let stream = provider
        .execute(generate("root", Some("child"), None), attempt)
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_scope_new"
    );
    drop(stream);
    assert_eq!(leases.capacity.waits.lock().unwrap().len(), 1);
    assert_eq!(
        leases.capacity.waits.lock().unwrap()[0].mode(),
        gateway_core::engine::AccountWaitMode::Sticky
    );
    assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
    assert_eq!(affinity.binding_count(), 2);
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn relaxed_compact_forwarding_preserves_business_body_and_rebuilds_account_identity() {
    let (provider, _affinity, leases, upstream) = fixture().await;
    leases
        .busy_accounts
        .lock()
        .unwrap()
        .insert(ProviderAccountId::new("acct_subagent_a").unwrap());
    let body = Bytes::from_static(br#"{ "model":"gpt-5.4","session_id":"root","thread_id":"child","input":[{"role":"user","content":"retain history"}],"account_id":"stale-account","installation_id":"stale-device","turnState":"stale-turn","future":9007199254740993 }"#);
    Mock::given(method("POST"))
        .and(path("/codex/responses/compact"))
        .and(header("authorization", "Bearer at-acct_subagent_b"))
        .and(header("chatgpt-account-id", "chatgpt-acct_subagent_b"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"output":[{"type":"compaction","encrypted_content":"synthetic-result"}]}),
        ))
        .expect(1)
        .mount(&upstream)
        .await;
    let compact = Operation::Compact(gateway_core::operation::CompactRequest::from_raw_json(
        RawJsonPayload::new("openai", body.clone()).unwrap(),
    ));
    let mut stream = provider
        .execute(
            planned_request("openai", compact),
            mode_context(AccountAffinity::Relaxed),
        )
        .await
        .unwrap();
    let mut completed = 0;
    while let Some(event) = stream.next().await {
        completed += event
            .unwrap()
            .canonical_facts()
            .iter()
            .filter(|fact| matches!(fact, GatewayEvent::Completed(_)))
            .count();
    }
    assert_eq!(completed, 1);
    let requests = upstream.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(sent["session_id"], "root");
    assert_eq!(sent["thread_id"], "child");
    assert_eq!(sent["input"][0]["content"], "retain history");
    assert_eq!(sent["future"], json!(9007199254740993u64));
    assert!(sent.get("account_id").is_none());
    assert!(sent.get("turnState").is_none());
    assert_ne!(sent["installation_id"], "stale-device");
    assert!(
        std::str::from_utf8(&body).unwrap().contains("stale-device"),
        "input copy is not mutated"
    );
    upstream.verify().await;
}
