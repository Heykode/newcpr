use super::*;
use gateway_core::operation::{
    ProviderHttpHeader, ProviderHttpMethod, ProviderHttpRequest, RawHttpPayload,
};
use gateway_core::provider_ports::{ProviderSessionAffinityKey, ProviderSessionAffinityPort};

const TURN: &str = "turn_child_endpoint";

#[tokio::test]
async fn connection_local_replay_has_no_selection_or_affinity_side_effects() {
    use gateway_core::engine::continuation::NativeContinuationScope;
    for existing in [false, true] {
        for state_scope in [false, true] {
            for attempt in [
                ContinuationAttempt::ReplayOwner,
                ContinuationAttempt::ReplayAny,
            ] {
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
                if existing {
                    seed_child_turn(&provider).await;
                }
                let before = (
                    affinity.binding_count(),
                    affinity.alias_count(),
                    affinity.lookup_keys(),
                    affinity.claim_ttls(),
                    affinity.renewal_ttls(),
                    leases.requests.lock().unwrap().len(),
                );
                let mut generation = generate_with_session_context("root", Some("child"), None);
                if state_scope {
                    generation = generation.with_provider_session_state(
                        ProviderSessionState::new(
                            "openai",
                            Map::from_iter([
                                ("account_id".to_owned(), json!("acct_subagent_a")),
                                ("conversation_id".to_owned(), json!("lc_preflight")),
                                ("continuation_scope".to_owned(), json!("connection_local")),
                            ]),
                        )
                        .unwrap(),
                    );
                }
                let account = ProviderAccountId::new("acct_subagent_a").unwrap();
                let provider_kind = ProviderKind::new("openai").unwrap();
                let key = ClientApiKeyId::new("key_openai_contract").unwrap();
                let pin = NativeContinuationPin::new(
                    PreviousResponseId::new("resp_client"),
                    PreviousResponseId::new("resp_upstream"),
                    key.clone(),
                    provider_kind.clone(),
                    account.clone(),
                )
                .with_scope(if state_scope {
                    NativeContinuationScope::Persisted
                } else {
                    NativeContinuationScope::ConnectionLocal
                });
                let attempt = AttemptContext::new(
                    RequestAttemptContext::new(ModelRequestId::new("req_preflight").unwrap(), key),
                    NonZeroU32::new(2).unwrap(),
                    SystemTime::now() + Duration::from_secs(30),
                    account_policy(),
                    AccountAttemptContext::new(
                        BTreeSet::new(),
                        None,
                        Some(ProviderAccountStateOwner::new(provider_kind, account)),
                    )
                    .with_account_scope(contract_account_scope()),
                    Some(ContinuationBinding::Pinned(pin)),
                    CancellationToken::new(),
                )
                .with_continuation_attempt(attempt);
                let error = provider
                    .execute(
                        planned_request("openai", Operation::Generate(generation)),
                        attempt,
                    )
                    .await
                    .err()
                    .unwrap();
                assert_eq!(
                    error.kind(),
                    ProviderErrorKind::ContinuationRecoveryRequired
                );
                assert_eq!(error.send_state(), UpstreamSendState::NotSent);
                assert_eq!(
                    before,
                    (
                        affinity.binding_count(),
                        affinity.alias_count(),
                        affinity.lookup_keys(),
                        affinity.claim_ttls(),
                        affinity.renewal_ttls(),
                        leases.requests.lock().unwrap().len()
                    )
                );
                assert!(upstream.received_requests().await.unwrap().is_empty());
            }
        }
    }
}

async fn seed_child_turn(provider: &CodexProvider) {
    let generation = generate_with_session_context("root", Some("child"), None);
    let payload = generation
        .protocol_payload()
        .clone()
        .with_context(Map::from_iter([
            ("turn_id".to_owned(), json!(TURN)),
            ("use_websocket".to_owned(), json!(false)),
        ]));
    let stream = provider
        .execute(
            planned_request(
                "openai",
                Operation::Generate(GenerateRequest::from_protocol_payload(payload)),
            ),
            context("req_seed_child_turn", CancellationToken::new()),
        )
        .await
        .expect("seed root binding and child turn alias");
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_a"
    );
    drop(stream);
}

fn image(session: Option<&str>) -> Operation {
    let mut body = json!({"prompt":"draw a test square"});
    if let Some(session) = session {
        body["session_id"] = json!(session);
    }
    Operation::GenerateImage(ImageRequest::from_raw_json(
        ImageRequestKind::Generation,
        RawJsonPayload::new("openai", Bytes::from(body.to_string()))
            .unwrap()
            .with_context(Map::from_iter([("image_turn_id".to_owned(), json!(TURN))])),
    ))
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
        Arc::clone(&affinity),
        upstream.uri(),
        Arc::clone(&leases),
    );
    seed_child_turn(&provider).await;
    assert_eq!(affinity.alias_count(), 1);
    create_account(&store, "acct_subagent_b").await;
    store.set_scheduling("acct_subagent_b", None, AccountWeight::new(100).unwrap());
    (provider, affinity, leases, upstream)
}

#[tokio::test]
async fn image_turn_alias_only_keeps_the_root_account_when_busy() {
    let (provider, affinity, leases, upstream) = fixture().await;
    leases
        .busy_accounts
        .lock()
        .unwrap()
        .insert(ProviderAccountId::new("acct_subagent_a").unwrap());
    let error = provider
        .execute(
            planned_provider_endpoint_request("openai", image(None)),
            context("req_image_alias_busy", CancellationToken::new()),
        )
        .await
        .err()
        .expect("child must not escape to B");
    assert_eq!(error.kind(), ProviderErrorKind::AccountCapacityUnavailable);
    assert_eq!(affinity.binding_count(), 1);
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn image_turn_alias_conflicting_root_is_rejected_before_send() {
    let (provider, affinity, _leases, upstream) = fixture().await;
    let error = provider
        .execute(
            planned_provider_endpoint_request("openai", image(Some("different-root"))),
            context("req_image_alias_conflict", CancellationToken::new()),
        )
        .await
        .err()
        .expect("conflicting root");
    assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
    assert_eq!(affinity.binding_count(), 1);
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn child_turn_alias_with_lost_root_atomically_claims_a_new_binding() {
    let (provider, affinity, _leases, upstream) = fixture().await;
    let key = ProviderSessionAffinityKey::try_new(affinity.lookup_keys()[0].clone()).unwrap();
    affinity
        .clear(&ProviderKind::new("openai").unwrap(), &key)
        .await
        .unwrap();
    let stream = provider
        .execute(
            planned_provider_endpoint_request("openai", image(None)),
            context("req_lost_alias_root", CancellationToken::new()),
        )
        .await
        .expect("missing binding can be claimed through its retained session alias");
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
    assert_eq!(affinity.binding_count(), 1);
    assert_eq!(affinity.alias_count(), 1);
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn live_turn_alias_only_keeps_the_root_account_when_busy() {
    let (provider, affinity, leases, upstream) = fixture().await;
    leases
        .busy_accounts
        .lock()
        .unwrap()
        .insert(ProviderAccountId::new("acct_subagent_a").unwrap());
    let operation = Operation::ProviderHttp(
        ProviderHttpRequest::new(
            "realtime-calls",
            ProviderHttpMethod::Post,
            None,
            vec![ProviderHttpHeader::new(
                "x-client-turn-id",
                Bytes::from_static(TURN.as_bytes()),
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
    let error = provider
        .execute(
            ProviderRequest::new(operation, plan.candidates()[0].clone()),
            context("req_live_alias_busy", CancellationToken::new()),
        )
        .await
        .err()
        .expect("Live child must not escape to B");
    assert_eq!(error.kind(), ProviderErrorKind::AccountCapacityUnavailable);
    assert_eq!(affinity.binding_count(), 1);
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn native_owner_wins_over_a_conflicting_turn_alias_without_rewriting_it() {
    let (provider, affinity, _leases, upstream) = fixture().await;
    let generation = generate_with_session_context("another-root", None, None);
    let payload = generation
        .protocol_payload()
        .clone()
        .with_context(Map::from_iter([
            ("turn_id".to_owned(), json!(TURN)),
            ("use_websocket".to_owned(), json!(false)),
        ]));
    let stream = provider
        .execute(
            planned_request(
                "openai",
                Operation::Generate(GenerateRequest::from_protocol_payload(payload)),
            ),
            pinned_continuation_context(
                "req_native_conflicting_alias",
                "acct_subagent_b",
                "resp_native_owner",
                "resp_native_owner",
                1,
                ContinuationAttempt::Native,
            ),
        )
        .await
        .expect("native owner must not be rejected by a stale turn alias");
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
    drop(stream);
    assert_eq!(affinity.alias_count(), 1);
    let image_stream = provider
        .execute(
            planned_provider_endpoint_request("openai", image(None)),
            context("req_alias_original_root", CancellationToken::new()),
        )
        .await
        .expect("alias still follows its original root");
    assert_eq!(
        image_stream.metadata().provider_account_id().as_str(),
        "acct_subagent_a"
    );
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn restored_owner_wins_over_a_conflicting_turn_alias() {
    let (provider, affinity, _leases, upstream) = fixture().await;
    let generation = generate_with_persisted_session_context(
        "acct_subagent_b",
        "lc_restored_owner",
        "another-root",
        "another-root",
    );
    let payload = generation
        .protocol_payload()
        .clone()
        .with_context(Map::from_iter([
            ("turn_id".to_owned(), json!(TURN)),
            ("use_websocket".to_owned(), json!(false)),
        ]));
    let generation = GenerateRequest::from_protocol_payload(payload)
        .with_provider_session_state(generation.provider_session_state("openai").unwrap().clone());
    let stream = provider
        .execute(
            planned_request("openai", Operation::Generate(generation)),
            context("req_restored_conflicting_alias", CancellationToken::new()),
        )
        .await
        .expect("restored owner is authoritative");
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
    drop(stream);
    assert_eq!(affinity.alias_count(), 1);
    let stream = provider
        .execute(
            planned_provider_endpoint_request("openai", image(None)),
            context("req_restored_alias_original_root", CancellationToken::new()),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_a"
    );
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn turn_alias_cannot_be_read_by_another_client_key() {
    let (provider, affinity, leases, upstream) = fixture().await;
    leases
        .busy_accounts
        .lock()
        .unwrap()
        .insert(ProviderAccountId::new("acct_subagent_a").unwrap());
    let attempt = AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_cross_key_turn_alias").unwrap(),
            ClientApiKeyId::new("key_another_client").unwrap(),
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope()),
        None,
        CancellationToken::new(),
    );
    let stream = provider
        .execute(
            planned_provider_endpoint_request("openai", image(None)),
            attempt,
        )
        .await
        .expect("another key can select independently without inheriting the busy root");
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
    assert_eq!(affinity.binding_count(), 1);
    assert_eq!(affinity.alias_count(), 1);
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn child_waits_for_its_busy_root_with_a_free_alternative_and_releases_on_cancel() {
    use std::sync::atomic::Ordering;

    for cancel in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        let affinity = Arc::new(MemorySessionAffinity::default());
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        let upstream = MockServer::start().await;
        let provider = super::scheduling::waiting_provider_with_affinity_and_limit(
            &store,
            Arc::clone(&leases),
            upstream.uri(),
            Arc::new(MemorySessionExclusions::default()),
            Arc::clone(&affinity),
            NonZeroU32::MIN,
        );
        let root = provider
            .execute(
                planned_request(
                    "openai",
                    Operation::Generate(generate_with_session_context("root", None, None)),
                ),
                context("req_seed_wait_root", CancellationToken::new()),
            )
            .await
            .unwrap();
        assert_eq!(
            root.metadata().provider_account_id().as_str(),
            "acct_scope_old"
        );
        drop(root);
        assert_eq!(affinity.binding_count(), 1, "root is already bound");
        create_account(&store, "acct_scope_new").await;
        store.set_scheduling("acct_scope_new", None, AccountWeight::new(100).unwrap());
        leases.capacity.set_load("acct_scope_old", 1);
        leases.capacity.set_load("acct_scope_new", 0);
        // Only capacity is busy; the seed request's interval is not this scenario.
        leases
            .capacity
            .signals
            .lock()
            .unwrap()
            .get_mut(&ProviderAccountId::new("acct_scope_old").unwrap())
            .unwrap()
            .last_started_at = None;
        let cancellation = CancellationToken::new();
        let attempt = context("req_busy_root_child", cancellation.clone()).with_request_tuning(
            gateway_core::routing::RequestTuning {
                account_busy_wait_enabled: true,
                ..Default::default()
            },
        );
        let generation = generate_with_session_context("root", Some("child"), None);
        let pending = provider.execute(
            planned_request("openai", Operation::Generate(generation)),
            attempt,
        );
        tokio::pin!(pending);
        tokio::select! {
            result = &mut pending => panic!("child escaped the root wait: {:?}", result.err()),
            () = async {
                timeout(Duration::from_secs(2), async {
                    while leases.capacity.waiting.load(Ordering::SeqCst) == 0 {
                        tokio::task::yield_now().await;
                    }
                }).await.expect("child queues for root capacity");
            } => {}
        }
        assert_eq!(
            leases.capacity.waits.lock().unwrap().last().unwrap().mode(),
            gateway_core::engine::AccountWaitMode::Sticky
        );
        assert!(upstream.received_requests().await.unwrap().is_empty());
        if cancel {
            cancellation.cancel();
            assert_eq!(
                pending.await.err().unwrap().kind(),
                ProviderErrorKind::Cancelled
            );
        } else {
            leases.capacity.set_load("acct_scope_old", 0);
            let stream = timeout(Duration::from_secs(3), pending)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                stream.metadata().provider_account_id().as_str(),
                "acct_scope_old"
            );
            drop(stream);
        }
        assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
        assert!(
            leases
                .capacity
                .promotion_requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| request.account_id().as_str() == "acct_scope_old")
        );
        assert_eq!(affinity.binding_count(), 1);
        assert!(upstream.received_requests().await.unwrap().is_empty());
    }
}
