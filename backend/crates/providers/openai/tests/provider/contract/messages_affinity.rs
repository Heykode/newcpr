//! 官方 Claude 会话正反例，接入本地 OAuth → Responses 选号与租约。

use super::*;
use gateway_core::account::AccountAffinity;
use std::sync::atomic::Ordering;

const SESSION: &str = "12345678-1234-1234-1234-123456789abc";

fn operation(header: Option<&str>, user_id: Value) -> Operation {
    let body = json!({"model":"gpt-5.4", "max_tokens":128, "stream":true,
        "metadata":{"user_id":user_id}, "messages":[{"role":"user","content":"hello"}]});
    let mut context = Map::from_iter([("use_websocket".into(), json!(false))]);
    if let Some(header) = header {
        context.insert(
            "opaque_request_headers".to_owned(),
            json!([["x-claude-code-session-id", STANDARD.encode(header)]]),
        );
    }
    Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("anthropic", body.as_object().unwrap().clone())
            .unwrap()
            .with_context(context),
    ))
}

fn attempt(client: &str) -> AttemptContext {
    attempt_with(client, AccountAffinity::Strict, BTreeSet::new())
}

fn attempt_with(
    client: &str,
    mode: AccountAffinity,
    excluded: BTreeSet<ProviderAccountId>,
) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_messages_affinity").unwrap(),
            ClientApiKeyId::new(client).unwrap(),
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(5),
        gateway_core::account::AccountSelectionPolicy::new(
            gateway_core::account::RotationStrategy::Smart,
            NonZeroU32::new(2).unwrap(),
            Duration::ZERO,
        )
        .with_account_affinity(mode)
        .with_queue(gateway_core::concurrency::ConcurrencyQueuePolicy {
            max_waiting: 1,
            timeout: Duration::from_secs(2),
        }),
        AccountAttemptContext::new(excluded, None, None)
            .with_account_scope(contract_account_scope()),
        None,
        CancellationToken::new(),
    )
    .with_request_tuning(gateway_core::routing::RequestTuning {
        account_busy_wait_enabled: true,
        max_waiting_per_account: 1,
        account_concurrency_wait_timeout_seconds: 2,
        ..Default::default()
    })
}

async fn fixture() -> (
    MockServer,
    Arc<CodexProvider>,
    Arc<MemorySessionAffinity>,
    Arc<TestLeaseCoordinator>,
) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
        )
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    for id in ["acct_subagent_a", "acct_subagent_b"] {
        create_account(&store, id).await;
    }
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    let provider = Arc::new(provider_with_affinity_and_base_url_and_leases(
        &store,
        affinity.clone(),
        server.uri(),
        leases.clone(),
    ));
    (server, provider, affinity, leases)
}

#[tokio::test]
async fn claude_sessions_keep_accounts_across_identity_formats_and_isolate_client_keys() {
    let (server, provider, affinity, _) = fixture().await;
    let mut selected = Vec::new();
    let mut keys = Vec::new();
    for (client, header, user_id) in [
        (
            "key_openai_contract",
            Some(SESSION),
            json!("{\"session_id\":\"ignored-body-session\"}"),
        ),
        (
            "key_openai_contract",
            None,
            json!(json!({"session_id":SESSION}).to_string()),
        ),
        (
            "key_openai_contract",
            None,
            json!(format!("user_device_account__session_{SESSION}")),
        ),
        ("different-client", Some(SESSION), json!("ordinary-user")),
        (
            "key_openai_contract",
            Some("different-session"),
            json!("ordinary-user"),
        ),
    ] {
        let before = affinity.lookup_keys().len();
        let source = operation(header, user_id);
        let observed = provider
            .request_observation(&source, &ClientApiKeyId::new(client).unwrap())
            .continuation
            .affinity_hash
            .unwrap();
        let mut stream = provider
            .clone()
            .execute(planned_request("openai", source), attempt(client))
            .await
            .unwrap();
        selected.push(stream.metadata().provider_account_id().clone());
        keys.push(affinity.lookup_keys()[before].clone());
        assert_eq!(keys.last().unwrap(), &observed);
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
    }
    assert!(selected[..3].iter().all(|account| account == &selected[0]));
    assert!(keys[..3].iter().all(|key| key == &keys[0]));
    assert_ne!(keys[3], keys[0]);
    assert_ne!(keys[4], keys[0]);
    assert_eq!(affinity.binding_count(), 3);
    assert!(
        affinity.alias_count() == 0,
        "Claude sessions must not create OpenAI turn aliases"
    );

    let before = affinity.lookup_keys().len();
    drop(
        provider
            .clone()
            .execute(
                planned_request("openai", operation(None, json!("ordinary-user"))),
                attempt("key_openai_contract"),
            )
            .await
            .unwrap(),
    );
    assert_eq!(
        affinity.lookup_keys().len(),
        before,
        "ordinary users and message text do not identify sessions"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 5);
}

#[tokio::test]
async fn messages_strict_affinity_does_not_send_to_an_idle_alternate_when_bound_account_is_busy() {
    let (server, provider, affinity, leases) = fixture().await;
    let request = || planned_request("openai", operation(Some(SESSION), json!("user")));
    let first = provider
        .clone()
        .execute(request(), attempt("key_openai_contract"))
        .await
        .unwrap();
    let bound = first.metadata().provider_account_id().clone();
    drop(first);
    leases.capacity.set_load(bound.as_str(), 3);
    let context = attempt("key_openai_contract");
    let cancellation = context.cancellation().clone();
    let request = request();
    let pending = tokio::spawn(async move { provider.execute(request, context).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while leases.capacity.waiting.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    assert!(pending.await.unwrap().is_err());
    assert!(
        leases
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.account_id() == &bound)
    );
    assert_eq!(affinity.binding_count(), 1);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn messages_retry_rebinds_but_preferred_busy_overflow_keeps_the_original_binding() {
    for mode in [AccountAffinity::Strict, AccountAffinity::Preferred] {
        let (_, provider, affinity, leases) = fixture().await;
        let request = || planned_request("openai", operation(Some(SESSION), json!("user")));
        let first = provider
            .execute(request(), attempt("key_openai_contract"))
            .await
            .unwrap();
        let original = first.metadata().provider_account_id().clone();
        drop(first);
        let excluded = if mode == AccountAffinity::Strict {
            // Core authorizes a retry by excluding the failed account; budgets stay in Core.
            BTreeSet::from([original.clone()])
        } else {
            leases.capacity.set_load(original.as_str(), 3);
            BTreeSet::new()
        };
        let mut switched = provider
            .execute(
                request(),
                attempt_with("key_openai_contract", mode, excluded),
            )
            .await
            .unwrap();
        let alternate = switched.metadata().provider_account_id().clone();
        assert_ne!(alternate, original);
        while let Some(event) = switched.next().await {
            event.unwrap();
        }
        drop(switched);
        leases.capacity.signals.lock().unwrap().clear();
        let resumed = provider
            .execute(
                request(),
                attempt_with("key_openai_contract", mode, BTreeSet::new()),
            )
            .await
            .unwrap();
        assert_eq!(
            resumed.metadata().provider_account_id(),
            if mode == AccountAffinity::Strict {
                &alternate
            } else {
                &original
            }
        );
        assert_eq!(affinity.binding_count(), 1);
        assert_eq!(affinity.alias_count(), 0);
    }
}
