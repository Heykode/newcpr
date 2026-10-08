//! Guardian is a checked, client-scoped preference, not a native continuation owner.

use gateway_core::provider_ports::{
    ProviderConcurrencyPool, ProviderSessionAffinityKey, ProviderSessionAffinityPort,
};
use gateway_core::routing::RequestTuning;

use super::*;

fn request(model: &str, body: Value, mut protocol_context: Map<String, Value>) -> ProviderRequest {
    protocol_context.insert("use_websocket".to_owned(), json!(false));
    let payload = ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
        .unwrap()
        .with_context(protocol_context);
    planned_request_with_model(
        "openai",
        Operation::Generate(GenerateRequest::from_protocol_payload(payload)),
        model,
    )
}

fn child_body(kind: &str) -> Value {
    json!({"input": [], "turn_metadata": {
        "subagent_kind": kind, "parent_thread_id": "parent-thread"
    }})
}

fn attempt(client: &str, exclusions: BTreeSet<ProviderAccountId>, reserved: u32) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_guardian_contract").unwrap(),
            ClientApiKeyId::new(client).unwrap(),
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(30),
        account_policy().with_openai_guardian_reserved_concurrency(reserved),
        AccountAttemptContext::new(exclusions, None, None)
            .with_account_scope(contract_account_scope()),
        None,
        CancellationToken::new(),
    )
}

async fn complete(
    provider: &CodexProvider,
    request: ProviderRequest,
    attempt: AttemptContext,
) -> ProviderAccountId {
    let mut stream = provider.execute(request, attempt).await.expect("prepare");
    let account = stream.metadata().provider_account_id().clone();
    timeout(Duration::from_secs(5), async {
        let mut completed = false;
        while let Some(event) = stream.next().await {
            completed |= event
                .expect("local response")
                .canonical_facts()
                .iter()
                .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
        }
        assert!(
            completed,
            "wire completion alone is not successful execution"
        );
    })
    .await
    .expect("bounded response");
    account
}

async fn successful_server() -> MockServer {
    let server = MockServer::start().await;
    mount_success(&server).await;
    server
}

async fn mount_success(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!(
                    "event: response.created\ndata: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_scope_capture\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\"}}}}\n\n{CAPTURE_COMPLETED_SSE}"
                )),
        )
        .mount(server)
        .await;
}

async fn parent_key(provider: &CodexProvider, bindings: &MemorySessionAffinity) -> String {
    let parent = || {
        request(
            "gpt-5.4",
            json!({"input": [{"role":"user","content":"parent"}]}),
            Map::from_iter([("thread_id".to_owned(), json!("parent-thread"))]),
        )
    };
    let stream = provider
        .execute(parent(), attempt("key_openai_contract", BTreeSet::new(), 0))
        .await
        .unwrap();
    drop(stream);
    assert!(
        bindings.bind_keys().is_empty(),
        "prepare must not record a parent"
    );
    complete(
        provider,
        parent(),
        attempt("key_openai_contract", BTreeSet::new(), 0),
    )
    .await;
    assert_eq!(bindings.bind_keys().len(), 1);
    bindings.bind_keys()[0].clone()
}

async fn bound(bindings: &MemorySessionAffinity, key: &str) -> ProviderAccountId {
    bindings
        .load(
            &ProviderKind::new("openai").unwrap(),
            &ProviderSessionAffinityKey::try_new(key).unwrap(),
        )
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn guardian_and_review_inherit_parent_without_writing_or_renewing_parent_binding() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    create_account(&store, "acct_scope_new").await;
    let bindings = Arc::new(MemorySessionAffinity::default());
    let server = successful_server().await;
    let provider = provider_with_affinity_and_base_url(&store, Arc::clone(&bindings), server.uri());
    let key = parent_key(&provider, &bindings).await;
    let parent_account = ProviderAccountId::new("acct_scope_old").unwrap();
    bindings.seed_binding(
        &ProviderKind::new("openai").unwrap(),
        &key,
        parent_account.clone(),
    );
    let writes = bindings.bind_keys();
    let renewals = bindings.renewal_ttls();
    for kind in ["guardian", "review"] {
        let stream = provider
            .execute(
                request("codex-auto-review", child_body(kind), Map::new()),
                attempt("key_openai_contract", BTreeSet::new(), 0),
            )
            .await
            .unwrap();
        assert_eq!(stream.metadata().provider_account_id(), &parent_account);
        drop(stream);
        assert_eq!(
            complete(
                &provider,
                request("codex-auto-review", child_body(kind), Map::new()),
                attempt("key_openai_contract", BTreeSet::new(), 0),
            )
            .await,
            parent_account
        );
    }
    assert_eq!(bindings.bind_keys(), writes);
    assert_eq!(bindings.renewal_ttls(), renewals);
    assert_eq!(bound(&bindings, &key).await, parent_account);
    let before = bindings.lookup_keys().len();
    let stream = provider
        .execute(
            request("codex-auto-review", child_body("guardian"), Map::new()),
            attempt("other-client", BTreeSet::new(), 0),
        )
        .await
        .unwrap();
    drop(stream);
    assert!(
        !bindings.lookup_keys()[before..].contains(&key),
        "parent keys must be client-scoped"
    );
}

#[tokio::test]
async fn guardian_parent_preference_rechecks_all_account_eligibility_and_lease_capacity() {
    for state in ["disabled", "outside-scope", "excluded", "quota", "busy"] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_new").await;
        let old_id = if state == "outside-scope" {
            "acct_outside_hint_scope"
        } else {
            "acct_scope_old"
        };
        create_account_with_enabled(&store, old_id, state != "disabled").await;
        let bindings = Arc::new(MemorySessionAffinity::default());
        let leases = Arc::new(TestLeaseCoordinator::default());
        let server = successful_server().await;
        let provider = provider_with_affinity_and_base_url_and_leases(
            &store,
            Arc::clone(&bindings),
            server.uri(),
            Arc::clone(&leases),
        );
        let key = parent_key(&provider, &bindings).await;
        let old_account = ProviderAccountId::new(old_id).unwrap();
        bindings.seed_binding(
            &ProviderKind::new("openai").unwrap(),
            &key,
            old_account.clone(),
        );
        if state == "busy" {
            leases
                .busy_accounts
                .lock()
                .unwrap()
                .insert(old_account.clone());
        }
        if state == "quota" {
            let account = store.account(old_id).unwrap();
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
                .unwrap();
        }
        let excluded = if state == "excluded" {
            BTreeSet::from([old_account.clone()])
        } else {
            BTreeSet::new()
        };
        let writes = bindings.bind_keys();
        assert_eq!(
            complete(
                &provider,
                request("codex-auto-review", child_body("guardian"), Map::new()),
                attempt("key_openai_contract", excluded, 0)
            )
            .await
            .as_str(),
            "acct_scope_new",
            "{state}"
        );
        assert_eq!(
            bound(&bindings, &key).await,
            old_account,
            "{state}: parent unchanged"
        );
        assert_eq!(
            bindings.bind_keys(),
            writes,
            "{state}: child does not record parent"
        );
    }
}

#[tokio::test]
async fn guardian_parent_preference_never_overrides_own_binding_or_native_owner() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    create_account(&store, "acct_scope_new").await;
    let bindings = Arc::new(MemorySessionAffinity::default());
    let server = successful_server().await;
    let provider = provider_with_affinity_and_base_url(&store, Arc::clone(&bindings), server.uri());
    let parent_key = parent_key(&provider, &bindings).await;
    let kind = ProviderKind::new("openai").unwrap();
    let old = ProviderAccountId::new("acct_scope_old").unwrap();
    let new = ProviderAccountId::new("acct_scope_new").unwrap();
    bindings.seed_binding(&kind, &parent_key, old.clone());
    let child = || {
        request(
            "codex-auto-review",
            child_body("guardian"),
            Map::from_iter([("thread_id".to_owned(), json!("child-thread"))]),
        )
    };
    let before = bindings.lookup_keys().len();
    let stream = provider
        .execute(child(), attempt("key_openai_contract", BTreeSet::new(), 0))
        .await
        .unwrap();
    drop(stream);
    let own_key = bindings.lookup_keys()[before].clone();
    assert_ne!(own_key, parent_key);
    bindings.seed_binding(&kind, &own_key, new.clone());
    let before = bindings.lookup_keys().len();
    let stream = provider
        .execute(child(), attempt("key_openai_contract", BTreeSet::new(), 0))
        .await
        .unwrap();
    assert_eq!(stream.metadata().provider_account_id(), &new);
    drop(stream);
    assert_eq!(&bindings.lookup_keys()[before..], &[own_key]);

    let before = bindings.lookup_keys().len();
    let stream = provider
        .execute(
            request("codex-auto-review", child_body("guardian"), Map::new()),
            pinned_continuation_context(
                "req_guardian_native",
                new.as_str(),
                "resp_client",
                "resp_upstream",
                1,
                ContinuationAttempt::Native,
            ),
        )
        .await
        .unwrap();
    assert_eq!(stream.metadata().provider_account_id(), &new);
    drop(stream);
    assert_eq!(
        bindings.lookup_keys().len(),
        before,
        "native owner does not consult parent preference"
    );
    assert_eq!(bound(&bindings, &parent_key).await, old);
}

#[tokio::test]
async fn guardian_metadata_is_strict_and_reservation_reaches_the_actual_lease() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    let bindings = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    let server = successful_server().await;
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        Arc::clone(&bindings),
        server.uri(),
        Arc::clone(&leases),
    );
    let valid = child_body("guardian");
    let cases = [
        ("codex-auto-review", valid.clone(), Map::new(), true),
        ("codex-auto-review", child_body("review"), Map::new(), true),
        ("gpt-5.4", valid.clone(), Map::new(), false),
        (
            "codex-auto-review",
            child_body("explorer"),
            Map::new(),
            false,
        ),
        (
            "codex-auto-review",
            json!({"turn_metadata":{"subagent_kind":"guardian"}}),
            Map::new(),
            false,
        ),
        (
            "codex-auto-review",
            json!({"turn_metadata":{"subagent_kind":"guardian","parent_thread_id":" "}}),
            Map::new(),
            false,
        ),
        (
            "codex-auto-review",
            json!({"turn_metadata":{"subagent_kind":"guardian","parent_thread_id":42},"parent_thread_id":"parent-thread"}),
            Map::new(),
            false,
        ),
        (
            "codex-auto-review",
            json!({"turn_metadata":{"subagent_kind":42,"parent_thread_id":"parent-thread"},"client_metadata":{"x-openai-subagent":"guardian"}}),
            Map::new(),
            false,
        ),
        (
            "codex-auto-review",
            json!({"turn_metadata":{"subagent_kind":"guardian","parent_thread_id":"parent-thread","parentThreadId":"conflict"}}),
            Map::new(),
            false,
        ),
        (
            "codex-auto-review",
            json!({"turn_metadata":{"subagent_kind":"guardian","parent_thread_id":"parent-thread"},"parent_thread_id":"parent-thread","parentThreadId":"conflict"}),
            Map::new(),
            false,
        ),
        (
            "codex-auto-review",
            json!({"turn_metadata":{"subagent_kind":"guardian","parent_thread_id":"parent-thread"},"client_metadata":{"parent_thread_id":"parent-thread","parentThreadId":"conflict"}}),
            Map::new(),
            false,
        ),
        (
            "codex-auto-review",
            json!({"turn_metadata":"not-json"}),
            Map::new(),
            false,
        ),
        (
            "codex-auto-review",
            valid.clone(),
            Map::from_iter([("turn_metadata".to_owned(), json!("not-json"))]),
            false,
        ),
        (
            "codex-auto-review",
            valid.clone(),
            Map::from_iter([("parent_thread_id".to_owned(), json!("conflict"))]),
            false,
        ),
        (
            "codex-auto-review",
            json!({"client_metadata":{"x-openai-subagent":"guardian","parentThreadId":"parent-thread"}}),
            Map::new(),
            true,
        ),
        (
            "codex-auto-review",
            json!({"client_metadata":{"x-codex-turn-metadata": "{\"subagent_kind\":\"guardian\",\"parent_thread_id\":\"parent-thread\"}"}}),
            Map::new(),
            true,
        ),
        (
            "codex-auto-review",
            json!({"turn_metadata":{"subagent_kind":"guardian","parent_thread_id":"parent-thread"},"client_metadata":{"turnMetadata":{"subagent_kind":"review","parent_thread_id":"parent-thread"}}}),
            Map::new(),
            false,
        ),
        (
            "codex-auto-review",
            json!({"turn_metadata":{"subagent_kind":"guardian","parent_thread_id":"parent-thread"},"client_metadata":{"turnMetadata":false}}),
            Map::new(),
            false,
        ),
        (
            "codex-auto-review",
            json!({"turn_metadata":{"subagent_kind":"guardian","parent_thread_id":"parent-thread"},"client_metadata":[]}),
            Map::new(),
            false,
        ),
    ];
    for (index, (model, body, protocol_context, guardian)) in cases.into_iter().enumerate() {
        let before = bindings.lookup_keys().len();
        let stream = provider
            .execute(
                request(model, body, protocol_context),
                attempt("key_openai_contract", BTreeSet::new(), 1),
            )
            .await
            .expect("prepare valid transport");
        drop(stream);
        let requests = leases.requests.lock().unwrap();
        assert_eq!(
            requests.last().unwrap().max_concurrent().get(),
            if guardian { 1 } else { 2 },
            "case {index}"
        );
        assert_eq!(
            requests.last().unwrap().concurrency_pool(),
            if guardian {
                ProviderConcurrencyPool::Reserved
            } else {
                ProviderConcurrencyPool::Shared
            }
        );
        drop(requests);
        assert_eq!(
            bindings.lookup_keys().len() - before,
            usize::from(guardian),
            "case {index}: only valid Guardian reads the parent"
        );
    }
    for reserved in [0, 1, u32::MAX] {
        for guardian in [false, true] {
            let body = if guardian {
                valid.clone()
            } else {
                json!({"input":[]})
            };
            let model = if guardian {
                "codex-auto-review"
            } else {
                "gpt-5.4"
            };
            let stream = provider
                .execute(
                    request(model, body, Map::new()),
                    attempt("key_openai_contract", BTreeSet::new(), reserved),
                )
                .await
                .unwrap();
            drop(stream);
            assert_eq!(
                leases
                    .requests
                    .lock()
                    .unwrap()
                    .last()
                    .unwrap()
                    .max_concurrent()
                    .get(),
                if guardian && reserved > 0 {
                    reserved
                } else {
                    2
                }
            );
        }
    }
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "prepare must not send upstream"
    );
    assert!(bindings.bind_keys().is_empty());
}

#[tokio::test]
async fn failed_parent_or_guardian_does_not_record_a_parent_binding() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_scope_old").await;
    let bindings = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            json!({"error":{"type":"invalid_request_error","message":"local fixture failure"}}),
        ))
        .mount(&server)
        .await;
    let provider = provider_with_affinity_and_base_url(&store, Arc::clone(&bindings), server.uri());
    for request in [
        request(
            "gpt-5.4",
            json!({"input": [{"role":"user","content":"parent"}]}),
            Map::from_iter([("thread_id".to_owned(), json!("parent-thread"))]),
        ),
        request("codex-auto-review", child_body("guardian"), Map::new()),
    ] {
        let mut stream = provider
            .execute(request, attempt("key_openai_contract", BTreeSet::new(), 0))
            .await
            .unwrap();
        let mut failed = false;
        timeout(Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                match event {
                    Err(_) => failed = true,
                    Ok(event) => assert!(
                        !event
                            .canonical_facts()
                            .iter()
                            .any(|fact| matches!(fact, GatewayEvent::Completed(_)))
                    ),
                }
            }
        })
        .await
        .unwrap();
        assert!(failed, "fixture must actually fail");
        assert!(bindings.bind_keys().is_empty());
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn guardian_reservation_matches_live_capacity_wait_and_promotion_limits() {
    use std::sync::atomic::Ordering;

    for (guardian, reserved, expected_limit, should_wait) in [
        (false, 0, 2, false),
        (false, 1, 2, false),
        (true, 0, 2, false),
        (true, 1, 1, true),
        (true, 3, 3, false),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_scope_old").await;
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        leases.capacity.set_load("acct_scope_old", 1);
        leases
            .reserved_capacity
            .enabled
            .store(true, Ordering::SeqCst);
        leases.reserved_capacity.set_load("acct_scope_old", 1);
        let pool = if guardian && reserved > 0 {
            ProviderConcurrencyPool::Reserved
        } else {
            ProviderConcurrencyPool::Shared
        };
        let capacity = if pool == ProviderConcurrencyPool::Reserved {
            &leases.reserved_capacity
        } else {
            &leases.capacity
        };
        let bindings = Arc::new(MemorySessionAffinity::default());
        let server = successful_server().await;
        let provider = super::scheduling::waiting_provider_with_affinity_and_limit(
            &store,
            Arc::clone(&leases),
            server.uri(),
            Arc::new(MemorySessionExclusions::default()),
            bindings,
            NonZeroU32::new(2).unwrap(),
        );
        let model = if guardian {
            "codex-auto-review"
        } else {
            "gpt-5.4"
        };
        let body = if guardian {
            child_body("guardian")
        } else {
            json!({"input":[]})
        };
        let request = request(model, body, Map::new());
        let attempt = attempt("key_openai_contract", BTreeSet::new(), reserved)
            .with_request_tuning(RequestTuning {
                account_busy_wait_enabled: true,
                ..Default::default()
            });
        let execution = provider.execute(request, attempt);
        tokio::pin!(execution);
        if should_wait {
            tokio::select! {
                result = &mut execution => panic!("saturated pool must wait, prepared={}", result.is_ok()),
                _ = timeout(Duration::from_secs(2), async {
                    while capacity.waiting.load(Ordering::SeqCst) == 0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }) => {}
            }
            assert_eq!(capacity.waiting.load(Ordering::SeqCst), 1);
            assert!(server.received_requests().await.unwrap().is_empty());
            capacity.set_load("acct_scope_old", 0);
        }
        let stream = timeout(Duration::from_secs(3), &mut execution)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_scope_old"
        );
        let lease_requests = if should_wait {
            capacity.promotion_requests.lock().unwrap()
        } else {
            leases.requests.lock().unwrap()
        };
        assert!(
            !lease_requests.is_empty(),
            "actual admission must be observed"
        );
        assert!(
            lease_requests
                .iter()
                .all(|request| request.max_concurrent().get() == expected_limit
                    && request.concurrency_pool() == pool),
            "every admission must use the effective limit"
        );
        assert_eq!(capacity.promotions.load(Ordering::SeqCst) > 0, should_wait);
        assert_eq!(capacity.waiting.load(Ordering::SeqCst), 0);
        drop(stream);
    }
}
