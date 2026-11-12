use super::*;

fn preferred_context(request_id: &str, scope: Arc<FrozenAccountScope>) -> AttemptContext {
    preferred_context_with_ttl(request_id, scope, Duration::from_secs(24 * 3600))
}
fn preferred_context_with_ttl(
    request_id: &str,
    scope: Arc<FrozenAccountScope>,
    ttl: Duration,
) -> AttemptContext {
    preferred_context_with_policy(
        request_id,
        scope,
        account_policy()
            .with_openai_account_affinity(gateway_core::account::AccountAffinity::Preferred)
            .with_openai_session_binding_ttl(ttl),
    )
}
fn preferred_context_with_policy(
    request_id: &str,
    scope: Arc<FrozenAccountScope>,
    policy: gateway_core::account::AccountSelectionPolicy,
) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new(request_id).unwrap(),
            ClientApiKeyId::new("key_openai_contract").unwrap(),
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(5),
        policy,
        AccountAttemptContext::new(BTreeSet::new(), None, None).with_account_scope(scope),
        None,
        CancellationToken::new(),
    )
}

fn turn_request(root: &str, turn: &str) -> Operation {
    Operation::Generate(GenerateRequest::from_protocol_payload(ProtocolPayload::json_object("openai", json!({"model":"gpt-5.4", "input":"hello", "client_metadata":{"session_id":root,"thread_id":root,"turn_id":turn}}).as_object().unwrap().clone()).unwrap()))
}

#[tokio::test]
async fn preferred_primary_migration_distinguishes_temporary_and_persistent_unavailability() {
    use gateway_core::account::{
        AccountAffinity, AccountRuntimeSignals, AccountSelectionPolicy, AccountStateChange,
        RotationStrategy,
    };

    for (reason, migrated) in [
        ("concurrency", false),
        ("quota_soon", false),
        ("quota_just_reset", false),
        ("disabled", true),
        ("deleted", true),
        ("expired", true),
        ("invalid", true),
        ("banned", true),
        ("quota_unknown", true),
        ("quota_later", true),
        ("quota_stale", true),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_subagent_a").await;
        let leases = Arc::new(TestLeaseCoordinator::default());
        let server = MockServer::start().await;
        let provider = provider_with_affinity_and_base_url_and_leases(
            &store,
            Arc::new(MemorySessionAffinity::default()),
            server.uri(),
            leases.clone(),
        );
        let policy = AccountSelectionPolicy::new(
            RotationStrategy::Smart,
            NonZeroU32::new(2).unwrap(),
            Duration::ZERO,
        )
        .with_openai_account_affinity(AccountAffinity::Preferred);
        let select = || {
            provider.execute(
                planned_request("openai", turn_request("root", "availability-turn")),
                preferred_context_with_policy(
                    "req_primary_availability",
                    contract_account_scope(),
                    policy,
                ),
            )
        };
        drop(select().await.unwrap());
        create_account(&store, "acct_subagent_b").await;
        let primary = ProviderAccountId::new("acct_subagent_a").unwrap();
        let account = store.account(primary.as_str()).unwrap();
        let now = SystemTime::now();
        match reason {
            "concurrency" => {
                leases.capacity.signals.lock().unwrap().insert(
                    primary.clone(),
                    AccountRuntimeSignals {
                        in_flight: 2,
                        last_started_at: None,
                        quota_reset_at: None,
                        quota_reset_windows: None,
                        quota_remaining_rank: None,
                        rate_limited_until: None,
                        failure_rate_basis_points: None,
                        first_output_latency_ms: None,
                    },
                );
            }
            "disabled" => store.set_enabled(&primary, false).await.unwrap(),
            "deleted" => store.delete_account(&primary).await.unwrap(),
            "expired" | "invalid" | "banned" => {
                let state = CredentialState::parse(reason).unwrap();
                store
                    .apply_state_change(AccountStateChange {
                        account_id: primary.clone(),
                        expected_revision: account.revision(),
                        credential_state: state,
                        observed_at: now,
                        error_reason: state.error_reason(),
                        message: None,
                    })
                    .await
                    .unwrap();
            }
            _ => {
                let reset = match reason {
                    "quota_soon" => Some(now + Duration::from_secs(10)),
                    "quota_just_reset" => Some(now - Duration::from_secs(10)),
                    "quota_later" => Some(now + Duration::from_secs(3600)),
                    "quota_stale" => Some(now - Duration::from_secs(3600)),
                    _ => None,
                };
                store
                    .apply_quota_access(QuotaAccessChange {
                        account_id: primary.clone(),
                        expected_revision: account.revision(),
                        state: QuotaState::exhausted(QuotaEvidence::UsageLimitReached, now, reset),
                    })
                    .await
                    .unwrap();
            }
        }
        let fallback = tokio::time::timeout(Duration::from_millis(500), select())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            fallback.metadata().provider_account_id().as_str(),
            "acct_subagent_b",
            "{reason}"
        );
        drop(fallback);
        leases.capacity.signals.lock().unwrap().clear();
        if reason == "deleted" {
            create_account(&store, primary.as_str()).await;
        }
        store.set_enabled(&primary, true).await.unwrap();
        store
            .apply_state_change(AccountStateChange {
                account_id: primary.clone(),
                expected_revision: store.account(primary.as_str()).unwrap().revision(),
                credential_state: CredentialState::Ready,
                observed_at: SystemTime::now(),
                error_reason: None,
                message: None,
            })
            .await
            .unwrap();
        store
            .apply_quota_access(QuotaAccessChange {
                account_id: primary.clone(),
                expected_revision: store.account(primary.as_str()).unwrap().revision(),
                state: QuotaState::allowed(SystemTime::now()),
            })
            .await
            .unwrap();
        assert_eq!(
            select()
                .await
                .unwrap()
                .metadata()
                .provider_account_id()
                .as_str(),
            if migrated {
                "acct_subagent_b"
            } else {
                "acct_subagent_a"
            },
            "{reason}",
        );
    }
}

#[tokio::test]
async fn preferred_spillover_stays_stable_and_concurrent_migration_claims_one_primary() {
    use gateway_core::account::AccountRuntimeSignals;

    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let leases = Arc::new(TestLeaseCoordinator::default());
    let affinity = Arc::new(MemorySessionAffinity::with_operation_delay(
        Duration::from_millis(5),
    ));
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        affinity.clone(),
        server.uri(),
        leases.clone(),
    );
    let select = || {
        provider.execute(
            planned_request("openai", turn_request("root", "stable-spillover")),
            preferred_context("req_stable_spillover", contract_account_scope()),
        )
    };
    drop(select().await.unwrap());
    create_account(&store, "acct_subagent_b").await;
    create_account(&store, "acct_provider_contract").await;
    let primary = ProviderAccountId::new("acct_subagent_a").unwrap();
    leases.capacity.signals.lock().unwrap().insert(
        primary.clone(),
        AccountRuntimeSignals {
            in_flight: 100,
            last_started_at: None,
            quota_reset_at: None,
            quota_reset_windows: None,
            quota_remaining_rank: None,
            rate_limited_until: None,
            failure_rate_basis_points: None,
            first_output_latency_ms: None,
        },
    );
    let fallback = select()
        .await
        .unwrap()
        .metadata()
        .provider_account_id()
        .clone();
    assert_ne!(fallback, primary);
    for _ in 0..4 {
        assert_eq!(
            select().await.unwrap().metadata().provider_account_id(),
            &fallback
        );
    }
    leases
        .busy_accounts
        .lock()
        .unwrap()
        .insert(fallback.clone());
    let alternate = select()
        .await
        .unwrap()
        .metadata()
        .provider_account_id()
        .clone();
    assert_ne!(alternate, fallback);
    assert_ne!(alternate, primary);
    leases.busy_accounts.lock().unwrap().clear();
    assert_eq!(
        select().await.unwrap().metadata().provider_account_id(),
        &fallback
    );
    leases.capacity.signals.lock().unwrap().clear();
    assert_eq!(
        select().await.unwrap().metadata().provider_account_id(),
        &primary
    );

    store.set_enabled(&primary, false).await.unwrap();
    let (first, second) = tokio::join!(select(), select());
    let migrated = first.unwrap().metadata().provider_account_id().clone();
    assert_ne!(migrated, primary);
    assert_eq!(second.unwrap().metadata().provider_account_id(), &migrated);
    store.set_enabled(&primary, true).await.unwrap();
    assert_eq!(
        select().await.unwrap().metadata().provider_account_id(),
        &migrated
    );
    assert_eq!(affinity.binding_count(), 1);
}
