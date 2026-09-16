use super::*;

#[tokio::test]
async fn active_plan_updates_support_upgrades_downgrades_and_preserve_unknown_and_sku() {
    for (current_plan, observed_plan, expected) in [
        ("plus", Some("pro"), "pro"),
        ("pro", Some("free"), "free"),
        ("plus", None, "plus"),
        ("plus", Some("unknown"), "plus"),
        (
            "self_serve_business_prolite",
            Some("team"),
            "self_serve_business_prolite",
        ),
        ("edu_plus", Some("education"), "edu_plus"),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        let mut verified_account = profile("synthetic-plan-account");
        verified_account.plan_type = Some(current_plan.to_owned());
        store
            .seed_oauth_credential(ImportCodexOAuthCredential {
                account_id: "acct_plan_account".to_owned(),
                name: "plan-account".to_owned(),
                secret: secret("synthetic-plan-token"),
                verified_account,
                next_refresh_at: None,
                enabled: true,
            })
            .await;
        let before = store.account("acct_plan_account").unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/api/codex/usage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "plan_type": observed_plan, "rate_limit": {"allowed": true, "primary_window": {"used_percent": 12}}
            }))).expect(1).mount(&server).await;
        let service = quota_service_with_base_url(
            &store,
            reqwest::Client::builder().no_proxy().build().unwrap(),
            server.uri(),
        );
        service.refresh_account(before.id()).await.unwrap();
        let after = store.account("acct_plan_account").unwrap();
        assert_eq!(after.plan_type(), Some(expected));
        assert_eq!(after.revision(), before.revision());
        assert_eq!(after.upstream_account_id(), before.upstream_account_id());
        store
            .repository()
            .rotate_refreshed_oauth_secret(
                &before,
                secret("synthetic-new-token"),
                before.access_token_expires_at(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            store.account("acct_plan_account").unwrap().plan_type(),
            Some(expected)
        );
    }
}

#[tokio::test]
async fn passive_headers_without_plan_do_not_replay_an_old_account_plan() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_passive_plan").await;
    let original = store.account("acct_passive_plan").unwrap();
    let service = quota_service(&store);
    for (plan, expected) in [(Some("pro"), "pro"), (None, "pro"), (Some("free"), "free")] {
        let mut headers = vec![("x-codex-primary-used-percent".to_owned(), "5".to_owned())];
        if let Some(plan) = plan {
            headers.push(("x-codex-plan-type".to_owned(), plan.to_owned()));
        }
        service
            .synchronize_passive_headers(&original, &headers)
            .await
            .unwrap();
        assert_eq!(
            store.account("acct_passive_plan").unwrap().plan_type(),
            Some(expected)
        );
    }
}
