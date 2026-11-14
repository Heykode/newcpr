//! 验证真实响应头只写回一次，流内失败不重复处理 Cookie

use super::*;
use gateway_core::engine::probe::AccountProbeTarget;

fn cookie_provider(store: &Arc<MemoryAccountStore>, base_url: String) -> Arc<CodexProvider> {
    let profile = wire_profile();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let leases = Arc::new(TestLeaseCoordinator::default());
    let cooldowns = Arc::new(MemoryCooldownPort::new());
    let catalog = Arc::new(CodexCredentialCatalogService::new(
        store.repository(),
        profile.clone(),
        http.clone(),
        base_url.clone(),
        catalog_cache(),
        crate::support::runtime_policy(),
    ));
    let quota = Arc::new(CodexCredentialQuotaService::new(
        store.repository(),
        profile.clone(),
        http.clone(),
        base_url.clone(),
        cooldowns.clone(),
    ));
    let feedback = Arc::new(AccountFeedbackStats::default());
    let selector = Arc::new(CodexCredentialSelector::new(
        ProviderKind::new("openai").unwrap(),
        store.repository(),
        leases,
        Arc::new(MemorySessionAffinity::default()),
        Arc::new(MemorySessionExclusions::default()),
        Arc::clone(&quota),
        Arc::clone(&catalog),
        cooldowns,
        Arc::clone(&feedback),
        // 测试仅允许本机模拟上游，生产 Cookie 白名单不放宽
        CodexCookiePolicy::new(["cf_clearance"], ["127.0.0.1"]).unwrap(),
    ));
    Arc::new(
        CodexProvider::new(
            selector,
            catalog,
            quota,
            feedback,
            http,
            profile,
            base_url,
            Arc::new(CodexWebSocketPool::default()),
            0,
            gateway_core::time::DeploymentTimeZone::default(),
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn freeze_recovery_receives_upstream_outcome_without_rotating_response_cookies() {
    let completed_sse = format!(
        "event: response.created\ndata: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_scope_capture\",\"model\":\"gpt-5.4\"}}}}\n\n{CAPTURE_COMPLETED_SSE}"
    );
    for (status, content_type, body) in [
        (
            400,
            "application/json",
            r#"{"error":{"message":"invalid model","type":"invalid_request_error"}}"#,
        ),
        (200, "text/event-stream", completed_sse.as_str()),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        let account_id = "acct_provider_contract";
        create_account(&store, account_id).await;
        let original = store.account(account_id).unwrap();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("content-type", content_type)
                    .insert_header("set-cookie", "cf_clearance=changed; Path=/; Max-Age=3600")
                    .set_body_string(body),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut stream = cookie_provider(&store, server.uri())
            .execute(
                planned_request("openai", http_generate_operation()),
                probe_context(
                    "req_freeze_recovery",
                    AccountProbeTarget::FreezeRecovery(original.id().clone()),
                ),
            )
            .await
            .unwrap();
        let mut completed = false;
        let mut rejected = None;
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => {
                    completed |= event
                        .canonical_facts()
                        .iter()
                        .any(|fact| matches!(fact, GatewayEvent::Completed(_)))
                }
                Err(error) => {
                    assert_eq!(status, 400, "expected a completed probe: {error:?}");
                    rejected = error.upstream_status();
                    break;
                }
            }
        }
        assert_eq!(completed, status == 200);
        assert_eq!(rejected, (status == 400).then_some(400));
        assert_eq!(
            store.account(account_id).unwrap().revision(),
            original.revision()
        );
    }
}
