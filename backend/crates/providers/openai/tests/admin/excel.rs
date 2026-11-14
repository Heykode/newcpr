use super::*;
use provider_openai::credential::excel::{EXCEL_CALLBACK_URI, EXCEL_CLIENT_ID};

fn context(owner: &str) -> MutationContext {
    MutationContext {
        actor: MutationActor::AdminSession {
            admin_user_id: owner.into(),
        },
        request_id: "excel-authorization-fixture".into(),
    }
}

#[tokio::test]
async fn excel_oauth_uses_its_own_pending_namespace_and_releases_invalid_callbacks() {
    let pending = Arc::new(TestOAuthPending::default());
    let config = valid_config();
    let bundle = provider_openai::initialize(
        config.config.clone(),
        provider_ports_with(Arc::new(MemoryAccountStore::default()), pending.clone()),
    )
    .await
    .unwrap();
    let started = bundle
        .admin_provider()
        .start_excel_authorization(PendingAuthorizationMutation::new(
            ProviderKind::new("openai").unwrap(),
            AuthorizationMutationTarget::Create {
                name: "Excel fixture".into(),
            },
            AuthorizationOwnerBinding::from_context(&context("owner")),
        ))
        .await
        .unwrap();
    let url = url::Url::parse(&started.authorization_url).unwrap();
    assert!(
        url.query_pairs()
            .any(|(key, value)| key == "client_id" && value == EXCEL_CLIENT_ID)
    );
    let key = ("openai-excel".to_owned(), started.flow_id.clone());
    assert!(pending.values.lock().unwrap().contains_key(&key));
    for (owner, callback) in [
        (
            "other-owner",
            format!("{EXCEL_CALLBACK_URI}?code=unused&state=unused"),
        ),
        (
            "owner",
            "http://localhost:1455/auth/callback?code=unused&state=unused".into(),
        ),
        (
            "owner",
            format!("{EXCEL_CALLBACK_URI}?code=unused&state=wrong"),
        ),
    ] {
        let error = bundle
            .admin_provider()
            .complete_excel_authorization(CompleteAuthorization {
                context: context(owner),
                flow_id: started.flow_id.clone(),
                callback_url: callback,
                settings: None,
            })
            .await
            .expect_err("callback rejected before token exchange");
        assert_eq!(error.kind(), ProviderAdminErrorKind::Invalid);
        let values = pending.values.lock().unwrap();
        assert_eq!(values.len(), 1);
        assert!(
            values[&key].3.is_none(),
            "validation cannot leave a pending claim held"
        );
    }
}

#[tokio::test]
async fn excel_reauthorization_captures_its_own_revision_and_rejects_stale_flows_before_exchange() {
    let accounts = Arc::new(MemoryAccountStore::default());
    accounts
        .seed_oauth_credential(ImportCodexOAuthCredential {
            account_id: "acct_excel_pending".into(),
            name: "Excel fixture".into(),
            secret: secret("codex-original"),
            verified_account: profile("workspace-excel-pending"),
            next_refresh_at: None,
            enabled: true,
        })
        .await;
    accounts.seed_excel_credential("acct_excel_pending", true);
    let account = accounts.account("acct_excel_pending").unwrap();
    let original = accounts
        .load_current_credential(account.id())
        .await
        .unwrap()
        .credential;
    let pending = Arc::new(TestOAuthPending::default());
    let config = valid_config();
    let bundle = provider_openai::initialize(
        config.config.clone(),
        provider_ports_with(accounts.clone(), pending.clone()),
    )
    .await
    .unwrap();
    let started = bundle
        .admin_provider()
        .start_excel_authorization(PendingAuthorizationMutation::new(
            ProviderKind::new("openai").unwrap(),
            AuthorizationMutationTarget::Reauthorize {
                account_id: account.id().clone(),
            },
            AuthorizationOwnerBinding::from_context(&context("owner")),
        ))
        .await
        .unwrap();
    let key = ("openai-excel".to_owned(), started.flow_id.clone());
    let state = {
        let values = pending.values.lock().unwrap();
        let document = values[&key].1.expose_to_provider();
        assert_eq!(document["expected_revision"], 1);
        assert_eq!(
            document["identity"]["account_id"],
            "workspace-excel-pending"
        );
        document["state"].as_str().unwrap().to_owned()
    };
    accounts.seed_excel_credential("acct_excel_pending", true);
    let error = bundle
        .admin_provider()
        .complete_excel_authorization(CompleteAuthorization {
            context: context("owner"),
            flow_id: started.flow_id,
            settings: None,
            callback_url: format!("{EXCEL_CALLBACK_URI}?code=must-not-be-exchanged&state={state}"),
        })
        .await
        .expect_err("stale Excel revision rejected before network exchange");
    assert_eq!(error.kind(), ProviderAdminErrorKind::Conflict);
    assert!(pending.values.lock().unwrap()[&key].3.is_none());
    assert_eq!(
        accounts
            .load_current_credential(account.id())
            .await
            .unwrap()
            .credential,
        original
    );
}
