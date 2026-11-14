use super::*;
use gateway_core::account::{
    CredentialRevision, CredentialState, ExcelCredentialMetadata, OutboundProxy, ProviderAccount,
    ProviderAccountId, QuotaState, ResponsesUpstream,
};
use gateway_core::routing::ProviderKind;
use std::time::SystemTime;

fn excel_account(proxy: &str) -> ProviderAccount {
    ProviderAccount::new(
        ProviderAccountId::new("acct_fixture").unwrap(),
        ProviderKind::new("openai").unwrap(),
        "Fixture".into(),
        Some("user_fixture".into()),
        "oauth".into(),
        CredentialRevision::new(1).unwrap(),
        None,
    )
    .with_outbound_proxy(Some(OutboundProxy::parse(proxy).unwrap()))
    .with_excel_credential(Some(ExcelCredentialMetadata {
        revision: CredentialRevision::new(2).unwrap(),
        state: CredentialState::Ready,
        upstream_user_id: "user_fixture".into(),
        upstream_account_id: Some("workspace".into()),
        email: None,
        plan_type: None,
        access_token_expires_at: None,
        has_refresh_token: false,
        next_refresh_at: None,
        observed_at: SystemTime::now(),
        last_error_reason: None,
        last_error_message: None,
        suspended: false,
        refresh_attempted: false,
        quota: QuotaState::default(),
    }))
    .for_credential_route(ResponsesUpstream::Excel)
    .unwrap()
}

#[tokio::test]
async fn excel_real_send_checks_account_catalog_through_the_same_proxy() {
    for allowed in [false, true] {
        let proxy = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("{RESPONSES_PATH}/access")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "allowed": true,
                "model_catalog": {"models": [{"id": if allowed { VERIFIED_MODEL } else { "other-model" }}]}
            })))
            .expect(1)
            .mount(&proxy)
            .await;
        Mock::given(method("POST"))
            .and(path(RESPONSES_PATH))
            .respond_with(completed())
            .expect(if allowed { 1 } else { 0 })
            .mount(&proxy)
            .await;
        let account = excel_account(&proxy.uri());
        let endpoint = format!("http://fixture.invalid{RESPONSES_PATH}");
        let request = request(endpoint, json!("Keep requested model"));
        let result = client("http://fixture.invalid")
            .for_account(&account)
            .unwrap()
            .create_response_stream_with_pool_account(
                &request,
                CodexRequestContext::auxiliary(
                    "Bearer fixture-token",
                    Some("workspace"),
                    "request-fixture",
                    None,
                ),
                Some("acct_fixture"),
            )
            .await;
        if allowed {
            result.unwrap().body.try_collect::<Vec<_>>().await.unwrap();
        } else {
            assert!(matches!(
                result,
                Err(crate::transport::CodexClientError::ExcelModelUnsupported)
            ));
        }
        let requests = proxy.received_requests().await.unwrap();
        assert_eq!(requests.len(), if allowed { 2 } else { 1 });
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].url.query(), Some("include_models=true"));
        if allowed {
            assert_eq!(requests[1].method, "POST");
            let body = requests[1].body_json::<Value>().unwrap();
            assert_eq!(body["model"], VERIFIED_MODEL);
            assert_eq!(body["model_selection"], "explicit");
            for name in [
                "authorization",
                "chatgpt-account-id",
                "x-openai-account-id",
                "user-agent",
            ] {
                assert_eq!(requests[0].headers.get(name), requests[1].headers.get(name));
            }
        }
    }
}
