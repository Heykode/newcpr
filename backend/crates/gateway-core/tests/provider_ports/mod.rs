use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::time::{Duration, SystemTime};

use gateway_core::account::{
    AccountRuntimeSignals, CredentialRevision, OpaqueProviderData, ProviderAccountId,
};
use gateway_core::provider_ports::{
    NewOAuthPendingFlow, OAuthPendingBinding, ProviderRefreshPolicy, ProviderSchedulingState,
    ProviderSessionAffinityKey, ProviderStoreErrorKind,
};
use gateway_core::routing::ProviderKind;

#[test]
fn account_wait_ports_default_to_unavailable_without_reading_rotation_state() {
    use futures::executor::block_on;
    use futures::future::BoxFuture;
    use gateway_core::engine::{AccountWaitMode, ModelRequestId};
    use gateway_core::policy::ClientApiKeyId;
    use gateway_core::provider_ports::{
        ProviderLeaseAcquisition, ProviderLeasePort, ProviderLeaseRequest,
        ProviderSchedulingLeaseRequest, ProviderStoreError, ProviderWaitLeaseRequest,
    };
    struct LegacyPort;
    impl ProviderLeasePort for LegacyPort {
        fn load_state<'a>(
            &'a self,
            _: &'a ClientApiKeyId,
            _: &'a ProviderKind,
            _: &'a [ProviderAccountId],
        ) -> BoxFuture<'a, Result<ProviderSchedulingState, ProviderStoreError>> {
            panic!("wait signal reads must not advance the old cursor");
        }
        fn try_acquire(
            &self,
            _: ProviderLeaseRequest,
        ) -> BoxFuture<'_, Result<ProviderLeaseAcquisition, ProviderStoreError>> {
            panic!("opt-in scheduling must not fall back to legacy lease acquisition");
        }
    }
    let provider = ProviderKind::new("openai").unwrap();
    let account = ProviderAccountId::new("acct_wait").unwrap();
    let port: &dyn ProviderLeasePort = &LegacyPort;
    let scheduling = ProviderSchedulingLeaseRequest::new(
        provider.clone(),
        account.clone(),
        CredentialRevision::new(1).unwrap(),
        NonZeroU32::new(1).unwrap(),
        Duration::ZERO,
        SystemTime::now() + Duration::from_secs(180),
    );
    assert_eq!(
        block_on(port.try_acquire_scheduling(scheduling))
            .unwrap_err()
            .kind(),
        ProviderStoreErrorKind::Unavailable
    );
    assert_eq!(
        block_on(port.load_signals(&provider, std::slice::from_ref(&account)))
            .unwrap_err()
            .kind(),
        ProviderStoreErrorKind::Unavailable
    );
    let request = ProviderWaitLeaseRequest::new(
        provider.clone(),
        account.clone(),
        ModelRequestId::new("req_wait").unwrap(),
        AccountWaitMode::Sticky,
        NonZeroU32::new(3).unwrap(),
        SystemTime::now() + Duration::from_secs(120),
    );
    assert_eq!(request.provider_kind(), &provider);
    assert_eq!(request.account_id(), &account);
    assert_eq!(request.request_id().as_str(), "req_wait");
    assert_eq!(request.mode(), AccountWaitMode::Sticky);
    assert_eq!(request.max_waiting().get(), 3);
    assert_eq!(
        block_on(port.try_acquire_wait(request)).unwrap_err().kind(),
        ProviderStoreErrorKind::Unavailable
    );
}

#[test]
fn ipv6_egress_modes_and_source_validation_keep_disabled_default() {
    use gateway_core::provider_ports::egress::{
        EgressMode, ProviderEgressConfig, is_valid_source_address,
    };
    assert_eq!(
        ProviderEgressConfig::default().default_mode,
        EgressMode::Unchanged
    );
    for (mode, active, random, fresh) in [
        (EgressMode::Unchanged, false, false, false),
        (EgressMode::FixedIpv6Reuse, true, false, false),
        (EgressMode::RandomIpv6Reuse, true, true, false),
        (EgressMode::FixedIpv6Fresh, true, false, true),
        (EgressMode::RandomIpv6Fresh, true, true, true),
    ] {
        assert_eq!(EgressMode::parse(mode.as_str()), Some(mode));
        assert_eq!(mode.is_active(), active);
        assert_eq!(mode.is_random(), random);
        assert_eq!(mode.is_fresh(), fresh);
    }
    assert!(EgressMode::parse("ipv4_fallback").is_none());
    for address in [
        "::",
        "::1",
        "::ffff:192.0.2.1",
        "fe80::1",
        "ff02::1",
        "fec0::1",
    ] {
        assert!(
            !is_valid_source_address(address.parse().unwrap()),
            "{address}"
        );
    }
    for address in ["2001:db8::1", "fd00::1"] {
        assert!(
            is_valid_source_address(address.parse().unwrap()),
            "{address}"
        );
    }
}

#[test]
fn oauth_pending_binding_debug_redacts_raw_value() {
    let binding = OAuthPendingBinding::try_new("must-not-appear").expect("valid binding");

    assert_eq!(format!("{binding:?}"), "OAuthPendingBinding([REDACTED])");
}

#[test]
fn provider_session_affinity_key_debug_is_opaque() {
    let key = ProviderSessionAffinityKey::try_new("opaque-session-key").expect("valid key");

    assert_eq!(format!("{key:?}"), "ProviderSessionAffinityKey([OPAQUE])");
}

#[test]
fn oauth_pending_ttl_rejects_zero_and_more_than_thirty_minutes() {
    let provider = ProviderKind::new("fixture").expect("valid provider");
    let flow = OAuthPendingBinding::try_new("flow").expect("valid flow");
    let owner = OAuthPendingBinding::try_new("owner").expect("valid owner");
    let payload = OpaqueProviderData::new(serde_json::Map::new());

    for ttl in [Duration::ZERO, Duration::from_secs(30 * 60 + 1)] {
        let error = NewOAuthPendingFlow::try_new(
            provider.clone(),
            flow.clone(),
            owner.clone(),
            ttl,
            payload.clone(),
        )
        .expect_err("invalid TTL must fail");
        assert_eq!(error.kind(), ProviderStoreErrorKind::InvalidData);
    }
}

#[test]
fn refresh_policy_requires_a_positive_margin() {
    let error = ProviderRefreshPolicy::try_new(
        Duration::ZERO,
        NonZeroU32::new(1).expect("positive concurrency"),
    )
    .expect_err("zero margin must fail");

    assert_eq!(error.kind(), ProviderStoreErrorKind::InvalidData);
}

#[test]
fn refresh_policy_should_mark_tokens_due_at_the_exact_configured_margin() {
    let policy = ProviderRefreshPolicy::try_new(
        Duration::from_secs(3_600),
        NonZeroU32::new(2).expect("positive concurrency"),
    )
    .expect("valid policy");
    let observed_at = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
    let expires_at = observed_at + Duration::from_secs(7_200);

    assert!(!policy.is_refresh_due(expires_at, observed_at));
    assert!(policy.is_refresh_due(observed_at + Duration::from_secs(3_600), observed_at));
}

#[test]
fn refresh_policy_should_mark_expired_tokens_due() {
    let policy = ProviderRefreshPolicy::try_new(
        Duration::from_secs(3_600),
        NonZeroU32::new(1).expect("positive concurrency"),
    )
    .expect("valid policy");
    let observed_at = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);

    assert!(policy.is_refresh_due(observed_at - Duration::from_secs(1), observed_at));
}

#[test]
fn scheduling_state_preserves_provider_neutral_signals() {
    let account = ProviderAccountId::new("acct_fixture").expect("valid account");
    let signals = BTreeMap::from([(
        account.clone(),
        AccountRuntimeSignals {
            in_flight: 2,
            last_started_at: None,
            quota_reset_at: None,
            quota_remaining_rank: Some(7),
            rate_limited_until: None,
            failure_rate_basis_points: Some(125),
            first_output_latency_ms: Some(250),
        },
    )]);
    let state = ProviderSchedulingState::new(signals, 9);

    assert_eq!(state.signals()[&account].in_flight, 2);
    assert_eq!(
        state.signals()[&account].failure_rate_basis_points,
        Some(125)
    );
    assert_eq!(state.round_robin_cursor(), 9);
    assert_eq!(
        CredentialRevision::new(1).expect("positive revision").get(),
        1
    );
}
