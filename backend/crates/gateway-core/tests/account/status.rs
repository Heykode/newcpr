use super::*;
use gateway_core::account::{
    AccountErrorReason, AccountStatusFacts, resolve_account_operational_status,
    resolve_account_status,
};

#[test]
fn operational_status_preserves_disabled_account_facts_without_enabling_scheduling() {
    let now = SystemTime::now();
    for (credential_state, quota, cooldown, expected) in [
        (
            CredentialState::Ready,
            QuotaState::unknown(),
            None,
            AccountStatus::Normal,
        ),
        (
            CredentialState::Invalid,
            QuotaState::unknown(),
            None,
            AccountStatus::Error,
        ),
        (
            CredentialState::Ready,
            QuotaState::exhausted(QuotaEvidence::ProviderDenied, now, None),
            None,
            AccountStatus::QuotaExhausted,
        ),
        (
            CredentialState::Ready,
            QuotaState::unknown(),
            Some(now + Duration::from_secs(300)),
            AccountStatus::RateLimited,
        ),
    ] {
        for enabled in [true, false] {
            let facts = AccountStatusFacts {
                enabled,
                credential_state,
                access_token_expires_at: Some(now + Duration::from_secs(3600)),
                quota,
                rate_limited_until: cooldown,
                last_error_reason: Some(AccountErrorReason::CredentialInvalid),
                last_error_message: Some("invalid fixture credential".to_owned()),
            };
            let operational = resolve_account_operational_status(&facts, now);
            assert_eq!(operational.status, expected);
            if expected == AccountStatus::Error {
                assert_eq!(operational.error_message, facts.last_error_message);
            }
            assert_eq!(
                resolve_account_status(&facts, now).status,
                if enabled {
                    expected
                } else {
                    AccountStatus::Disabled
                },
            );
        }
    }
}
