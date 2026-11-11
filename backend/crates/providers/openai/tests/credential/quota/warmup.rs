use super::*;
use gateway_core::account::{
    OpaqueProviderData, OutboundProxy, QuotaObservation, QuotaWriteOutcome, RequestProxySource,
    ResponsesUpstream,
};
use gateway_core::provider_ports::session_proxy::{
    SessionProxyError, SessionProxyLease, SessionProxyOutcome, SessionProxyPool,
};
use provider_openai::credential::CodexCredentialAdmin;
use serde_json::Value;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct ExitState {
    acquired: AtomicUsize,
    active: AtomicUsize,
    outcomes: Mutex<Vec<SessionProxyOutcome>>,
    scopes: Mutex<Vec<String>>,
}

struct ExitLease {
    proxy: OutboundProxy,
    state: Arc<ExitState>,
}

impl Drop for ExitLease {
    fn drop(&mut self) {
        self.state.active.fetch_sub(1, Ordering::SeqCst);
    }
}

impl SessionProxyLease for ExitLease {
    fn proxy(&self) -> &OutboundProxy {
        &self.proxy
    }
    fn node_id(&self) -> &str {
        "warmup-fixture"
    }
    fn report(&self, outcome: SessionProxyOutcome) {
        self.state.outcomes.lock().unwrap().push(outcome);
    }
}

struct ExitPool {
    source: RequestProxySource,
    proxy: OutboundProxy,
    failure: Option<SessionProxyError>,
    state: Arc<ExitState>,
}

impl SessionProxyPool for ExitPool {
    fn acquire(
        &self,
        source: RequestProxySource,
        route: ResponsesUpstream,
        scope: &str,
        transient: bool,
    ) -> Result<Arc<dyn SessionProxyLease>, SessionProxyError> {
        assert_eq!(source, self.source);
        assert_eq!(route, ResponsesUpstream::Codex);
        assert!(transient);
        let parsed: Vec<String> = serde_json::from_str(scope).unwrap();
        assert_eq!(parsed[0], "account-warmup-v1");
        assert_eq!(parsed[1], "acct_warmup");
        assert!(parsed[2].starts_with("warmup_"));
        self.state.scopes.lock().unwrap().push(scope.to_owned());
        self.state.acquired.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.state.active.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(ExitLease {
            proxy: self.proxy.clone(),
            state: self.state.clone(),
        }))
    }
}

async fn warmup_store(
    source: RequestProxySource,
    proxy: Option<OutboundProxy>,
) -> Arc<MemoryAccountStore> {
    let store = Arc::new(MemoryAccountStore::default());
    let mut prepared = CodexCredentialAdmin
        .prepare_import(ImportCodexOAuthCredential {
            account_id: "acct_warmup".to_owned(),
            name: "warmup".to_owned(),
            secret: secret("synthetic-warmup-token"),
            verified_account: profile("synthetic-workspace"),
            next_refresh_at: Some(Utc::now() + chrono::Duration::minutes(30)),
            enabled: true,
        })
        .unwrap();
    prepared.account = prepared
        .account
        .with_request_proxy_source(source)
        .with_outbound_proxy(proxy);
    store.create_account(prepared).await.unwrap();
    store
}

fn completed() -> ResponseTemplate {
    ResponseTemplate::new(200).insert_header("content-type", "text/event-stream")
        .set_body_string("data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n")
}

fn pool(
    source: RequestProxySource,
    server: &MockServer,
    failure: Option<SessionProxyError>,
) -> (Arc<dyn SessionProxyPool>, Arc<ExitState>) {
    let state = Arc::new(ExitState::default());
    (
        Arc::new(ExitPool {
            source,
            proxy: OutboundProxy::parse(&server.uri()).unwrap(),
            failure,
            state: state.clone(),
        }),
        state,
    )
}

#[tokio::test]
async fn warmup_uses_selected_pool_preserves_identity_and_releases_lease() {
    for source in [RequestProxySource::Mihomo, RequestProxySource::ProxyPool] {
        let direct = MockServer::start().await;
        let proxy = MockServer::start().await;
        let store = warmup_store(source, None).await;
        let before = store.account("acct_warmup").unwrap();
        let credential = store
            .repository()
            .load_runtime_credential(&before)
            .await
            .unwrap();
        let (pool, state) = pool(source, &proxy, None);
        let observed = state.clone();
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(move |_: &wiremock::Request| {
                assert_eq!(observed.active.load(Ordering::SeqCst), 1);
                completed()
            })
            .expect(2)
            .mount(&proxy)
            .await;
        let service = quota_service_with_base_url(&store, reqwest::Client::new(), direct.uri())
            .with_session_proxy_pool(Some(pool));
        for _ in 0..2 {
            let summary = service.execute_warmup("gpt-5.4").await.unwrap();
            assert_eq!(summary.warmed_up, 1);
            assert_eq!(summary.failed, 0);
            assert_eq!(state.active.load(Ordering::SeqCst), 0);
        }
        assert!(direct.received_requests().await.unwrap().is_empty());
        assert_eq!(store.account("acct_warmup").unwrap(), before);
        assert_eq!(
            *state.outcomes.lock().unwrap(),
            vec![SessionProxyOutcome::Completed; 2]
        );
        let scopes = state.scopes.lock().unwrap().clone();
        assert_ne!(scopes[0], scopes[1]);
        for request in proxy.received_requests().await.unwrap() {
            assert_eq!(
                request.headers["authorization"],
                "Bearer synthetic-warmup-token"
            );
            assert_eq!(request.headers["chatgpt-account-id"], "synthetic-workspace");
            assert_eq!(
                request.headers["user-agent"],
                wire_profile().snapshot().user_agent()
            );
            assert_eq!(
                request.headers["x-codex-installation-id"],
                credential.installation_id
            );
            assert!(!request.headers.contains_key("x-codex-turn-state"));
            assert!(!request.headers.contains_key("cookie"));
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["generate"], false);
            assert_eq!(body["reasoning"]["effort"], "low");
            assert_eq!(body["store"], false);
        }
        assert_eq!(
            store
                .repository()
                .load_runtime_credential(&before)
                .await
                .unwrap()
                .installation_id,
            credential.installation_id
        );
    }
}

#[tokio::test]
async fn warmup_account_source_keeps_saved_proxy_without_acquiring_pool() {
    let direct = MockServer::start().await;
    let proxy = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(completed())
        .expect(1)
        .mount(&proxy)
        .await;
    let store = warmup_store(
        RequestProxySource::Account,
        Some(OutboundProxy::parse(&proxy.uri()).unwrap()),
    )
    .await;
    let (pool, state) = pool(
        RequestProxySource::Mihomo,
        &proxy,
        Some(SessionProxyError::Unavailable),
    );
    let service = quota_service_with_base_url(&store, reqwest::Client::new(), direct.uri())
        .with_session_proxy_pool(Some(pool));
    assert_eq!(
        service.execute_warmup("gpt-5.4").await.unwrap().warmed_up,
        1
    );
    assert_eq!(state.acquired.load(Ordering::SeqCst), 0);
    assert!(direct.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn warmup_unavailable_pool_never_falls_back_to_direct() {
    for source in [RequestProxySource::Mihomo, RequestProxySource::ProxyPool] {
        for failure in [
            None,
            Some(SessionProxyError::Warming),
            Some(SessionProxyError::Unavailable),
            Some(SessionProxyError::Capacity),
        ] {
            let direct = MockServer::start().await;
            let store = warmup_store(source, None).await;
            let mut service =
                quota_service_with_base_url(&store, reqwest::Client::new(), direct.uri());
            if let Some(failure) = failure {
                service =
                    service.with_session_proxy_pool(Some(pool(source, &direct, Some(failure)).0));
            }
            let summary = service.execute_warmup("gpt-5.4").await.unwrap();
            assert_eq!(summary.failed, 1);
            assert_eq!(summary.warmed_up, 0);
            assert!(direct.received_requests().await.unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn warmup_business_rejections_do_not_penalize_pool_and_release_lease() {
    for status in [400, 401, 403, 429] {
        let direct = MockServer::start().await;
        let proxy = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_json(json!({"error":{"message":"synthetic rejection"}})),
            )
            .expect(1)
            .mount(&proxy)
            .await;
        let store = warmup_store(RequestProxySource::Mihomo, None).await;
        let (pool, state) = pool(RequestProxySource::Mihomo, &proxy, None);
        let service = quota_service_with_base_url(&store, reqwest::Client::new(), direct.uri())
            .with_session_proxy_pool(Some(pool));
        assert_eq!(service.execute_warmup("gpt-5.4").await.unwrap().failed, 1);
        assert_eq!(state.active.load(Ordering::SeqCst), 0);
        assert!(state.outcomes.lock().unwrap().is_empty());
        assert!(direct.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn warmup_cancellation_releases_exit_without_penalty() {
    let direct = MockServer::start().await;
    let proxy = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(completed().set_delay(Duration::from_secs(60)))
        .mount(&proxy)
        .await;
    let store = warmup_store(RequestProxySource::ProxyPool, None).await;
    let (pool, state) = pool(RequestProxySource::ProxyPool, &proxy, None);
    let service = quota_service_with_base_url(&store, reqwest::Client::new(), direct.uri())
        .with_session_proxy_pool(Some(pool));
    let mut future = Box::pin(service.execute_warmup("gpt-5.4"));
    tokio::select! {
        result = &mut future => panic!("request unexpectedly finished: {result:?}"),
        () = async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while proxy.received_requests().await.unwrap().is_empty() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.unwrap();
        } => {}
    }
    assert_eq!(state.active.load(Ordering::SeqCst), 1);
    drop(future);
    assert_eq!(state.active.load(Ordering::SeqCst), 0);
    assert!(state.outcomes.lock().unwrap().is_empty());
}

fn short_window(used: u64, remaining: Option<u64>, reset: i64) -> serde_json::Value {
    let mut window = json!({
        "used_percent": used,
        "limit_window_seconds": 18_000,
        "reset_at": reset,
    });
    if let Some(remaining) = remaining {
        window["reset_after_seconds"] = json!(remaining);
    }
    window
}

fn usage(window: serde_json::Value) -> serde_json::Value {
    json!({"rate_limit": {"allowed": true, "primary_window": window}})
}

async fn seed_quota(
    store: &MemoryAccountStore,
    account_id: &str,
    quota: serde_json::Value,
    observed_at: SystemTime,
) {
    let account = store.account(account_id).expect("account");
    assert_eq!(
        store
            .compare_and_swap_quota(QuotaObservation {
                quota_pause: None,
                plan_type: None,
                account_id: account.id().clone(),
                expected_revision: account.revision(),
                quota: OpaqueProviderData::new(quota.as_object().unwrap().clone()),
                observed_at,
                state: QuotaState::allowed(observed_at),
            })
            .await
            .expect("persist quota"),
        QuotaWriteOutcome::Updated
    );
}

async fn mount_completion(server: &MockServer, expected: u64) {
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
            "text/event-stream",
        ))
        .expect(expected)
        .mount(server)
        .await;
}

#[tokio::test]
async fn warmup_uses_account_windows_and_preserves_the_reset_threshold() {
    for (name, used, remaining, until_reset, named, warmed) in [
        ("acct_zero_full", 0, Some(18_000), 18_000, false, true),
        ("acct_zero_near_full", 0, Some(17_999), 17_999, false, true),
        ("acct_zero_lower", 0, Some(12_000), 12_000, false, true),
        ("acct_zero_missing_countdown", 0, None, 14_400, false, true),
        ("acct_passive_active", 1, None, 14_400, false, false),
        ("acct_near_reset", 1, Some(1_800), 1_800, false, true),
        ("acct_expired", 1, Some(3_600), -1, false, true),
        ("acct_reset_zero", 0, Some(0), -1, false, true),
        ("acct_named_active", 1, Some(3_600), 3_600, true, true),
    ] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, name).await;
        let window = short_window(used, remaining, Utc::now().timestamp() + until_reset);
        let quota = if named {
            json!({
                "rate_limit": {"allowed": true},
                "additional_rate_limits": [{
                    "limit_id": "other_feature",
                    "rate_limit": {
                        "primary_window": window,
                        "secondary_window": {
                            "used_percent": 100,
                            "limit_window_seconds": 604_800,
                            "reset_at": Utc::now().timestamp() + 604_800,
                        },
                    },
                }],
            })
        } else {
            usage(window)
        };
        seed_quota(&store, name, quota, SystemTime::now()).await;
        mount_completion(&server, u64::from(warmed)).await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());

        let summary = service.execute_warmup("test-model").await.unwrap();

        assert_eq!(
            (
                summary.warmed_up,
                summary.skipped_active,
                summary.skipped_exhausted,
                summary.failed
            ),
            (u64::from(warmed), u64::from(!warmed), 0, 0),
            "{name}"
        );
    }
}

#[tokio::test]
async fn warmup_skips_exhausted_account_windows_without_refreshing_quota() {
    for exhausted in [true, false] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_exhausted").await;
        let account = store.account("acct_exhausted").unwrap();
        let now = SystemTime::now();
        let quota = json!({"rate_limit": {"allowed": true, "secondary_window": {
            "used_percent": 100, "limit_window_seconds": 604_800,
            "reset_at": Utc::now().timestamp() + 604_800,
        }}});
        store
            .compare_and_swap_quota(QuotaObservation {
                quota_pause: None,
                plan_type: None,
                account_id: account.id().clone(),
                expected_revision: account.revision(),
                quota: OpaqueProviderData::new(quota.as_object().unwrap().clone()),
                observed_at: now,
                state: if exhausted {
                    QuotaState::exhausted(QuotaEvidence::UsageLimitReached, now, None)
                } else {
                    QuotaState::allowed(now)
                },
            })
            .await
            .unwrap();
        let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());
        let summary = service.execute_warmup("test-model").await.unwrap();
        assert_eq!(summary.skipped_exhausted, 1);
        assert_eq!(summary.warmed_up, 0);
        assert_eq!(summary.failed, 0);
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn warmup_consumes_completed_sse_and_persists_rate_limit_event() {
    let server = MockServer::start().await;
    let sse = concat!(
        "event: codex.rate_limits\n",
        "data: {\"type\":\"codex.rate_limits\",\"plan_type\":\"team\",\"rate_limits\":{\"allowed\":true,\"limit_reached\":false,\"primary\":{\"used_percent\":5,\"window_minutes\":300,\"reset_at\":1900000000}}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_warmup\",\"status\":\"completed\",\"output\":[]}}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_warmup_completed").await;
    let service = quota_service_with_base_url(
        &store,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        server.uri(),
    );

    let summary = service.execute_warmup("test-model").await.expect("warmup");

    assert_eq!(summary.warmed_up, 1);
    assert_eq!(summary.failed, 0);
    assert_eq!(
        store.account("acct_warmup_completed").unwrap().plan_type(),
        Some("team")
    );
    assert!(store.quota_json("acct_warmup_completed").is_some());
}

#[tokio::test]
async fn warmup_sends_the_lowest_reasoning_effort_supported_by_the_catalog() {
    let server = MockServer::start().await;
    let sse = concat!(
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_warmup_effort\",\"status\":\"completed\",\"output\":[]}}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_warmup_effort").await;
    let service = quota_service_with_base_url(
        &store,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        server.uri(),
    );

    let summary = service.execute_warmup("test-model").await.expect("warmup");

    assert_eq!(summary.warmed_up, 1);
    let requests = server.received_requests().await.expect("recorded requests");
    let decoded = if requests[0]
        .headers
        .get("content-encoding")
        .is_some_and(|value| value == "zstd")
    {
        zstd::stream::decode_all(requests[0].body.as_slice()).expect("zstd body should decode")
    } else {
        requests[0].body.clone()
    };
    let body: serde_json::Value =
        serde_json::from_slice(&decoded).expect("warmup body should be valid JSON");
    // 上游按目录拒绝目录外的档位，none 曾让全部定时预热只留下 error 终态
    assert_eq!(body["reasoning"]["effort"], "low");
}

#[tokio::test]
async fn warmup_rejects_failed_sse_despite_successful_http_headers() {
    let server = MockServer::start().await;
    let sse = concat!(
        "event: codex.rate_limits\n",
        "data: {\"type\":\"codex.rate_limits\",\"plan_type\":\"team\",\"rate_limits\":{\"allowed\":false,\"limit_reached\":true,\"primary\":{\"used_percent\":100,\"window_minutes\":300,\"reset_at\":1900000000}}}\n\n",
        "event: response.failed\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-allowed", "false")
                .set_body_raw(sse, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_warmup_failed").await;
    let service = quota_service_with_base_url(
        &store,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        server.uri(),
    );

    let summary = service.execute_warmup("test-model").await.expect("warmup");

    assert_eq!(summary.warmed_up, 0);
    assert_eq!(summary.failed, 1);
    assert!(store.quota_json("acct_warmup_failed").is_none());
}

#[tokio::test]
async fn warmup_skips_expired_credential() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_warmup_expired").await;
    let account = store.account("acct_warmup_expired").expect("account");
    persist_credential_state(&store, &account, CredentialState::Expired).await;
    let service = quota_service_with_base_url(
        &store,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        server.uri(),
    );

    let summary = service.execute_warmup("test-model").await.expect("warmup");

    assert_eq!(summary.warmed_up, 0);
    assert_eq!(summary.failed, 0);
}
