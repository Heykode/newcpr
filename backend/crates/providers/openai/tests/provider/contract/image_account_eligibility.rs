use super::*;
use gateway_core::account::{AccountModelAccess, AccountModelAccessMode};

fn image_operation(body: &'static [u8]) -> Operation {
    Operation::GenerateImage(ImageRequest::from_raw_json(
        ImageRequestKind::Generation,
        RawJsonPayload::new("openai", Bytes::from_static(body)).expect("image payload"),
    ))
}

fn image_scope(id: &str, access: AccountModelAccess) -> Arc<FrozenAccountScope> {
    Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([(
            ProviderAccountId::new(id).expect("account ID"),
            RuntimeAccount::new(ProviderKind::new("openai").unwrap(), BTreeSet::new())
                .with_model_access(access),
        )]))),
        ClientRoutingScope::all_accounts(),
    ))
}

#[tokio::test]
async fn free_oauth_account_cannot_send_native_image_request() {
    let store = Arc::new(MemoryAccountStore::default());
    let mut free = profile("acct_free_image");
    free.plan_type = Some(" FrEe ".to_owned());
    store
        .seed_oauth_credential(ImportCodexOAuthCredential {
            account_id: "acct_free_image".to_owned(),
            name: "acct_free_image".to_owned(),
            secret: secret("free-image-token"),
            verified_account: free,
            next_refresh_at: Some(Utc::now() + chrono::Duration::minutes(30)),
            enabled: true,
        })
        .await;
    let server = MockServer::start().await;
    let result = provider_with_base_url(&store, server.uri())
        .execute(
            planned_provider_endpoint_request(
                "openai",
                image_operation(br#"{"model":"gpt-image-2","prompt":"test"}"#),
            ),
            context_with_account_scope(
                "req_free_image",
                CancellationToken::new(),
                image_scope("acct_free_image", AccountModelAccess::all()),
            ),
        )
        .await;
    assert_eq!(
        result.err().expect("free is ineligible").kind(),
        ProviderErrorKind::NoEligibleAccount
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn image_model_access_requires_exact_model_or_unrestricted_policy() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_restricted_image").await;
    let server = MockServer::start().await;
    let access = AccountModelAccess::new(
        AccountModelAccessMode::Allowlist,
        vec!["gpt-image-2".to_owned()],
    )
    .unwrap();
    let provider = provider_with_base_url(&store, server.uri());
    for body in [
        &br#"{"model":"gpt-image-3","prompt":"test"}"#[..],
        &br#"{"prompt":"test"}"#[..],
        &br#"{"model":"gpt-image-2","model":"gpt-image-3","prompt":"test"}"#[..],
    ] {
        let result = provider
            .execute(
                planned_provider_endpoint_request("openai", image_operation(body)),
                context_with_account_scope(
                    "req_restricted_image",
                    CancellationToken::new(),
                    image_scope("acct_restricted_image", access.clone()),
                ),
            )
            .await;
        assert_eq!(
            result.err().expect("image model denied").kind(),
            ProviderErrorKind::NoEligibleAccount
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn eligible_image_requests_send_with_matching_allowlist_or_unrestricted_scope() {
    for (body, access) in [
        (
            &br#"{"model":"gpt-image-2","prompt":"test"}"#[..],
            AccountModelAccess::new(
                AccountModelAccessMode::Allowlist,
                vec!["gpt-image-2".to_owned()],
            )
            .unwrap(),
        ),
        (&br#"{"prompt":"test"}"#[..], AccountModelAccess::all()),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_allowed_image").await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/codex/images/generations"))
            .and(body_bytes(body.to_vec()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"created":100,"data":[{"b64_json":"AAEC"}]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut stream = provider_with_base_url(&store, server.uri())
            .execute(
                planned_provider_endpoint_request("openai", image_operation(body)),
                context_with_account_scope(
                    "req_allowed_image",
                    CancellationToken::new(),
                    image_scope("acct_allowed_image", access),
                ),
            )
            .await
            .expect("eligible image request");
        let mut completed = false;
        while let Some(event) = stream.next().await {
            if event
                .unwrap()
                .canonical_facts()
                .iter()
                .any(|fact| matches!(fact, GatewayEvent::Completed(_)))
            {
                completed = true;
            }
        }
        assert!(completed);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn free_excel_image_account_keeps_its_separate_route() {
    let store = Arc::new(MemoryAccountStore::default());
    let mut free = profile("acct_free_excel_image");
    free.plan_type = Some("free".to_owned());
    store
        .seed_oauth_credential(ImportCodexOAuthCredential {
            account_id: "acct_free_excel_image".to_owned(),
            name: "acct_free_excel_image".to_owned(),
            secret: secret("free-excel-image-token"),
            verified_account: free,
            next_refresh_at: Some(Utc::now() + chrono::Duration::minutes(30)),
            enabled: true,
        })
        .await;
    store.set_responses_upstream(
        "acct_free_excel_image",
        gateway_core::account::ResponsesUpstream::Excel,
    );
    let server = MockServer::start().await;
    let provider = provider_with_base_url(&store, server.uri());
    let stream = provider
        .execute(
            planned_provider_endpoint_request(
                "openai",
                image_operation(br#"{"model":"gpt-image-2","prompt":"test"}"#),
            ),
            context_with_account_scope(
                "req_free_excel_image",
                CancellationToken::new(),
                image_scope("acct_free_excel_image", AccountModelAccess::all()),
            ),
        )
        .await
        .expect("Excel image remains eligible");
    assert_eq!(stream.metadata().transport().as_str(), "http_json");
    // Preparation is lazy; do not poll the real Excel endpoint.
    drop(stream);
    // Native images are opaque here; this rejection proves Excel's parser owns
    // the route even when the selected account has a Free plan.
    let rejected = provider
        .execute(
            planned_provider_endpoint_request(
                "openai",
                image_operation(br#"{"model":"gpt-image-2","prompt":"test","size":"unsupported"}"#),
            ),
            context_with_account_scope(
                "req_free_excel_image_invalid",
                CancellationToken::new(),
                image_scope("acct_free_excel_image", AccountModelAccess::all()),
            ),
        )
        .await
        .err()
        .expect("Excel image validation applies before sending");
    assert_eq!(rejected.kind(), ProviderErrorKind::InvalidRequest);
    assert_eq!(rejected.send_state(), UpstreamSendState::NotSent);
    assert!(server.received_requests().await.unwrap().is_empty());
}
