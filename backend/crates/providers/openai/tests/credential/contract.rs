use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::executor::block_on;
use gateway_core::account::{
    AccountAttemptFeedback, AccountConcurrencyLimit, AccountErrorReason, AccountFeedbackStats,
    AccountSelectionPolicy, AccountStateChange, AccountWeight, CredentialState, OpaqueProviderData,
    ProviderAccount, ProviderAccountId, ProviderAccountStore as _, ProviderDeviceCodec,
    QuotaAccessChange, QuotaAccessState, QuotaEvidence, QuotaObservation, QuotaState,
    QuotaWriteOutcome, RotationStrategy,
};
use gateway_core::engine::continuation::{
    ContinuationBinding, NativeContinuationPin, PreviousResponseId,
};
use gateway_core::engine::{
    AccountAttemptContext, AttemptContext, ModelRequestId, RequestAttemptContext,
};
use gateway_core::lifecycle::CancellationToken;
use gateway_core::policy::ClientApiKeyId;
use gateway_core::provider_ports::{
    ProviderCooldownPort, ProviderSessionAffinityKey, ProviderSessionAffinityPort,
};
use gateway_core::routing::{
    ClientRoutingScope, FrozenAccountScope, ProviderKind, RuntimeAccount, RuntimeAccountDirectory,
};
use provider_openai::OFFICIAL_CODEX_BASE_URL;
use provider_openai::credential::{
    CodexAccountFailure, CodexCookiePolicy, CodexCredentialCodec, CodexCredentialQuotaService,
    CodexCredentialSelector, CodexDeviceCodec, CredentialSelectionError,
    ImportCodexOAuthCredential, SelectCodexCredential,
};
use provider_openai::transport::profile::{CodexWireProfile, CodexWireProfileState};
use secrecy::ExposeSecret;
use serde_json::json;
use url::Url;

use crate::support::{
    MemoryAccountStore, MemoryCooldownPort, MemorySessionAffinity, MemorySessionExclusions,
    TestLeaseCoordinator, account_policy, profile, secret,
};

fn create_account(store: &Arc<MemoryAccountStore>, id: &str, token: &str) {
    block_on(store.seed_oauth_credential(ImportCodexOAuthCredential {
        account_id: id.to_owned(),
        name: id.to_owned(),
        secret: secret(token),
        verified_account: profile(&format!("chatgpt-{id}")),
        next_refresh_at: Some(chrono::Utc::now() + chrono::Duration::minutes(30)),
        enabled: true,
    }));
}

fn contract_account_scope() -> Arc<FrozenAccountScope> {
    let provider = ProviderKind::new("openai").expect("provider");
    let accounts = [
        "acct_available",
        "acct_fallback",
        "acct_fallback_signal",
        "acct_first",
        "acct_missing",
        "acct_original",
        "acct_original_signal",
        "acct_other",
        "acct_primary",
        "acct_second",
    ]
    .into_iter()
    .map(|id| {
        (
            ProviderAccountId::new(id).expect("account"),
            RuntimeAccount::new(provider.clone(), BTreeSet::new()),
        )
    })
    .collect::<BTreeMap<_, _>>();
    Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(accounts)),
        ClientRoutingScope::all_accounts(),
    ))
}

fn attempt(excluded_accounts: BTreeSet<ProviderAccountId>) -> AttemptContext {
    attempt_with_required(excluded_accounts, None)
}

fn attempt_with_required(
    excluded_accounts: BTreeSet<ProviderAccountId>,
    required_account: Option<ProviderAccountId>,
) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_codex_contract").expect("request id"),
            ClientApiKeyId::new("key_codex_contract").expect("client key id"),
        ),
        NonZeroU32::new(1).expect("attempt"),
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::new(excluded_accounts, required_account, None)
            .with_account_scope(contract_account_scope()),
        None,
        CancellationToken::new(),
    )
}

fn round_robin_attempt() -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_codex_round_robin").expect("request id"),
            ClientApiKeyId::new("key_codex_contract").expect("client key id"),
        ),
        NonZeroU32::new(1).expect("attempt"),
        SystemTime::now() + Duration::from_secs(30),
        AccountSelectionPolicy::new(
            RotationStrategy::RoundRobin,
            NonZeroU32::new(2).expect("concurrency"),
            Duration::ZERO,
        ),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope()),
        None,
        CancellationToken::new(),
    )
}

fn selector(
    store: &Arc<MemoryAccountStore>,
    leases: Arc<TestLeaseCoordinator>,
) -> CodexCredentialSelector {
    selector_with_affinity(store, leases, Arc::new(MemorySessionAffinity::default()))
}

fn selector_with_affinity(
    store: &Arc<MemoryAccountStore>,
    leases: Arc<TestLeaseCoordinator>,
    session_affinity: Arc<MemorySessionAffinity>,
) -> CodexCredentialSelector {
    selector_with_runtime(
        store,
        leases,
        session_affinity,
        Arc::new(AccountFeedbackStats::default()),
        Arc::new(MemoryCooldownPort::new()),
    )
}

fn selector_with_runtime(
    store: &Arc<MemoryAccountStore>,
    leases: Arc<TestLeaseCoordinator>,
    session_affinity: Arc<MemorySessionAffinity>,
    account_feedback: Arc<AccountFeedbackStats>,
    cooldowns: Arc<dyn ProviderCooldownPort>,
) -> CodexCredentialSelector {
    let profile = CodexWireProfileState::new(CodexWireProfile {
        raw_user_agent: None,
        originator: "codex_cli_rs".to_owned(),
        codex_version: "0.144.0".to_owned(),
        desktop_version: "1.0.0".to_owned(),
        desktop_build: "1".to_owned(),
        os_type: "linux".to_owned(),
        os_version: "6.8".to_owned(),
        arch: "x86_64".to_owned(),
        terminal: "selector-contract".to_owned(),
        residency: None,
        location: Default::default(),
        verified_at: chrono::Utc::now(),
    });
    let http = reqwest::Client::builder().build().expect("HTTP client");
    let quota = Arc::new(CodexCredentialQuotaService::new(
        store.repository(),
        profile,
        http,
        OFFICIAL_CODEX_BASE_URL.to_owned(),
        cooldowns,
    ));
    CodexCredentialSelector::new(
        ProviderKind::new("openai").expect("provider"),
        store.repository(),
        leases,
        session_affinity,
        Arc::new(MemorySessionExclusions::default()),
        quota,
        account_feedback,
        CodexCookiePolicy::official().expect("official cookie policy"),
    )
}

fn persist_credential_state(
    store: &MemoryAccountStore,
    account: &ProviderAccount,
    credential_state: CredentialState,
) {
    block_on(store.apply_state_change(AccountStateChange {
        account_id: account.id().clone(),
        expected_revision: account.revision(),
        credential_state,
        observed_at: SystemTime::now(),
        error_reason: credential_state.error_reason(),
        message: None,
    }))
    .expect("persist credential state");
}

fn persist_quota_exhaustion(
    store: &MemoryAccountStore,
    account: &ProviderAccount,
    reset_at: Option<SystemTime>,
) {
    block_on(store.apply_quota_access(QuotaAccessChange {
        account_id: account.id().clone(),
        expected_revision: account.revision(),
        state: QuotaState::exhausted(
            QuotaEvidence::UsageLimitReached,
            SystemTime::now(),
            reset_at,
        ),
    }))
    .expect("persist quota exhaustion");
}

#[test]
fn codec_persists_tokens_as_plaintext_provider_json() {
    let encoded = CodexCredentialCodec::encode_new(
        &secret("literal-access-token"),
        &profile("chatgpt-literal"),
        Vec::new(),
    )
    .expect("encode plaintext credential");
    assert_eq!(
        encoded
            .expose_to_provider()
            .get("access_token")
            .and_then(serde_json::Value::as_str),
        Some("literal-access-token")
    );
    assert_eq!(
        encoded
            .expose_to_provider()
            .get("refresh_token")
            .and_then(serde_json::Value::as_str),
        Some("rt-literal-access-token")
    );
    let mut keys = encoded
        .expose_to_provider()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "access_token",
            "cookies",
            "installation_id",
            "principal",
            "refresh_token",
            "schema_version",
        ]
    );
}

#[test]
fn codec_reimport_preserves_existing_installation_id_for_the_same_principal() {
    let existing = CodexCredentialCodec::encode_new(
        &secret("existing-access-token"),
        &profile("chatgpt-stable-installation"),
        Vec::new(),
    )
    .expect("existing credential");
    let incoming = CodexCredentialCodec::encode_new(
        &secret("incoming-access-token"),
        &profile("chatgpt-stable-installation"),
        Vec::new(),
    )
    .expect("incoming credential");
    let existing_id = CodexCredentialCodec::decode_complete(&existing)
        .expect("existing data")
        .installation_id()
        .to_owned();

    let preserved = CodexCredentialCodec::preserve_installation_id(&incoming, &existing)
        .expect("preserve installation ID");
    let preserved = CodexCredentialCodec::decode_complete(&preserved).expect("preserved data");

    assert_eq!(preserved.installation_id(), existing_id);
    assert_eq!(
        preserved.oauth().expect("OAuth data").access_token,
        "incoming-access-token"
    );
}

#[test]
fn device_registry_codec_restores_only_installation_and_preserves_new_material() {
    let incoming = CodexCredentialCodec::encode_new(
        &secret("newly-authorized"),
        &profile("current-principal"),
        Vec::new(),
    )
    .expect("incoming credential");
    let installation = uuid::Uuid::new_v4().to_string();
    let restored = CodexDeviceCodec
        .with_installation_id(&incoming, &installation)
        .expect("restore durable installation");
    let expected = CodexCredentialCodec::decode_complete(&incoming).expect("incoming data");
    let restored_data = CodexCredentialCodec::decode_complete(&restored).expect("restored data");
    assert_eq!(
        CodexDeviceCodec.installation_id(&restored).expect("device"),
        installation,
    );
    let mut expected_json = serde_json::to_value(expected).expect("expected JSON");
    expected_json["installation_id"] = json!(installation);
    assert_eq!(
        serde_json::to_value(restored_data).expect("restored JSON"),
        expected_json,
    );
    assert!(
        CodexDeviceCodec
            .with_installation_id(&incoming, "invalid-device")
            .is_err()
    );
}

#[test]
fn codec_reimport_preserves_installation_id_without_principal_validation() {
    let existing = CodexCredentialCodec::encode_new(
        &secret("existing-access-token"),
        &profile("chatgpt-existing-principal"),
        Vec::new(),
    )
    .expect("existing credential");
    let incoming = CodexCredentialCodec::encode_new(
        &secret("incoming-access-token"),
        &profile("chatgpt-incoming-principal"),
        Vec::new(),
    )
    .expect("incoming credential");

    let existing_id = CodexCredentialCodec::decode_complete(&existing)
        .expect("existing data")
        .installation_id()
        .to_owned();
    let preserved = CodexCredentialCodec::preserve_installation_id(&incoming, &existing)
        .expect("preserve installation ID");
    let preserved = CodexCredentialCodec::decode_complete(&preserved).expect("preserved data");

    assert_eq!(preserved.installation_id(), existing_id);
    assert_eq!(
        preserved.oauth().expect("OAuth data").access_token,
        "incoming-access-token"
    );
}

#[test]
fn repository_round_trips_plaintext_runtime_secret() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let account = store.account("acct_primary").expect("account");
    let runtime = block_on(store.repository().load_runtime_credential(&account))
        .expect("load runtime credential");
    let oauth = runtime.authentication.oauth().expect("OAuth credential");
    assert_eq!(oauth.access_token.expose_secret(), "at-primary");
    assert_eq!(
        oauth
            .refresh_token
            .as_ref()
            .expect("refresh token")
            .expose_secret(),
        "rt-at-primary"
    );
}

#[test]
fn selector_uses_frozen_global_account_policy_for_lease() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let leases = Arc::new(TestLeaseCoordinator::default());
    let selector = selector(&store, Arc::clone(&leases));
    let attempt = attempt(BTreeSet::new());
    let lease =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect("select account");

    assert_eq!(lease.account_id().as_str(), "acct_primary");
    let installation_id = lease.installation_id();
    assert_eq!(
        uuid::Uuid::parse_str(installation_id)
            .expect("installation UUID")
            .get_version_num(),
        4
    );
    let runtime = block_on(store.repository().load_runtime_credential(lease.account()))
        .expect("runtime credential");
    assert_eq!(runtime.installation_id, installation_id);
    let requests = leases.requests.lock().expect("lease requests lock");
    assert_eq!(
        requests[0].provider_kind(),
        &ProviderKind::new("openai").expect("provider")
    );
    assert_eq!(requests[0].account_id(), lease.account_id());
    assert_eq!(
        requests[0].credential_revision(),
        lease.account().revision()
    );
    assert_eq!(requests[0].max_concurrent().get(), 2);
    assert_eq!(requests[0].request_interval(), Duration::from_millis(10));
    assert_eq!(requests[0].deadline(), attempt.deadline());
}

#[test]
fn selector_uses_the_account_concurrency_override_for_the_redis_lease() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    store.set_scheduling(
        "acct_primary",
        Some(AccountConcurrencyLimit::new(7).expect("concurrency override")),
        AccountWeight::DEFAULT,
    );
    let leases = Arc::new(TestLeaseCoordinator::default());
    let selector = selector(&store, Arc::clone(&leases));
    let attempt = attempt(BTreeSet::new());

    block_on(selector.select(&SelectCodexCredential {
        upstream_model: "gpt-5.4",
        request_url:
            &Url::parse("https://chatgpt.com/backend-api/codex/responses").expect("request URL"),
        attempt: &attempt,
        session_affinity_key: None,
    }))
    .expect("select account");

    let requests = leases.requests.lock().expect("lease requests lock");
    assert_eq!(requests[0].max_concurrent().get(), 7);
}

#[test]
fn selector_round_robin_cursor_advances_across_requests() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_first", "at-first");
    create_account(&store, "acct_second", "at-second");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let request_url =
        Url::parse("https://chatgpt.com/backend-api/codex/responses").expect("request URL");
    let mut selected = Vec::new();

    for _ in 0..4 {
        let attempt = round_robin_attempt();
        let lease = block_on(selector.select(&SelectCodexCredential {
            upstream_model: "gpt-5.4",
            request_url: &request_url,
            attempt: &attempt,
            session_affinity_key: None,
        }))
        .expect("select round robin account");
        selected.push(lease.account_id().as_str().to_owned());
    }

    assert_eq!(
        selected,
        ["acct_first", "acct_second", "acct_first", "acct_second"]
    );
}

#[tokio::test]
async fn selector_should_claim_the_initial_session_account_before_upstream_send() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_first", "at-first");
    create_account(&store, "acct_second", "at-second");
    let affinity = Arc::new(MemorySessionAffinity::default());
    let selector = selector_with_affinity(
        &store,
        Arc::new(TestLeaseCoordinator::default()),
        Arc::clone(&affinity),
    );
    let provider = ProviderKind::new("openai").expect("provider");
    let key = ProviderSessionAffinityKey::try_new("initial-claim").expect("affinity key");
    let request_attempt = attempt(BTreeSet::new());

    let selected = selector
        .select(&SelectCodexCredential {
            upstream_model: "gpt-5.4",
            request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                .expect("request URL"),
            attempt: &request_attempt,
            session_affinity_key: Some(&key),
        })
        .await
        .expect("select initial account");

    assert_eq!(
        affinity
            .load(&provider, &key)
            .await
            .expect("load claimed affinity"),
        Some(selected.account_id().clone())
    );
}

#[tokio::test]
async fn record_success_should_not_overwrite_a_newer_session_winner() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_first", "at-first");
    create_account(&store, "acct_second", "at-second");
    let affinity = Arc::new(MemorySessionAffinity::default());
    let selector = selector_with_affinity(
        &store,
        Arc::new(TestLeaseCoordinator::default()),
        Arc::clone(&affinity),
    );
    let provider = ProviderKind::new("openai").expect("provider");
    let key = ProviderSessionAffinityKey::try_new("late-success").expect("affinity key");
    let first = store.account("acct_first").expect("first account");
    let second = ProviderAccountId::new("acct_second").expect("second account");
    affinity
        .bind(&provider, &key, &second, Duration::from_secs(60))
        .await
        .expect("seed newer affinity");

    selector
        .record_success(&first, Some(&key), first.id())
        .await;

    assert_eq!(
        affinity
            .load(&provider, &key)
            .await
            .expect("load preserved affinity"),
        Some(second)
    );
}

#[tokio::test]
async fn selector_should_reuse_and_renew_the_account_bound_to_the_same_session() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_first", "at-first");
    create_account(&store, "acct_second", "at-second");
    let affinity = Arc::new(MemorySessionAffinity::default());
    let selector = selector_with_affinity(
        &store,
        Arc::new(TestLeaseCoordinator::default()),
        Arc::clone(&affinity),
    );
    let key = ProviderSessionAffinityKey::try_new("same-session").expect("affinity key");
    let request_url =
        Url::parse("https://chatgpt.com/backend-api/codex/responses").expect("request URL");
    let first_attempt = attempt(BTreeSet::new());
    let first = selector
        .select(&SelectCodexCredential {
            upstream_model: "gpt-5.4",
            request_url: &request_url,
            attempt: &first_attempt,
            session_affinity_key: Some(&key),
        })
        .await
        .expect("select first account");
    selector
        .record_success(first.account(), Some(&key), first.account_id())
        .await;
    let first_account = first.account_id().clone();

    let second_attempt = attempt(BTreeSet::new());
    let second = selector
        .select(&SelectCodexCredential {
            upstream_model: "gpt-5.4",
            request_url: &request_url,
            attempt: &second_attempt,
            session_affinity_key: Some(&key),
        })
        .await
        .expect("select bound account");

    assert_eq!(
        (
            second.account_id().as_str(),
            second.affinity_hit(),
            second.escape_reason(),
            second.account_switch(),
        ),
        (first_account.as_str(), true, None, false)
    );
    assert_eq!(
        affinity
            .load(&ProviderKind::new("openai").expect("provider"), &key)
            .await
            .expect("load affinity"),
        Some(first_account)
    );
    assert_eq!(
        affinity.renewal_ttls(),
        vec![Duration::from_secs(24 * 60 * 60); 2],
        "successful response and next selection both renew the binding"
    );
}

#[tokio::test]
async fn selector_should_replace_a_busy_affinity_binding_after_the_fallback_succeeds() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_first", "at-first");
    create_account(&store, "acct_second", "at-second");
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases
        .busy_accounts
        .lock()
        .expect("busy account lock")
        .insert(ProviderAccountId::new("acct_first").expect("account"));
    let affinity = Arc::new(MemorySessionAffinity::default());
    let provider = ProviderKind::new("openai").expect("provider");
    let key = ProviderSessionAffinityKey::try_new("busy-session").expect("affinity key");
    let bound = ProviderAccountId::new("acct_first").expect("bound account");
    affinity
        .bind(&provider, &key, &bound, Duration::from_secs(60))
        .await
        .expect("seed affinity");
    let selector = selector_with_affinity(&store, leases, Arc::clone(&affinity));
    let request_url =
        Url::parse("https://chatgpt.com/backend-api/codex/responses").expect("request URL");
    let request_attempt = attempt(BTreeSet::new());

    let selected = selector
        .select(&SelectCodexCredential {
            upstream_model: "gpt-5.4",
            request_url: &request_url,
            attempt: &request_attempt,
            session_affinity_key: Some(&key),
        })
        .await
        .expect("select fallback account");
    selector
        .record_success(selected.account(), Some(&key), &bound)
        .await;

    assert_eq!(
        (
            selected.account_id().as_str(),
            selected.affinity_hit(),
            selected.escape_reason(),
            selected.account_switch(),
        ),
        ("acct_second", false, Some("lease_saturated"), true)
    );
    assert_eq!(
        affinity
            .load(&provider, &key)
            .await
            .expect("load replaced affinity"),
        Some(ProviderAccountId::new("acct_second").expect("second account"))
    );
}

#[tokio::test]
async fn selector_should_prefer_session_over_weight_and_soft_health() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_first", "at-first");
    create_account(&store, "acct_second", "at-second");
    store.set_scheduling(
        "acct_second",
        None,
        AccountWeight::new(100).expect("weight"),
    );
    let affinity = Arc::new(MemorySessionAffinity::default());
    let provider = ProviderKind::new("openai").expect("provider");
    let key = ProviderSessionAffinityKey::try_new("unhealthy-session").expect("affinity key");
    let bound = ProviderAccountId::new("acct_first").expect("bound account");
    affinity
        .bind(&provider, &key, &bound, Duration::from_secs(60))
        .await
        .expect("seed affinity");
    let account_feedback = Arc::new(AccountFeedbackStats::default());
    let selector = selector_with_runtime(
        &store,
        Arc::new(TestLeaseCoordinator::default()),
        affinity,
        Arc::clone(&account_feedback),
        Arc::new(MemoryCooldownPort::new()),
    );
    for _ in 0..4 {
        account_feedback.report(
            &provider,
            &bound,
            AccountAttemptFeedback::Failed {
                first_output_ms: None,
            },
        );
    }
    let request_url =
        Url::parse("https://chatgpt.com/backend-api/codex/responses").expect("request URL");
    let request_attempt = attempt(BTreeSet::new());

    let selected = selector
        .select(&SelectCodexCredential {
            upstream_model: "gpt-5.4",
            request_url: &request_url,
            attempt: &request_attempt,
            session_affinity_key: Some(&key),
        })
        .await
        .expect("select bound account");

    assert_eq!(
        (
            selected.account_id().as_str(),
            selected.affinity_hit(),
            selected.escape_reason(),
            selected.account_switch(),
        ),
        ("acct_first", true, None, false)
    );
}

#[tokio::test]
async fn selector_should_escape_a_quota_exhausted_affinity_account() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_first", "at-first");
    create_account(&store, "acct_second", "at-second");
    let first = store.account("acct_first").expect("first account");
    persist_quota_exhaustion(&store, &first, None);
    let affinity = Arc::new(MemorySessionAffinity::default());
    let provider = ProviderKind::new("openai").expect("provider");
    let key = ProviderSessionAffinityKey::try_new("quota-session").expect("affinity key");
    affinity
        .bind(&provider, &key, first.id(), Duration::from_secs(60))
        .await
        .expect("seed affinity");
    let selector =
        selector_with_affinity(&store, Arc::new(TestLeaseCoordinator::default()), affinity);
    let request_url =
        Url::parse("https://chatgpt.com/backend-api/codex/responses").expect("request URL");
    let request_attempt = attempt(BTreeSet::new());

    let selected = selector
        .select(&SelectCodexCredential {
            upstream_model: "gpt-5.4",
            request_url: &request_url,
            attempt: &request_attempt,
            session_affinity_key: Some(&key),
        })
        .await
        .expect("select fallback account");

    assert_eq!(
        (
            selected.account_id().as_str(),
            selected.affinity_hit(),
            selected.escape_reason(),
            selected.account_switch(),
        ),
        ("acct_second", false, Some("quota_exhausted"), true)
    );
}

#[test]
fn selector_honors_attempt_local_account_exclusion() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_first", "at-first");
    create_account(&store, "acct_second", "at-second");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let attempt = attempt(BTreeSet::from([
        ProviderAccountId::new("acct_first").expect("account id")
    ]));
    let lease =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect("select non-excluded account");
    assert_eq!(lease.account_id().as_str(), "acct_second");
}

#[test]
fn selector_uses_only_the_required_account() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_first", "at-first");
    create_account(&store, "acct_second", "at-second");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let required = ProviderAccountId::new("acct_second").expect("account id");
    let attempt = attempt_with_required(BTreeSet::new(), Some(required.clone()));
    let lease =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect("select required account");
    assert_eq!(lease.account_id(), &required);
}

#[test]
fn unavailable_required_account_never_falls_back() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_available", "at-available");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let attempt = attempt_with_required(
        BTreeSet::new(),
        Some(ProviderAccountId::new("acct_missing").expect("account id")),
    );
    let error =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect_err("missing required account must not fall back");
    assert!(matches!(
        error,
        CredentialSelectionError::NoEligibleCredential
    ));
}

#[tokio::test]
async fn diagnostic_required_account_boundaries_never_select_an_enabled_alternative() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    create_account(&store, "acct_available", "at-available");
    let disabled = ProviderAccountId::new("acct_primary").expect("disabled account");
    store.set_enabled(&disabled, false).await.expect("disable");
    let leases = Arc::new(TestLeaseCoordinator::default());
    let selector = selector(&store, Arc::clone(&leases));
    let diagnostic = |required, excluded| {
        AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_diagnostic_boundary").expect("request"),
                ClientApiKeyId::new("key_codex_contract").expect("key"),
            ),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(30),
            account_policy(),
            AccountAttemptContext::diagnostic(excluded, required, None),
            None,
            CancellationToken::new(),
        )
    };
    let cases = [
        (
            "missing diagnostic account",
            diagnostic(
                ProviderAccountId::new("acct_missing").expect("missing account"),
                BTreeSet::new(),
            ),
        ),
        (
            "excluded diagnostic account",
            diagnostic(disabled.clone(), BTreeSet::from([disabled.clone()])),
        ),
        (
            "ordinary required disabled account",
            attempt_with_required(BTreeSet::new(), Some(disabled.clone())),
        ),
    ];
    for (case, attempt) in cases {
        assert!(
            matches!(
                capacity_select(&selector, &attempt, None).await,
                Err(CredentialSelectionError::NoEligibleCredential)
            ),
            "{case}"
        );
        assert!(leases.requests.lock().expect("leases").is_empty(), "{case}");
    }
    assert!(!store.account(disabled.as_str()).expect("account").enabled());
}

#[test]
fn selector_returns_capacity_error_when_every_redis_lease_is_busy() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let leases = Arc::new(TestLeaseCoordinator::default());
    *leases.busy.lock().expect("lease busy lock") = true;
    let selector = selector(&store, leases);
    let attempt = attempt(BTreeSet::new());
    let error =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect_err("busy lease must reject selection");
    assert!(matches!(
        error,
        CredentialSelectionError::CapacityUnavailable {
            retry_after: Some(_)
        }
    ));
}

#[test]
fn credential_expired_failure_marks_unified_account_expired() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let attempt = attempt(BTreeSet::new());
    let lease =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect("select account");
    block_on(selector.record_failure(
        lease.account(),
        CodexAccountFailure::CredentialExpired,
        None,
    ))
    .expect("record credential expiry");
    assert_eq!(
        store
            .account("acct_primary")
            .expect("account")
            .credential_state(),
        CredentialState::Expired
    );
    let account = store.account("acct_primary").expect("expired account");
    assert_eq!(
        account.last_error_reason(),
        Some(AccountErrorReason::AccessTokenExpired)
    );
    assert_eq!(account.last_error_message(), None);
}

#[test]
fn credential_expired_failure_keeps_expired_oauth_for_bounded_refresh_recovery() {
    let store = Arc::new(MemoryAccountStore::default());
    let mut expired_profile = profile("chatgpt-acct_primary");
    expired_profile.access_token_expires_at =
        Some(chrono::Utc::now() - chrono::Duration::minutes(1));
    block_on(store.seed_oauth_credential(ImportCodexOAuthCredential {
        account_id: "acct_primary".to_owned(),
        name: "acct_primary".to_owned(),
        secret: secret("at-primary"),
        verified_account: expired_profile,
        next_refresh_at: Some(chrono::Utc::now() + chrono::Duration::minutes(10)),
        enabled: true,
    }));
    let account = store.account("acct_primary").expect("account");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));

    block_on(selector.record_failure(
        &account,
        CodexAccountFailure::CredentialExpired,
        Some("token_expired".to_owned()),
    ))
    .expect("record credential expiry");

    let retained = store
        .account("acct_primary")
        .expect("account retained for refresh");
    assert_eq!(retained.credential_state(), CredentialState::Expired);
    assert!(retained.needs_authentication_refresh());
    assert!(retained.enabled());
    assert_eq!(
        retained.last_error_reason(),
        Some(AccountErrorReason::AccessTokenExpired)
    );
    assert_eq!(retained.last_error_message(), Some("token_expired"));
}

#[test]
fn revoked_credentials_do_not_enter_automatic_refresh_recovery() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_revoked", "at-revoked");
    let account = store.account("acct_revoked").expect("account");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    block_on(selector.record_failure(&account, CodexAccountFailure::CredentialRevoked, None))
        .expect("record revocation");
    let current = store.account("acct_revoked").expect("account");
    assert!(current.enabled());
    assert_eq!(current.credential_state(), CredentialState::Expired);
    assert_eq!(
        current.last_error_reason(),
        Some(AccountErrorReason::CredentialExpired)
    );
    assert!(!current.needs_authentication_refresh());
}

#[test]
fn rate_limited_failure_records_runtime_cooldown_without_changing_persisted_facts() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let cooldowns = Arc::new(MemoryCooldownPort::new());
    let selector = selector_with_runtime(
        &store,
        Arc::new(TestLeaseCoordinator::default()),
        Arc::new(MemorySessionAffinity::default()),
        Arc::new(AccountFeedbackStats::default()),
        Arc::clone(&cooldowns) as Arc<dyn ProviderCooldownPort>,
    );
    let attempt = attempt(BTreeSet::new());
    let lease =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect("select account");

    block_on(selector.record_failure(
        lease.account(),
        CodexAccountFailure::RateLimited {
            retry_after: Some(Duration::from_secs(30)),
        },
        None,
    ))
    .expect("record rate-limit failure");

    let account = store.account("acct_primary").expect("account");
    assert_eq!(account.credential_state(), CredentialState::Ready);
    assert_eq!(account.quota().access(), QuotaAccessState::Unknown);
    assert!(
        store.quota_json("acct_primary").is_none(),
        "429 must not synthesize quota window"
    );
    let cooldown = block_on(cooldowns.read(account.id())).expect("read cooldown");
    assert!(
        cooldown.is_some_and(|cooling| cooling.until() > SystemTime::now()),
        "429 must record a Redis cooldown expiring in the future"
    );
}

#[tokio::test]
async fn usage_limit_exhaustion_marks_quota_exhausted_without_usage_probe() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let account = store.account("acct_primary").expect("account");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));

    selector
        .record_failure(
            &account,
            CodexAccountFailure::UsageLimitExhausted {
                reset_at: Some(SystemTime::now() + Duration::from_secs(30)),
            },
            None,
        )
        .await
        .expect("record usage-limit exhaustion");

    let account = store.account("acct_primary").expect("persisted account");
    assert_eq!(account.quota().access(), QuotaAccessState::Exhausted);
    assert_eq!(
        account.quota().evidence(),
        Some(QuotaEvidence::UsageLimitReached)
    );
    assert_eq!(store.quota_reads(), 0);
}

#[test]
fn rate_limited_failure_does_not_downgrade_persisted_quota_exhaustion() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let account = store.account("acct_primary").expect("account");
    persist_quota_exhaustion(&store, &account, None);
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));

    block_on(selector.record_failure(
        &account,
        CodexAccountFailure::RateLimited {
            retry_after: Some(Duration::from_secs(30)),
        },
        None,
    ))
    .expect("record rate-limit failure");

    assert_eq!(
        store
            .account("acct_primary")
            .expect("persisted account")
            .quota()
            .access(),
        QuotaAccessState::Exhausted
    );
}

#[test]
fn rate_limited_failure_does_not_consult_stale_quota_snapshot() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let account = store.account("acct_primary").expect("account");
    let cooldowns = Arc::new(MemoryCooldownPort::new());
    let selector = selector_with_runtime(
        &store,
        Arc::new(TestLeaseCoordinator::default()),
        Arc::new(MemorySessionAffinity::default()),
        Arc::new(AccountFeedbackStats::default()),
        Arc::clone(&cooldowns) as Arc<dyn ProviderCooldownPort>,
    );

    block_on(selector.record_failure(
        &account,
        CodexAccountFailure::RateLimited {
            retry_after: Some(Duration::from_secs(30)),
        },
        None,
    ))
    .expect("record rate-limit failure");

    assert_eq!(
        store
            .account("acct_primary")
            .expect("persisted account")
            .credential_state(),
        CredentialState::Ready
    );
    // 429 临时限流写入 Redis 冷却，不读写 quota JSON。
    assert_eq!(store.quota_reads(), 0);
    let cooldown = block_on(cooldowns.read(account.id())).expect("read cooldown");
    assert!(
        cooldown.is_some_and(|cooling| cooling.until() > SystemTime::now()),
        "429 must record a Redis cooldown"
    );
}

#[test]
fn rate_limited_failure_does_not_overwrite_stale_authentication_state() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let account = store.account("acct_primary").expect("account");
    persist_credential_state(&store, &account, CredentialState::Invalid);
    let current = store.account("acct_primary").expect("invalid account");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));

    block_on(selector.record_failure(
        &current,
        CodexAccountFailure::RateLimited {
            retry_after: Some(Duration::from_secs(30)),
        },
        None,
    ))
    .expect("record rate-limit failure");

    assert_eq!(
        store
            .account("acct_primary")
            .expect("persisted account")
            .credential_state(),
        CredentialState::Invalid
    );
}

#[test]
fn successful_upstream_response_recovers_non_quota_terminal_states() {
    for stale in [
        CredentialState::Expired,
        CredentialState::Invalid,
        CredentialState::Banned,
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_primary", "at-primary");
        let account = store.account("acct_primary").expect("account");
        persist_credential_state(&store, &account, stale);
        let current = store.account("acct_primary").expect("stale account");
        let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));

        block_on(selector.record_success(&current, None, current.id()));

        assert_eq!(
            store
                .account("acct_primary")
                .expect("recovered account")
                .credential_state(),
            CredentialState::Ready,
            "stale state {stale:?}",
        );
    }
}

#[tokio::test]
async fn diagnostic_success_clears_ready_errors_only_on_the_selected_account() {
    for enabled in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        for id in ["acct_primary", "acct_other"] {
            create_account(&store, id, "at-primary");
            let account = store.account(id).expect("account");
            store
                .apply_state_change(AccountStateChange {
                    account_id: account.id().clone(),
                    expected_revision: account.revision(),
                    credential_state: CredentialState::Ready,
                    observed_at: SystemTime::now(),
                    error_reason: Some(AccountErrorReason::AccessTokenExpired),
                    message: Some("token_expired".to_owned()),
                })
                .await
                .expect("seed recoverable error");
        }
        let account_id = ProviderAccountId::new("acct_primary").expect("account ID");
        store
            .set_enabled(&account_id, enabled)
            .await
            .expect("set account enabled flag");
        let before = store.account("acct_primary").expect("selected account");
        let other = store.account("acct_other").expect("other account");
        let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));

        selector.record_diagnostic_success(&before).await;

        let after = store.account("acct_primary").expect("diagnosed account");
        assert_eq!(after.enabled(), enabled);
        assert_eq!(after.revision(), before.revision());
        assert_eq!(after.credential_state(), CredentialState::Ready);
        assert_eq!(after.last_error_reason(), None);
        assert_eq!(after.last_error_message(), None);
        assert_eq!(store.account("acct_other").expect("other account"), other);
        assert_eq!(store.quota_reads(), 0);
    }
}

#[tokio::test]
async fn diagnostic_success_cannot_overwrite_newer_credential_revision() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let before = store.account("acct_primary").expect("account");
    let data = store
        .repository()
        .load_complete_data(&before)
        .await
        .expect("load credentials");
    store
        .repository()
        .compare_and_swap_data(&before, data)
        .await
        .expect("advance credential revision");
    let current = store.account("acct_primary").expect("current account");
    persist_credential_state(&store, &current, CredentialState::Invalid);
    let current = store.account("acct_primary").expect("invalid account");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));

    selector.record_diagnostic_success(&before).await;

    assert_eq!(
        store.account("acct_primary").expect("fenced account"),
        current
    );
}

#[test]
fn elapsed_quota_reset_remains_blocked_until_authoritative_recovery() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let account = store.account("acct_primary").expect("account");
    persist_quota_exhaustion(
        &store,
        &account,
        Some(SystemTime::now() - Duration::from_secs(1)),
    );
    let blocked = store.account("acct_primary").expect("blocked account");
    assert!(blocked.quota().is_exhausted());
    assert_eq!(
        blocked
            .status_projection(SystemTime::now(), None)
            .status
            .as_str(),
        "quota_exhausted"
    );
}

#[test]
fn rate_limited_failures_for_distinct_accounts_do_not_conflict() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_first", "at-first");
    create_account(&store, "acct_second", "at-second");
    let first = store.account("acct_first").expect("first account");
    let second = store.account("acct_second").expect("second account");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));

    block_on(async {
        let (first_result, second_result) = futures::join!(
            selector.record_failure(
                &first,
                CodexAccountFailure::RateLimited {
                    retry_after: Some(Duration::from_secs(30))
                },
                None,
            ),
            selector.record_failure(
                &second,
                CodexAccountFailure::RateLimited {
                    retry_after: Some(Duration::from_secs(30))
                },
                None,
            ),
        );
        first_result.expect("record first account failure");
        second_result.expect("record second account failure");
    });

    assert_eq!(
        [
            store
                .account("acct_first")
                .expect("persisted first account")
                .credential_state(),
            store
                .account("acct_second")
                .expect("persisted second account")
                .credential_state(),
        ],
        [CredentialState::Ready; 2]
    );
}

#[test]
fn native_continuation_surfaces_the_original_accounts_quota_status_to_the_coordinator() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_original", "at-original");
    create_account(&store, "acct_fallback", "at-fallback");
    let strict_selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let original = store.account("acct_original").expect("original account");
    block_on(strict_selector.record_failure(&original, CodexAccountFailure::QuotaExhausted, None))
        .expect("mark original account exhausted");

    let selector = selector_with_runtime(
        &store,
        Arc::new(TestLeaseCoordinator::default()),
        Arc::new(MemorySessionAffinity::default()),
        Arc::new(AccountFeedbackStats::default()),
        Arc::new(MemoryCooldownPort::new()),
    );
    let continuation = NativeContinuationPin::new(
        PreviousResponseId::new("previous-response"),
        PreviousResponseId::new("upstream-response"),
        ClientApiKeyId::new("key_codex_contract").expect("client key id"),
        ProviderKind::new("openai").expect("provider"),
        original.id().clone(),
    );
    let attempt = AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_native_continuation").expect("request id"),
            ClientApiKeyId::new("key_codex_contract").expect("client key id"),
        ),
        NonZeroU32::new(1).expect("attempt"),
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope()),
        Some(ContinuationBinding::Pinned(continuation)),
        CancellationToken::new(),
    );
    let error =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect_err("the selector must surface the unavailable native account");

    assert!(matches!(
        error,
        CredentialSelectionError::NoEligibleCredential
    ));
}

#[test]
fn native_continuation_surfaces_the_original_accounts_quota_signal_to_the_coordinator() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_original_signal", "at-original-signal");
    create_account(&store, "acct_fallback_signal", "at-fallback-signal");
    let original = store
        .account("acct_original_signal")
        .expect("original account");
    let quota = json!({
        "rate_limit": {
            "allowed": false,
            "limit_reached": true,
            "primary_window": {"used_percent": 98}
        }
    });
    let observed_at = SystemTime::now();
    let outcome = block_on(store.compare_and_swap_quota(QuotaObservation {
        plan_type: None,
        account_id: original.id().clone(),
        expected_revision: original.revision(),
        quota: OpaqueProviderData::new(quota.as_object().expect("quota object").clone()),
        observed_at,
        state: QuotaState::exhausted(QuotaEvidence::ProviderDenied, observed_at, None),
    }))
    .expect("persist quota signal");
    assert!(matches!(outcome, QuotaWriteOutcome::Updated));

    let selector = selector_with_runtime(
        &store,
        Arc::new(TestLeaseCoordinator::default()),
        Arc::new(MemorySessionAffinity::default()),
        Arc::new(AccountFeedbackStats::default()),
        Arc::new(MemoryCooldownPort::new()),
    );
    let continuation = NativeContinuationPin::new(
        PreviousResponseId::new("previous-response"),
        PreviousResponseId::new("upstream-response"),
        ClientApiKeyId::new("key_codex_contract").expect("client key id"),
        ProviderKind::new("openai").expect("provider"),
        original.id().clone(),
    );
    let attempt = AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_native_continuation_signal").expect("request id"),
            ClientApiKeyId::new("key_codex_contract").expect("client key id"),
        ),
        NonZeroU32::new(1).expect("attempt"),
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope()),
        Some(ContinuationBinding::Pinned(continuation)),
        CancellationToken::new(),
    );
    let error =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect_err("the selector must surface the quota-limited native account");

    assert!(matches!(
        error,
        CredentialSelectionError::NoEligibleCredential
    ));
}

#[test]
fn identity_verification_failure_isolates_only_selected_account() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    create_account(&store, "acct_other", "at-other");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let attempt = attempt(BTreeSet::new());
    let lease =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect("select account");

    block_on(selector.record_failure(
        lease.account(),
        CodexAccountFailure::IdentityVerificationRequired,
        None,
    ))
    .expect("record identity verification failure");

    assert_eq!(
        store
            .account(lease.account_id().as_str())
            .expect("selected account")
            .credential_state(),
        CredentialState::Invalid
    );
    let other = if lease.account_id().as_str() == "acct_primary" {
        "acct_other"
    } else {
        "acct_primary"
    };
    assert_eq!(
        store
            .account(other)
            .expect("other account")
            .credential_state(),
        CredentialState::Ready
    );
}

#[test]
fn cloudflare_challenge_does_not_change_persisted_account_facts() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let attempt = attempt(BTreeSet::new());
    let lease =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect("select account");

    block_on(selector.record_failure(
        lease.account(),
        CodexAccountFailure::CloudflareChallenge { retry_after: None },
        None,
    ))
    .expect("record challenge");

    assert_eq!(
        store
            .account("acct_primary")
            .expect("account")
            .credential_state(),
        CredentialState::Ready
    );
    block_on(selector.record_success(lease.account(), None, lease.account_id()));
}

#[test]
fn repeated_cloudflare_path_block_marks_only_the_affected_account_invalid() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    create_account(&store, "acct_other", "at-other");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let attempt = attempt_with_required(
        BTreeSet::new(),
        Some(ProviderAccountId::new("acct_primary").expect("account id")),
    );
    let lease =
        block_on(
            selector.select(&SelectCodexCredential {
                upstream_model: "gpt-5.4",
                request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses")
                    .expect("request URL"),
                attempt: &attempt,
                session_affinity_key: None,
            }),
        )
        .expect("select account");

    for _ in 0..3 {
        block_on(selector.record_failure(
            lease.account(),
            CodexAccountFailure::CloudflarePathBlocked,
            None,
        ))
        .expect("record path block");
    }

    assert_eq!(
        store
            .account("acct_primary")
            .expect("affected account")
            .credential_state(),
        CredentialState::Invalid
    );
    assert_eq!(
        store
            .account("acct_other")
            .expect("other account")
            .credential_state(),
        CredentialState::Ready
    );
}

#[test]
fn cloudflare_challenge_expires_provider_owned_cookies_at_cooldown_boundary() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let required = ProviderAccountId::new("acct_primary").expect("account id");
    let request_url =
        Url::parse("https://chatgpt.com/backend-api/codex/responses").expect("request URL");
    let first_attempt = attempt_with_required(BTreeSet::new(), Some(required.clone()));
    let first = block_on(selector.select(&SelectCodexCredential {
        upstream_model: "gpt-5.4",
        request_url: &request_url,
        attempt: &first_attempt,
        session_affinity_key: None,
    }))
    .expect("select account");
    block_on(selector.capture_response_cookies(
        first.account(),
        &request_url,
        &["cf_clearance=old; Path=/; Domain=chatgpt.com; Secure; Max-Age=3600".to_owned()],
    ))
    .expect("capture cookie");

    let second_attempt = attempt_with_required(BTreeSet::new(), Some(required));
    let second = block_on(selector.select(&SelectCodexCredential {
        upstream_model: "gpt-5.4",
        request_url: &request_url,
        attempt: &second_attempt,
        session_affinity_key: None,
    }))
    .expect("select revised account");
    block_on(selector.record_failure(
        second.account(),
        CodexAccountFailure::CloudflareChallenge { retry_after: None },
        None,
    ))
    .expect("record challenge");

    let account = store.account("acct_primary").expect("account");
    let data = block_on(store.repository().load_complete_data(&account)).expect("credential data");
    assert_eq!(data.cookies().len(), 1);
    assert!(data.cookies()[0].expires_at.is_some_and(|expires_at| {
        let expires_at = SystemTime::from(expires_at);
        expires_at > SystemTime::now() && expires_at <= SystemTime::now() + Duration::from_secs(120)
    }));
}

#[test]
fn cloudflare_path_block_deletes_provider_owned_cookies() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let required = ProviderAccountId::new("acct_primary").expect("account id");
    let request_url =
        Url::parse("https://chatgpt.com/backend-api/codex/responses").expect("request URL");
    let first_attempt = attempt_with_required(BTreeSet::new(), Some(required.clone()));
    let first = block_on(selector.select(&SelectCodexCredential {
        upstream_model: "gpt-5.4",
        request_url: &request_url,
        attempt: &first_attempt,
        session_affinity_key: None,
    }))
    .expect("select account");
    block_on(selector.capture_response_cookies(
        first.account(),
        &request_url,
        &["__cf_bm=old; Path=/; Domain=chatgpt.com; Secure; Max-Age=3600".to_owned()],
    ))
    .expect("capture cookie");

    let second_attempt = attempt_with_required(BTreeSet::new(), Some(required));
    let second = block_on(selector.select(&SelectCodexCredential {
        upstream_model: "gpt-5.4",
        request_url: &request_url,
        attempt: &second_attempt,
        session_affinity_key: None,
    }))
    .expect("select revised account");
    block_on(selector.record_failure(
        second.account(),
        CodexAccountFailure::CloudflarePathBlocked,
        None,
    ))
    .expect("record path block");

    let account = store.account("acct_primary").expect("account");
    let data = block_on(store.repository().load_complete_data(&account)).expect("credential data");
    assert!(data.cookies().is_empty());
}

#[test]
fn response_cookie_rotation_returns_a_current_account_for_later_fenced_writes() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let selector = selector(&store, Arc::new(TestLeaseCoordinator::default()));
    let request_url =
        Url::parse("https://chatgpt.com/backend-api/codex/responses").expect("request URL");
    let attempt = attempt_with_required(
        BTreeSet::new(),
        Some(ProviderAccountId::new("acct_primary").expect("account id")),
    );
    let lease = block_on(selector.select(&SelectCodexCredential {
        upstream_model: "gpt-5.4",
        request_url: &request_url,
        attempt: &attempt,
        session_affinity_key: None,
    }))
    .expect("select account");

    let outcome = block_on(selector.capture_response_cookies(
        lease.account(),
        &request_url,
        &["cf_clearance=updated; Path=/; Domain=chatgpt.com; Secure; Max-Age=3600".to_owned()],
    ))
    .expect("capture response cookie");
    let current = block_on(selector.current_account(lease.account_id())).expect("current account");

    assert_eq!(outcome.credential_revision, Some(current.revision().get()));
    assert_ne!(current.revision(), lease.account().revision());
    block_on(selector.record_failure(&current, CodexAccountFailure::QuotaExhausted, None))
        .expect("record failure with current revision");
    assert_eq!(
        store
            .account("acct_primary")
            .expect("updated account")
            .quota()
            .access(),
        QuotaAccessState::Exhausted
    );
}

fn capacity_tuning() -> gateway_core::routing::RequestTuning {
    gateway_core::routing::RequestTuning {
        account_busy_wait_enabled: true,
        ..Default::default()
    }
}

fn capacity_handle(ids: &[&str], limit: u32) -> gateway_core::runtime::AccountConcurrencyHandle {
    gateway_core::runtime::AccountConcurrencyHandle::new(
        gateway_core::routing::ConfigRevision::new(1).unwrap(),
        ids.iter()
            .map(|id| ((*id).to_owned(), NonZeroU32::new(limit).unwrap()))
            .collect(),
    )
}

async fn wait_until_queued(leases: &TestLeaseCoordinator) {
    tokio::time::timeout(Duration::from_secs(2), async {
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
    .expect("selector reaches the account queue");
}

async fn capacity_select(
    selector: &CodexCredentialSelector,
    attempt: &AttemptContext,
    affinity: Option<&ProviderSessionAffinityKey>,
) -> Result<provider_openai::credential::CodexCredentialLease, CredentialSelectionError> {
    selector
        .select(&SelectCodexCredential {
            upstream_model: "gpt-5.4",
            request_url: &Url::parse("https://chatgpt.com/backend-api/codex/responses").unwrap(),
            attempt,
            session_affinity_key: affinity,
        })
        .await
}

#[tokio::test]
async fn capacity_wait_sticky_holds_original_before_an_idle_alternative_and_does_not_advance_rr() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_original", "at-original");
    create_account(&store, "acct_other", "at-other");
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    leases.capacity.set_load("acct_original", 1);
    let affinity = Arc::new(MemorySessionAffinity::default());
    let key = ProviderSessionAffinityKey::try_new("wait-original").unwrap();
    affinity
        .bind(
            &ProviderKind::new("openai").unwrap(),
            &key,
            &ProviderAccountId::new("acct_original").unwrap(),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    let selector = selector_with_affinity(&store, Arc::clone(&leases), affinity)
        .with_account_concurrency(capacity_handle(&["acct_original", "acct_other"], 1));
    let attempt = attempt(BTreeSet::new()).with_request_tuning(capacity_tuning());
    let selected = capacity_select(&selector, &attempt, Some(&key));
    tokio::pin!(selected);
    tokio::select! {
        result = &mut selected => panic!("must wait for original: {result:?}"),
        () = wait_until_queued(&leases) => {}
    }
    assert!(leases.requests.lock().unwrap().is_empty());
    assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 1);
    {
        let waits = leases.capacity.waits.lock().unwrap();
        assert_eq!(
            waits[0].mode(),
            gateway_core::engine::AccountWaitMode::Sticky
        );
        assert_eq!(waits[0].max_waiting().get(), 3);
        assert!(waits[0].deadline() <= attempt.deadline());
    }
    leases.capacity.set_load("acct_original", 0);
    let lease = selected.await.unwrap();
    assert_eq!(lease.account_id().as_str(), "acct_original");
    assert!(lease.affinity_hit());
    assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
    assert_eq!(leases.capacity.state_reads.load(Ordering::SeqCst), 1);
    assert!(leases.capacity.signal_reads.load(Ordering::SeqCst) >= 3);
    assert_eq!(lease.capacity_snapshot().unwrap().used_slots(), 1);
    assert_eq!(lease.capacity_snapshot().unwrap().total_slots(), 2);
    drop(lease);
    assert_eq!(
        leases.capacity.signals.lock().unwrap()[&ProviderAccountId::new("acct_original").unwrap()]
            .in_flight,
        0
    );
}

#[tokio::test]
async fn capacity_wait_fallback_accepts_busy_snapshot_and_busy_without_retry_hint() {
    use std::sync::atomic::Ordering;
    for race in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_primary", "at-primary");
        create_account(&store, "acct_other", "at-other");
        persist_credential_state(
            &store,
            &store.account("acct_other").unwrap(),
            CredentialState::Invalid,
        );
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        leases.capacity.set_load("acct_primary", u32::from(!race));
        leases.capacity.race_busy.store(race, Ordering::SeqCst);
        let selector = selector(&store, Arc::clone(&leases))
            .with_account_concurrency(capacity_handle(&["acct_primary", "acct_other"], 1));
        let attempt = attempt(BTreeSet::new()).with_request_tuning(capacity_tuning());
        let selected = capacity_select(&selector, &attempt, None);
        tokio::pin!(selected);
        tokio::select! {
            result = &mut selected => panic!("busy must queue: {result:?}"),
            () = wait_until_queued(&leases) => {}
        }
        {
            let waits = leases.capacity.waits.lock().unwrap();
            assert_eq!(waits.len(), 1);
            assert_eq!(
                waits[0].mode(),
                gateway_core::engine::AccountWaitMode::Fallback
            );
            assert_eq!(waits[0].max_waiting().get(), 100);
        }
        leases.capacity.set_load("acct_primary", 0);
        leases.capacity.race_busy.store(false, Ordering::SeqCst);
        let lease = selected.await.unwrap();
        assert_eq!(lease.account_id().as_str(), "acct_primary");
        assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn capacity_wait_never_classifies_non_capacity_blockers_as_busy() {
    use std::sync::atomic::Ordering;
    for blocked in [
        "disabled",
        "invalid",
        "quota",
        "excluded",
        "interval",
        "cooldown",
        "missing-live",
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_primary", "at-primary");
        let id = ProviderAccountId::new("acct_primary").unwrap();
        let account = store.account("acct_primary").unwrap();
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        leases.capacity.set_load("acct_primary", 1);
        let mut exclusions = BTreeSet::new();
        let cooldowns = Arc::new(MemoryCooldownPort::new());
        match blocked {
            "disabled" => store.set_enabled(&id, false).await.unwrap(),
            "invalid" => persist_credential_state(&store, &account, CredentialState::Invalid),
            "quota" => persist_quota_exhaustion(&store, &account, None),
            "excluded" => {
                exclusions.insert(id.clone());
            }
            "interval" => {
                leases
                    .capacity
                    .signals
                    .lock()
                    .unwrap()
                    .get_mut(&id)
                    .unwrap()
                    .last_started_at = Some(SystemTime::now() + Duration::from_secs(60));
            }
            "cooldown" => {
                let recorder = selector_with_runtime(
                    &store,
                    Arc::clone(&leases),
                    Arc::new(MemorySessionAffinity::default()),
                    Arc::new(AccountFeedbackStats::default()),
                    cooldowns.clone(),
                );
                recorder
                    .record_failure(
                        &account,
                        CodexAccountFailure::RateLimited {
                            retry_after: Some(Duration::from_secs(60)),
                        },
                        None,
                    )
                    .await
                    .unwrap();
            }
            _ => {}
        }
        let selector = selector_with_runtime(
            &store,
            Arc::clone(&leases),
            Arc::new(MemorySessionAffinity::default()),
            Arc::new(AccountFeedbackStats::default()),
            cooldowns,
        )
        .with_account_concurrency(capacity_handle(
            if blocked == "missing-live" {
                &[]
            } else {
                &["acct_primary"]
            },
            1,
        ));
        let attempt = attempt(exclusions).with_request_tuning(capacity_tuning());
        assert!(
            matches!(
                capacity_select(&selector, &attempt, None).await,
                Err(CredentialSelectionError::NoEligibleCredential)
            ),
            "{blocked}"
        );
        assert!(
            leases.capacity.waits.lock().unwrap().is_empty(),
            "{blocked}"
        );
    }
}

#[tokio::test]
async fn capacity_wait_queue_full_soft_affinity_can_use_idle_but_required_cannot_move() {
    use std::sync::atomic::Ordering;
    for required in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_original", "at-original");
        create_account(&store, "acct_other", "at-other");
        let original = ProviderAccountId::new("acct_original").unwrap();
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        leases.capacity.set_load("acct_original", 1);
        leases.capacity.full.store(true, Ordering::SeqCst);
        let affinity = Arc::new(MemorySessionAffinity::default());
        let key = ProviderSessionAffinityKey::try_new("full-original").unwrap();
        affinity
            .bind(
                &ProviderKind::new("openai").unwrap(),
                &key,
                &original,
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        let selector = selector_with_affinity(&store, Arc::clone(&leases), affinity)
            .with_account_concurrency(capacity_handle(&["acct_original", "acct_other"], 1));
        let attempt = attempt_with_required(BTreeSet::new(), required.then_some(original))
            .with_request_tuning(capacity_tuning());
        let selected = capacity_select(&selector, &attempt, Some(&key)).await;
        if required {
            assert!(matches!(
                selected,
                Err(CredentialSelectionError::PinnedAccount {
                    owner_lost: false,
                    ..
                })
            ));
        } else {
            assert_eq!(selected.unwrap().account_id().as_str(), "acct_other");
        }
        assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
        assert_eq!(leases.capacity.waits.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn capacity_wait_disabled_and_diagnostic_preserve_the_legacy_path() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let leases = Arc::new(TestLeaseCoordinator::default());
    let selector = selector(&store, Arc::clone(&leases));
    let legacy = attempt(BTreeSet::new());
    capacity_select(&selector, &legacy, None).await.unwrap();
    let diagnostic = AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_diagnostic_wait").unwrap(),
            ClientApiKeyId::new("key_codex_contract").unwrap(),
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::diagnostic(
            BTreeSet::new(),
            ProviderAccountId::new("acct_primary").unwrap(),
            None,
        ),
        None,
        CancellationToken::new(),
    )
    .with_request_tuning(capacity_tuning());
    capacity_select(&selector, &diagnostic, None).await.unwrap();
    assert_eq!(leases.capacity.signal_reads.load(Ordering::SeqCst), 0);
    assert!(leases.capacity.waits.lock().unwrap().is_empty());
}

#[tokio::test]
async fn capacity_wait_cancellation_interrupts_signal_io_and_releases_the_waiter() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    leases.capacity.set_load("acct_primary", 1);
    let selector = selector(&store, Arc::clone(&leases))
        .with_account_concurrency(capacity_handle(&["acct_primary"], 1));
    let attempt = attempt(BTreeSet::new()).with_request_tuning(capacity_tuning());
    let selected = capacity_select(&selector, &attempt, None);
    tokio::pin!(selected);
    tokio::select! {
        result = &mut selected => panic!("must wait: {result:?}"),
        () = wait_until_queued(&leases) => {}
    }
    leases.capacity.pause_signals.store(true, Ordering::SeqCst);
    let before = leases.capacity.signal_reads.load(Ordering::SeqCst);
    tokio::select! {
        result = &mut selected => panic!("must remain pending: {result:?}"),
        () = async {
            while leases.capacity.signal_reads.load(Ordering::SeqCst) == before { tokio::task::yield_now().await; }
        } => {}
    }
    attempt.cancellation().cancel();
    let result = tokio::time::timeout(Duration::from_millis(200), selected)
        .await
        .unwrap();
    assert!(matches!(result, Err(CredentialSelectionError::Cancelled)));
    assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn capacity_wait_expiry_and_storage_failures_are_closed() {
    use std::sync::atomic::Ordering;
    for failure in ["signals", "enqueue", "expired", "no-publication"] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_primary", "at-primary");
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        leases.capacity.set_load("acct_primary", 1);
        leases
            .capacity
            .fail_signals
            .store(failure == "signals", Ordering::SeqCst);
        leases
            .capacity
            .fail_wait
            .store(failure == "enqueue", Ordering::SeqCst);
        leases
            .capacity
            .expire
            .store(failure == "expired", Ordering::SeqCst);
        let handle = if failure == "no-publication" {
            Default::default()
        } else {
            capacity_handle(&["acct_primary"], 1)
        };
        let selector = selector(&store, Arc::clone(&leases)).with_account_concurrency(handle);
        let attempt = attempt(BTreeSet::new()).with_request_tuning(capacity_tuning());
        let result = capacity_select(&selector, &attempt, None).await;
        assert!(result.is_err(), "{failure}");
        assert!(leases.requests.lock().unwrap().is_empty());
        assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
        if failure == "expired" {
            assert!(attempt.account_wait_budget().unwrap().is_exhausted());
        }
    }
}

#[tokio::test]
async fn capacity_wait_live_increase_ignores_old_request_limits_and_never_republishes_them() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.enabled.store(true, Ordering::SeqCst);
    leases.capacity.set_load("acct_primary", 1);
    let live = capacity_handle(&["acct_primary"], 1);
    let selector = selector(&store, Arc::clone(&leases)).with_account_concurrency(live.clone());
    let attempt = attempt(BTreeSet::new()).with_request_tuning(capacity_tuning());
    let selected = capacity_select(&selector, &attempt, None);
    tokio::pin!(selected);
    tokio::select! {
        result = &mut selected => panic!("must respect live limit one: {result:?}"),
        () = wait_until_queued(&leases) => {}
    }
    live.publish(
        gateway_core::routing::ConfigRevision::new(2).unwrap(),
        BTreeMap::from([("acct_primary".to_owned(), NonZeroU32::new(2).unwrap())]),
    );
    let lease = selected.await.unwrap();
    assert_eq!(lease.capacity_snapshot().unwrap().used_slots(), 2);
    assert_eq!(live.load().unwrap().unwrap().revision().get(), 2);
    assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn capacity_wait_rechecks_deletion_disable_and_credential_rotation_after_promotion() {
    use std::sync::atomic::Ordering;
    for change in ["delete", "disable", "rotate", "lower-limit"] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_primary", "at-primary");
        let id = ProviderAccountId::new("acct_primary").unwrap();
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.enabled.store(true, Ordering::SeqCst);
        leases.capacity.set_load("acct_primary", 1);
        let live = capacity_handle(&["acct_primary"], 1);
        let selector = selector(&store, Arc::clone(&leases)).with_account_concurrency(live.clone());
        let attempt = round_robin_attempt().with_request_tuning(capacity_tuning());
        let selected = capacity_select(&selector, &attempt, None);
        tokio::pin!(selected);
        tokio::select! {
            result = &mut selected => panic!("must wait: {result:?}"),
            () = wait_until_queued(&leases) => {}
        }
        let store_for_hook = Arc::clone(&store);
        *leases.capacity.after_acquire.lock().unwrap() = Some(Box::new(move || match change {
            "delete" => block_on(store_for_hook.delete_account(&id)).unwrap(),
            "disable" => block_on(store_for_hook.set_enabled(&id, false)).unwrap(),
            "rotate" => {
                let account = store_for_hook.account("acct_primary").unwrap();
                let repository = store_for_hook.repository();
                let mut data = block_on(repository.load_complete_data(&account)).unwrap();
                data.oauth_mut().unwrap().access_token = "at-rotated".to_owned();
                block_on(repository.compare_and_swap_data(&account, data)).unwrap();
            }
            "lower-limit" => {
                live.publish(
                    gateway_core::routing::ConfigRevision::new(2).unwrap(),
                    BTreeMap::new(),
                );
            }
            _ => unreachable!(),
        }));
        leases.capacity.set_load("acct_primary", 0);
        let result = selected.await;
        if change == "rotate" {
            let lease = result.unwrap();
            assert_eq!(
                lease
                    .authentication()
                    .oauth()
                    .unwrap()
                    .access_token
                    .expose_secret(),
                "at-rotated"
            );
            drop(lease);
        } else {
            assert!(result.is_err(), "{change}");
        }
        assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
        assert_eq!(
            leases.capacity.signals.lock().unwrap()
                [&ProviderAccountId::new("acct_primary").unwrap()]
                .in_flight,
            0,
            "{change}"
        );
    }
}

#[tokio::test]
async fn capacity_wait_final_validation_failure_does_not_claim_or_renew_affinity() {
    use std::sync::atomic::Ordering;
    for bound in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_primary", "at-primary");
        let leases = Arc::new(TestLeaseCoordinator::default());
        let affinity = Arc::new(MemorySessionAffinity::default());
        let key = ProviderSessionAffinityKey::try_new("final-check-affinity").unwrap();
        let id = ProviderAccountId::new("acct_primary").unwrap();
        if bound {
            affinity.seed_binding(
                &ProviderKind::new("openai").unwrap(),
                "final-check-affinity",
                id.clone(),
            );
        }
        let selector = selector_with_affinity(&store, Arc::clone(&leases), Arc::clone(&affinity))
            .with_account_concurrency(capacity_handle(&["acct_primary"], 1));
        let store_for_acquire = Arc::clone(&store);
        *leases.capacity.after_acquire.lock().unwrap() = Some(Box::new(move || {
            let store_for_list = Arc::clone(&store_for_acquire);
            *store_for_acquire.before_provider_list.lock().unwrap() = Some(Box::new(move || {
                block_on(store_for_list.set_enabled(&id, false)).unwrap();
            }));
        }));
        let attempt = attempt(BTreeSet::new()).with_request_tuning(capacity_tuning());
        assert!(
            capacity_select(&selector, &attempt, Some(&key))
                .await
                .is_err()
        );
        assert_eq!(affinity.binding_count(), usize::from(bound));
        assert!(
            affinity.renewal_ttls().is_empty(),
            "failed final validation cannot renew a binding"
        );
        assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
        assert_eq!(
            leases.capacity.signals.lock().unwrap()
                [&ProviderAccountId::new("acct_primary").unwrap()]
                .in_flight,
            0
        );
    }
}

#[tokio::test]
async fn capacity_wait_frozen_universe_does_not_add_new_imports_during_reselection() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.set_load("acct_primary", 1);
    let live = capacity_handle(&["acct_primary"], 1);
    let selector = selector(&store, Arc::clone(&leases)).with_account_concurrency(live.clone());
    let attempt = attempt(BTreeSet::new()).with_request_tuning(capacity_tuning());
    let selected = capacity_select(&selector, &attempt, None);
    tokio::pin!(selected);
    tokio::select! {
        result = &mut selected => panic!("must queue: {result:?}"),
        () = wait_until_queued(&leases) => {}
    }
    create_account(&store, "acct_other", "at-new-import");
    live.publish(
        gateway_core::routing::ConfigRevision::new(2).unwrap(),
        BTreeMap::from([
            ("acct_primary".to_owned(), NonZeroU32::MIN),
            ("acct_other".to_owned(), NonZeroU32::MIN),
        ]),
    );
    store
        .set_enabled(&ProviderAccountId::new("acct_primary").unwrap(), false)
        .await
        .unwrap();
    assert!(matches!(
        selected.await,
        Err(CredentialSelectionError::NoEligibleCredential)
    ));
    assert!(leases.requests.lock().unwrap().is_empty());
    assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn capacity_wait_cancel_safe_fast_acquire_does_not_consult_a_full_wait_queue() {
    use std::sync::atomic::Ordering;
    for cancel in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_primary", "at-primary");
        let leases = Arc::new(TestLeaseCoordinator::default());
        leases.capacity.full.store(true, Ordering::SeqCst);
        leases
            .capacity
            .pause_acquire_reply
            .store(cancel, Ordering::SeqCst);
        let selector = selector(&store, Arc::clone(&leases))
            .with_account_concurrency(capacity_handle(&["acct_primary"], 1));
        let attempt = attempt(BTreeSet::new()).with_request_tuning(capacity_tuning());
        if cancel {
            let selected = capacity_select(&selector, &attempt, None);
            tokio::pin!(selected);
            tokio::select! {
                result = &mut selected => panic!("wait for acquire reply: {result:?}"),
                () = async {
                    while leases.capacity.safe_acquires.load(Ordering::SeqCst) == 0 { tokio::task::yield_now().await; }
                } => {}
            }
            attempt.cancellation().cancel();
            assert!(matches!(
                selected.await,
                Err(CredentialSelectionError::Cancelled)
            ));
        } else {
            drop(capacity_select(&selector, &attempt, None).await.unwrap());
        }
        assert_eq!(leases.capacity.safe_acquires.load(Ordering::SeqCst), 1);
        assert!(leases.capacity.waits.lock().unwrap().is_empty());
        assert_eq!(
            leases.capacity.signals.lock().unwrap()
                [&ProviderAccountId::new("acct_primary").unwrap()]
                .in_flight,
            0
        );
    }
}

#[tokio::test]
async fn capacity_wait_total_deadline_interrupts_account_io_and_exhausts_shared_budget() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.set_load("acct_primary", 1);
    let selector = selector(&store, Arc::clone(&leases))
        .with_account_concurrency(capacity_handle(&["acct_primary"], 1));
    let attempt = AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_short_wait").unwrap(),
            ClientApiKeyId::new("key_codex_contract").unwrap(),
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_millis(300),
        account_policy(),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope()),
        None,
        CancellationToken::new(),
    )
    .with_request_tuning(capacity_tuning());
    let selected = capacity_select(&selector, &attempt, None);
    tokio::pin!(selected);
    tokio::select! {
        result = &mut selected => panic!("must queue: {result:?}"),
        () = wait_until_queued(&leases) => {}
    }
    let before = store.account_reads.load(Ordering::SeqCst);
    store.pause_account_read.store(true, Ordering::SeqCst);
    let result = tokio::time::timeout(Duration::from_secs(1), selected)
        .await
        .unwrap();
    assert!(matches!(
        result,
        Err(CredentialSelectionError::AccountWait(
            provider_openai::credential::AccountWaitFailure::Timeout
        ))
    ));
    assert!(store.account_reads.load(Ordering::SeqCst) > before);
    assert!(attempt.account_wait_budget().unwrap().is_exhausted());
    assert_eq!(leases.capacity.waiting.load(Ordering::SeqCst), 0);
    store.pause_account_read.store(false, Ordering::SeqCst);
    let next = attempt_with_required(
        BTreeSet::new(),
        Some(ProviderAccountId::new("acct_primary").unwrap()),
    )
    .with_request_tuning(capacity_tuning())
    .with_account_wait_budget(Arc::clone(attempt.account_wait_budget().unwrap()));
    let queued_before = leases.capacity.waits.lock().unwrap().len();
    assert!(matches!(
        capacity_select(&selector, &next, None).await,
        Err(CredentialSelectionError::PinnedAccount {
            owner_lost: false,
            ..
        })
    ));
    assert_eq!(leases.capacity.waits.lock().unwrap().len(), queued_before);
}

#[tokio::test]
async fn capacity_wait_owner_that_becomes_ready_before_enqueue_bypasses_the_full_queue() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_primary", "at-primary");
    let leases = Arc::new(TestLeaseCoordinator::default());
    leases.capacity.set_load("acct_primary", 1);
    leases.capacity.full.store(true, Ordering::SeqCst);
    let capacity = Arc::clone(&leases.capacity);
    *leases.capacity.after_state_load.lock().unwrap() = Some(Box::new(move || {
        capacity.set_load("acct_primary", 0);
    }));
    let selector = selector(&store, Arc::clone(&leases))
        .with_account_concurrency(capacity_handle(&["acct_primary"], 1));
    let attempt = attempt_with_required(
        BTreeSet::new(),
        Some(ProviderAccountId::new("acct_primary").unwrap()),
    )
    .with_request_tuning(capacity_tuning());
    let lease = capacity_select(&selector, &attempt, None).await.unwrap();
    assert_eq!(lease.account_id().as_str(), "acct_primary");
    assert_eq!(leases.capacity.safe_acquires.load(Ordering::SeqCst), 1);
    assert!(leases.capacity.waits.lock().unwrap().is_empty());
}
