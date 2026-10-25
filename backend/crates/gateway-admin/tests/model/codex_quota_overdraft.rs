use chrono::Utc;
use gateway_admin::model::codex_quota_overdraft::CodexQuotaOverdraftOutcome;
use gateway_core::{
    engine::probe::AccountProbeError,
    error::{ProviderError, ProviderErrorKind},
    upstream::UpstreamSendState,
};

#[test]
fn typed_probe_failures_map_to_stable_overdraft_outcomes() {
    for (kind, expected) in [
        (ProviderErrorKind::QuotaExhausted, "quota_limited"),
        (ProviderErrorKind::Unauthorized, "authentication_failed"),
        (ProviderErrorKind::Unavailable, "upstream_unavailable"),
    ] {
        let outcome = CodexQuotaOverdraftOutcome::from_probe_error(
            AccountProbeError::from_provider(&ProviderError::new(kind, UpstreamSendState::Sent)),
            Utc::now(),
        );
        match outcome {
            CodexQuotaOverdraftOutcome::Failed { reason_code, .. } => {
                assert_eq!(reason_code, expected);
                assert_ne!(expected, "upstream_unavailable");
            }
            CodexQuotaOverdraftOutcome::Inconclusive { reason_code, .. } => {
                assert_eq!(reason_code, expected);
                assert_eq!(expected, "upstream_unavailable");
            }
            CodexQuotaOverdraftOutcome::Passed { .. } => panic!("failure probe passed"),
        }
    }
}
