use super::*;
use gateway_core::account::{OutboundProxy, RequestProxySource, ResponsesUpstream};
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
            assert!(!request.headers.contains_key("x-codex-installation-id"));
            assert!(!request.headers.contains_key("x-codex-turn-state"));
            assert!(!request.headers.contains_key("cookie"));
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["generate"], false);
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
