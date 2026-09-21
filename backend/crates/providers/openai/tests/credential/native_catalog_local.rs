use super::catalog::{
    OFFICIAL_FIXTURE, client_scope, seed_account, service_with_catalog_cache, wire_profile,
};
use crate::support::{MemoryAccountStore, catalog_cache};

use std::collections::BTreeMap;
use std::net::Ipv6Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use futures::future::BoxFuture;
use gateway_core::account::{
    NewProviderAccount, OutboundProxy, ProviderAccount, ProviderAccountId, ProviderAccountStore,
    ProviderAccountUpdate,
};
use gateway_core::provider_ports::{
    ProviderStoreError,
    egress::{EgressMode, ProviderEgressAddress, ProviderEgressConfig, ProviderEgressStorePort},
};
use provider_openai::credential::{CodexCredentialCatalogError, CodexCredentialCatalogService};
use provider_openai::transport::egress::{CodexEgressError, CodexEgressRuntime};
use provider_openai::transport::profile::{
    CodexBundledReleaseProfile, CodexWireProfile, CodexWireProfileOverride, CodexWireProfileState,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct CatalogEgressStore(Mutex<ProviderEgressConfig>);

impl ProviderEgressStorePort for CatalogEgressStore {
    fn load(&self) -> BoxFuture<'_, Result<Arc<ProviderEgressConfig>, ProviderStoreError>> {
        Box::pin(async { Ok(Arc::new(self.0.lock().expect("config").clone())) })
    }

    fn ensure_fixed_affinity(
        &self,
        _: &ProviderAccountId,
    ) -> BoxFuture<'_, Result<Option<Ipv6Addr>, ProviderStoreError>> {
        Box::pin(async { panic!("catalog reads must not allocate affinity") })
    }
}

fn egress_config(account: &ProviderAccount) -> ProviderEgressConfig {
    ProviderEgressConfig {
        revision: 1,
        default_mode: EgressMode::FixedIpv6Reuse,
        addresses: vec![ProviderEgressAddress {
            id: "catalog-loopback".to_owned(),
            address: Ipv6Addr::LOCALHOST,
            enabled: true,
        }],
        account_overrides: BTreeMap::from([(account.id().clone(), None)]),
        fixed_bindings: BTreeMap::from([(account.id().clone(), Ipv6Addr::LOCALHOST)]),
    }
}

fn service_with_profile(
    store: &Arc<MemoryAccountStore>,
    url: String,
    profile: CodexWireProfileState,
) -> CodexCredentialCatalogService {
    CodexCredentialCatalogService::new(
        store.repository(),
        profile,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        url,
        catalog_cache(),
    )
}

async fn replace_account_facts(store: &MemoryAccountStore, account: ProviderAccount) {
    let loaded = store
        .load_current_credential(account.id())
        .await
        .expect("credential");
    store
        .delete_account(account.id())
        .await
        .expect("remove fixture");
    store
        .create_account(NewProviderAccount {
            model_access: Some(account.model_access().clone()),
            account,
            credential: loaded.credential,
        })
        .await
        .expect("replace fixture facts without rotating credential");
}

async fn read_headers(stream: &mut tokio::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let mut buffer = [0; 1024];
        let read = stream.read(&mut buffer).await.expect("request bytes");
        assert_ne!(read, 0, "request headers must be complete");
        bytes.extend_from_slice(&buffer[..read]);
        assert!(bytes.len() <= 32 * 1024, "bounded fixture headers");
        if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            return String::from_utf8(bytes).expect("HTTP headers");
        }
    }
}

async fn write_catalog(stream: &mut tokio::net::TcpStream) {
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        OFFICIAL_FIXTURE.len()
    );
    stream.write_all(headers.as_bytes()).await.expect("headers");
    stream
        .write_all(OFFICIAL_FIXTURE)
        .await
        .expect("catalog body");
    stream.shutdown().await.expect("fixture close");
}

#[tokio::test]
async fn native_catalog_freezes_effective_custom_profile_and_invalidates_default_release() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_profile_catalog").await;
    let scope = client_scope(&[account]);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .and(query_param("client_version", "0.154.0"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(OFFICIAL_FIXTURE, "application/json")
                .set_delay(Duration::from_millis(25)),
        )
        .expect(3)
        .mount(&server)
        .await;
    let profile = wire_profile();
    let initial = profile.snapshot();
    let service = service_with_profile(&store, server.uri(), profile.clone());
    let first = tokio::spawn({
        let service = service.clone();
        let scope = scope.clone();
        async move { service.client_model_catalog(&scope, "0.154.0").await }
    });
    timeout(Duration::from_secs(5), async {
        while server
            .received_requests()
            .await
            .expect("requests")
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("first request reached loopback");
    let custom = CodexWireProfile::parse_user_agent(
        "Codex Desktop/0.155.1 (Mac OS 15.1; arm64) catalog-contract (Codex Desktop; 26.901.11111)",
    )
    .expect("custom profile");
    profile
        .apply_override(CodexWireProfileOverride::Custom(Box::new(custom.clone())))
        .expect("custom override");
    service
        .client_model_catalog(&scope, "0.154.0")
        .await
        .expect("custom");
    first.await.expect("join").expect("original frozen request");
    profile.update_bundled_release(&CodexBundledReleaseProfile {
        codex_version: "0.156.0".to_owned(),
        desktop_version: "26.902.11111".to_owned(),
        desktop_build: "902".to_owned(),
        verified_at: Utc::now(),
    });
    service
        .client_model_catalog(&scope, "0.154.0")
        .await
        .expect("default release must not replace custom cache identity");
    profile
        .apply_override(CodexWireProfileOverride::Default)
        .expect("default");
    service
        .client_model_catalog(&scope, "0.154.0")
        .await
        .expect("new default");
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 3);
    for (request, expected) in requests.iter().zip([initial, custom, profile.snapshot()]) {
        assert_eq!(request.headers["user-agent"], expected.user_agent());
        assert_eq!(request.headers["version"], expected.codex_version);
        assert_eq!(request.headers["originator"], expected.originator);
        assert_eq!(
            request.headers["authorization"],
            "Bearer access-acct_profile_catalog"
        );
        assert_eq!(
            request.headers["chatgpt-account-id"],
            "chatgpt-acct_profile_catalog"
        );
        // This endpoint has no installation-ID wire header in the established profile.
        assert!(!request.headers.contains_key("x-codex-installation-id"));
        assert!(!request.headers.contains_key("x-codex-turn-metadata"));
    }
    server.verify().await;
}

#[tokio::test]
async fn native_catalog_rechecks_plan_user_account_and_live_eligibility() {
    let store = Arc::new(MemoryAccountStore::default());
    let original = seed_account(&store, "acct_catalog_facts").await;
    let scope = client_scope(std::slice::from_ref(&original));
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(OFFICIAL_FIXTURE, "application/json"))
        .expect(4)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    service
        .client_model_catalog(&scope, "0.154.0")
        .await
        .expect("original");
    store
        .update_account(ProviderAccountUpdate {
            account_id: original.id().clone(),
            name: original.name().to_owned(),
            email: original.email().map(str::to_owned),
            plan_type: Some("team".to_owned()),
        })
        .await
        .expect("plan-only update");
    service
        .client_model_catalog(&scope, "0.154.0")
        .await
        .expect("new plan");
    let changed_user = ProviderAccount::new(
        original.id().clone(),
        original.provider().clone(),
        original.name().to_owned(),
        Some("different-upstream-user".to_owned()),
        original.authentication_kind().to_owned(),
        original.revision(),
        original.access_token_expires_at(),
    )
    .with_profile(
        original.email().map(str::to_owned),
        original.upstream_account_id().map(str::to_owned),
        Some("team".to_owned()),
    );
    replace_account_facts(&store, changed_user.clone()).await;
    service
        .client_model_catalog(&scope, "0.154.0")
        .await
        .expect("new user");
    replace_account_facts(
        &store,
        changed_user.with_profile(
            None,
            Some("different-workspace".to_owned()),
            Some("team".to_owned()),
        ),
    )
    .await;
    service
        .client_model_catalog(&scope, "0.154.0")
        .await
        .expect("new upstream account");
    assert_eq!(
        store
            .account("acct_catalog_facts")
            .expect("account")
            .revision(),
        original.revision()
    );
    store
        .set_enabled(original.id(), false)
        .await
        .expect("disable");
    assert!(matches!(
        service.client_model_catalog(&scope, "0.154.0").await,
        Err(CodexCredentialCatalogError::NoEligibleCredential)
    ));
    store.delete_account(original.id()).await.expect("delete");
    assert!(matches!(
        service.client_model_catalog(&scope, "0.154.0").await,
        Err(CodexCredentialCatalogError::NoEligibleCredential)
    ));
    server.verify().await;
}

#[tokio::test]
async fn native_catalog_uses_account_proxy_and_does_not_reuse_direct_catalog() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_catalog_proxy").await;
    let scope = client_scope(std::slice::from_ref(&account));
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(OFFICIAL_FIXTURE, "application/json"))
        .expect(1)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    service
        .client_model_catalog(&scope, "0.154.0")
        .await
        .expect("direct");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("proxy");
    let proxy = OutboundProxy::parse(&format!(
        "http://catalog-user:catalog-pass@{}",
        listener.local_addr().expect("proxy address")
    ))
    .expect("proxy");
    replace_account_facts(&store, account.with_outbound_proxy(Some(proxy))).await;
    let proxy_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("proxy connection");
        let request = read_headers(&mut stream).await;
        write_catalog(&mut stream).await;
        request
    });
    timeout(
        Duration::from_secs(5),
        service.client_model_catalog(&scope, "0.154.0"),
    )
    .await
    .expect("proxy deadline")
    .expect("proxied catalog");
    let request = timeout(Duration::from_secs(5), proxy_task)
        .await
        .expect("proxy captured")
        .expect("join");
    assert!(request.starts_with(&format!(
        "GET {}/codex/models?client_version=0.154.0 HTTP/1.1\r\n",
        server.uri()
    )));
    let request = request.to_ascii_lowercase();
    assert!(
        request.contains("proxy-authorization: basic y2f0ywxvzy11c2vyomnhdgfsb2ctcgfzcw==\r\n")
    );
    assert!(request.contains("authorization: bearer access-acct_catalog_proxy\r\n"));
    assert!(request.contains("chatgpt-account-id: chatgpt-acct_catalog_proxy\r\n"));
    server.verify().await;
}

#[tokio::test]
async fn native_catalog_uses_ipv6_source_and_fences_cached_data_after_route_reload() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_catalog_ipv6").await;
    let scope = client_scope(std::slice::from_ref(&account));
    let egress_store = Arc::new(CatalogEgressStore(Mutex::new(egress_config(&account))));
    let runtime = CodexEgressRuntime::load(egress_store.clone())
        .await
        .expect("runtime");
    let listener = TcpListener::bind("[::1]:0").await.expect("IPv6 loopback");
    let url = format!("http://{}", listener.local_addr().expect("listen address"));
    let service = service_with_catalog_cache(&store, url, catalog_cache())
        .with_egress_runtime(runtime.clone());
    let server = tokio::spawn(async move {
        let (mut stream, peer) = listener.accept().await.expect("IPv6 connection");
        assert_eq!(peer.ip(), std::net::IpAddr::V6(Ipv6Addr::LOCALHOST));
        let request = read_headers(&mut stream).await;
        write_catalog(&mut stream).await;
        request
    });
    timeout(
        Duration::from_secs(5),
        service.client_model_catalog(&scope, "0.154.0"),
    )
    .await
    .expect("catalog deadline")
    .expect("IPv6 catalog");
    let headers = server.await.expect("server").to_ascii_lowercase();
    assert!(headers.contains("authorization: bearer access-acct_catalog_ipv6\r\n"));
    assert!(headers.contains("chatgpt-account-id: chatgpt-acct_catalog_ipv6\r\n"));
    assert!(headers.contains(
        &format!("user-agent: {}\r\n", wire_profile().snapshot().user_agent()).to_ascii_lowercase()
    ));
    {
        let mut config = egress_store.0.lock().expect("config");
        config.revision += 1;
        config.addresses[0].enabled = false;
    }
    runtime.reload().await.expect("disable source");
    assert!(
        matches!(
            service.client_model_catalog(&scope, "0.154.0").await,
            Err(CodexCredentialCatalogError::Upstream {
                egress: Some(CodexEgressError::SourceDisabled),
                status: None,
                ..
            })
        ),
        "route revision must invalidate a previously successful catalog"
    );
}

#[tokio::test]
async fn native_catalog_explicit_ipv6_never_falls_back_to_ipv4() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_catalog_ipv4").await;
    let scope = client_scope(std::slice::from_ref(&account));
    let egress_store = Arc::new(CatalogEgressStore(Mutex::new(egress_config(&account))));
    let runtime = CodexEgressRuntime::load(egress_store)
        .await
        .expect("runtime");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("IPv4 fixture");
    let service = service_with_catalog_cache(
        &store,
        format!("http://{}", listener.local_addr().expect("address")),
        catalog_cache(),
    )
    .with_egress_runtime(runtime);
    assert!(matches!(
        timeout(
            Duration::from_secs(5),
            service.client_model_catalog(&scope, "0.154.0")
        )
        .await
        .expect("deadline"),
        Err(CodexCredentialCatalogError::Upstream {
            egress: Some(_),
            status: None,
            ..
        })
    ));
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err(),
        "no IPv4 payload"
    );
}

#[tokio::test]
async fn native_catalog_invalidations_preserve_inflight_bound_and_cancelled_slots_are_reusable() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_catalog_inflight").await;
    let scope = client_scope(&[account]);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(OFFICIAL_FIXTURE, "application/json"))
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    let versions = (0..32)
        .map(|index| format!("0.154.{index}"))
        .collect::<Vec<_>>();
    let mut requests = versions
        .iter()
        .map(|version| Box::pin(service.client_model_catalog(&scope, version)))
        .collect::<Vec<_>>();
    for request in &mut requests {
        assert!(futures::poll!(request.as_mut()).is_pending());
    }
    assert!(matches!(
        service.client_model_catalog(&scope, "0.155.0").await,
        Err(CodexCredentialCatalogError::Cache)
    ));
    service
        .invalidate()
        .expect("invalidate while requests are pending");
    assert!(matches!(
        service.client_model_catalog(&scope, "0.155.0").await,
        Err(CodexCredentialCatalogError::Cache)
    ));
    drop(requests);
    timeout(
        Duration::from_secs(5),
        service.client_model_catalog(&scope, "0.155.0"),
    )
    .await
    .expect("deadline")
    .expect("cancelled owners release cache capacity");
}

#[tokio::test]
async fn native_catalog_tries_at_most_three_in_scope_accounts_and_stops_on_first_success() {
    let store = Arc::new(MemoryAccountStore::default());
    let mut accounts = Vec::new();
    for index in 0..4 {
        accounts.push(seed_account(&store, &format!("acct_catalog_attempt_{index}")).await);
    }
    let scope = client_scope(&accounts);
    let server = MockServer::start().await;
    for index in 0..3 {
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .and(header(
                "authorization",
                format!("Bearer access-acct_catalog_attempt_{index}"),
            ))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
    }
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    assert!(
        service
            .client_model_catalog(&scope, "0.154.0")
            .await
            .is_err()
    );
    assert_eq!(server.received_requests().await.expect("requests").len(), 3);
    server.verify().await;
    server.reset().await;
    for index in 0..2 {
        let response = if index == 0 {
            ResponseTemplate::new(503)
        } else {
            ResponseTemplate::new(200).set_body_raw(OFFICIAL_FIXTURE, "application/json")
        };
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .and(header(
                "authorization",
                format!("Bearer access-acct_catalog_attempt_{index}"),
            ))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
    }
    service.invalidate().expect("reset failed catalog");
    service
        .client_model_catalog(&scope, "0.154.0")
        .await
        .expect("second account succeeds");
    assert_eq!(server.received_requests().await.expect("requests").len(), 2);
    server.verify().await;
}
