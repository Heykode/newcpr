use super::*;
use gateway_core::account::{ExcelCredentialMetadata, ResponsesUpstream};

fn excel() -> ExcelCredentialMetadata {
    ExcelCredentialMetadata {
        revision: CredentialRevision::new(7).unwrap(),
        state: CredentialState::Ready,
        upstream_user_id: "user-a".into(),
        upstream_account_id: Some("workspace-a".into()),
        email: None,
        plan_type: Some("plus".into()),
        access_token_expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
        has_refresh_token: true,
        next_refresh_at: None,
        refresh_attempted: false,
        observed_at: SystemTime::now(),
        last_error_reason: None,
        last_error_message: None,
        suspended: false,
        quota: QuotaState::unknown(),
    }
}

#[test]
fn excel_projection_preserves_shared_policy_and_codex_facts() {
    let original = account("acct_a")
        .with_account_facts(
            true,
            CredentialState::Expired,
            QuotaState::exhausted(QuotaEvidence::PaymentRequired, SystemTime::now(), None),
            None,
            None,
        )
        .with_excel_credential(Some(excel()))
        .with_responses_upstream(ResponsesUpstream::Excel);
    let projected = original
        .clone()
        .for_credential_route(ResponsesUpstream::Excel)
        .unwrap();
    assert_eq!(projected.credential_state(), CredentialState::Ready);
    assert!(!projected.quota().is_exhausted());
    assert_eq!(projected.revision(), original.revision());
    assert_eq!(projected.credential_revision().get(), 7);
    assert_eq!(projected.id(), original.id());
    assert_eq!(projected.weight(), original.weight());
    assert_eq!(projected.model_access(), original.model_access());
    assert_eq!(
        projected.status_projection(SystemTime::now(), None).status,
        gateway_core::account::AccountStatus::Normal
    );
    assert_eq!(
        projected
            .for_credential_route(ResponsesUpstream::Codex)
            .unwrap(),
        original
    );
}

#[test]
fn runtime_pauses_are_isolated_to_the_selected_credential() {
    let codex_paused = account("acct_a")
        .with_excel_credential(Some(excel()))
        .with_quota_limits(gateway_core::account::AccountQuotaLimits::disabled(), true);
    let projected = codex_paused
        .clone()
        .for_credential_route(ResponsesUpstream::Excel)
        .unwrap();
    assert!(!projected.quota_paused());
    assert_eq!(
        projected.status_projection(SystemTime::now(), None).status,
        AccountStatus::Normal
    );
    assert_eq!(
        codex_paused
            .status_projection(SystemTime::now(), None)
            .status,
        AccountStatus::QuotaPaused
    );

    let mut suspended = excel();
    suspended.suspended = true;
    let codex = account("acct_a").with_excel_credential(Some(suspended));
    assert!(!codex.quota_paused());
    assert_eq!(
        codex.status_projection(SystemTime::now(), None).status,
        AccountStatus::Normal
    );
    let projected = codex
        .for_credential_route(ResponsesUpstream::Excel)
        .unwrap();
    assert!(projected.quota_paused());
    assert_eq!(
        projected.status_projection(SystemTime::now(), None).status,
        AccountStatus::QuotaPaused
    );
}

#[test]
fn absent_credentials_cannot_be_projected_or_silently_substituted() {
    assert!(
        account("acct_a")
            .for_credential_route(ResponsesUpstream::Excel)
            .is_none()
    );
    let excel_only = ProviderAccount::new(
        ProviderAccountId::new("acct_a").unwrap(),
        ProviderKind::new("openai").unwrap(),
        "a".into(),
        Some("user-a".into()),
        "none".into(),
        CredentialRevision::new(1).unwrap(),
        None,
    )
    .with_excel_credential(Some(excel()));
    assert!(!excel_only.has_codex_credential());
    assert!(
        excel_only
            .clone()
            .for_credential_route(ResponsesUpstream::Codex)
            .is_none()
    );
    assert_eq!(
        excel_only
            .for_credential_route(ResponsesUpstream::Excel)
            .unwrap()
            .credential_state(),
        CredentialState::Ready
    );
}

#[test]
fn active_excel_route_does_not_depend_on_the_legacy_model_list() {
    let account = account("acct_a")
        .with_responses_upstream(ResponsesUpstream::Excel)
        .with_excel_models(gateway_core::account::ExcelModels::try_from(Vec::new()).unwrap());
    assert_eq!(
        account.responses_upstream_for_model("some-supported-model"),
        ResponsesUpstream::Excel
    );
}
