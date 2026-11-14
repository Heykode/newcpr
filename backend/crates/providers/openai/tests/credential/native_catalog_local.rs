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
use wiremock::matchers::{method, path};
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
        crate::support::runtime_policy(),
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
async fn excel_catalog_uses_independent_access_and_never_the_legacy_model_list() {
    use gateway_core::account::ResponsesUpstream;
    use serde_json::{Value, json};

    let store = Arc::new(MemoryAccountStore::default());
    seed_account(&store, "acct_excel_catalog").await;
    store.seed_excel_credential("acct_excel_catalog", true);
    store.set_responses_upstream("acct_excel_catalog", ResponsesUpstream::Excel);
    let account = store.account("acct_excel_catalog").unwrap();
    let native_before = account.clone().with_excel_credential(None);
    let scope = client_scope(std::slice::from_ref(&account));
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/basispoints/api/responses/access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "allowed":true, "model_catalog":{"models":[
                {"id":"gpt-5.4","label":"GPT 5.4","efforts":[{"value":"high"}]},
                {"id":"gpt-5.6-sol","model_picker_enabled":false}
            ]}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    let catalog = service.refresh_account_catalog(account.id()).await.unwrap();
    assert_eq!(catalog.models(), ["gpt-5.4"]);
    let models = service.client_model_catalog(&scope).await.unwrap();
    assert_eq!(models.len(), 1);
    let document: Value = serde_json::from_slice(models[0].payload.body()).unwrap();
    assert_eq!(document["source"], "excel_bridge");
    assert_eq!(document["multi_agent_version"], Value::Null);
    let current = store.account(account.id().as_str()).unwrap();
    assert_eq!(current.with_excel_credential(None), native_before);
    assert!(
        store
            .load_account_catalogs(account.provider())
            .await
            .unwrap()
            .is_empty(),
        "BPS catalogs must not overwrite persisted Codex snapshots"
    );
    let request = server.received_requests().await.unwrap().remove(0);
    assert!(
        request.headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Bearer e30.")
    );
    assert_eq!(request.headers["x-basispoints-auth-mode"], "chatgpt");
    assert!(request.url.query().unwrap().contains("include_models=true"));

    let restored = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    restored.restore().await.unwrap();
    assert!(
        restored.cached().unwrap().is_none(),
        "Excel restarts must fetch real access again"
    );
}

#[tokio::test]
async fn excel_catalog_failure_does_not_fabricate_models_or_fall_back_to_codex() {
    use gateway_core::account::ResponsesUpstream;
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_excel_catalog_failed").await;
    store.set_responses_upstream(account.id().as_str(), ResponsesUpstream::Excel);
    let server = MockServer::start().await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    assert!(service.refresh_account_catalog(account.id()).await.is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
    store.seed_excel_credential(account.id().as_str(), true);
    Mock::given(path("/basispoints/api/responses/access"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    assert!(service.refresh_account_catalog(account.id()).await.is_err());
    assert!(service.cached().unwrap().is_none());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        !requests
            .iter()
            .any(|request| request.url.path().contains("codex"))
    );
}

#[tokio::test]
async fn catalog_switch_uses_the_selected_credential_and_keeps_native_descriptors() {
    use gateway_core::account::ResponsesUpstream;
    use serde_json::{Value, json};
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_excel_catalog_switch").await;
    store.seed_excel_credential(account.id().as_str(), true);
    store.set_responses_upstream(account.id().as_str(), ResponsesUpstream::Excel);
    let scope = client_scope(std::slice::from_ref(&account));
    let server = MockServer::start().await;
    Mock::given(path("/basispoints/api/responses/access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "allowed":true,"model_catalog":{"models":[{"id":"gpt-5.4"}]}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let mut fixture: Value = serde_json::from_slice(OFFICIAL_FIXTURE).unwrap();
    fixture["models"][0]["multi_agent_version"] = json!("v2");
    Mock::given(path("/codex/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture.clone()))
        .expect(1)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    service.refresh_account_catalog(account.id()).await.unwrap();
    store.set_responses_upstream(account.id().as_str(), ResponsesUpstream::Codex);
    service
        .invalidate_accounts(&[account.id().clone()])
        .await
        .unwrap();
    service.refresh_account_catalog(account.id()).await.unwrap();
    let models = service.client_model_catalog(&scope).await.unwrap();
    let descriptor: Value = serde_json::from_slice(models[0].payload.body()).unwrap();
    assert_eq!(descriptor, fixture["models"][0]);
    assert_eq!(
        store.account(account.id().as_str()).unwrap().revision(),
        account.revision()
    );
}

#[tokio::test]
async fn background_catalog_refresh_freezes_effective_profile_without_installation_header() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_profile_catalog").await;
    let installation_id = store
        .repository()
        .load_runtime_credential(&account)
        .await
        .unwrap()
        .installation_id;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
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
        let id = account.id().clone();
        async move { service.refresh_account_catalog(&id).await }
    });
    timeout(Duration::from_secs(5), async {
        while server.received_requests().await.unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let custom = CodexWireProfile::parse_user_agent(
        "Codex Desktop/0.155.1 (Mac OS 15.1; arm64) catalog-contract (Codex Desktop; 26.901.11111)",
    )
    .unwrap();
    profile
        .apply_override(CodexWireProfileOverride::Custom(Box::new(custom.clone())))
        .unwrap();
    first.await.unwrap().unwrap();
    service.refresh_account_catalog(account.id()).await.unwrap();
    profile.update_bundled_release(&CodexBundledReleaseProfile {
        codex_version: "0.156.0".into(),
        desktop_version: "26.902.11111".into(),
        desktop_build: "902".into(),
        verified_at: Utc::now(),
    });
    service
        .client_model_catalog(&client_scope(std::slice::from_ref(&account)))
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    profile
        .apply_override(CodexWireProfileOverride::Default)
        .unwrap();
    service.refresh_account_catalog(account.id()).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    for (request, expected) in requests.iter().zip([initial, custom, profile.snapshot()]) {
        assert_eq!(request.headers["user-agent"], expected.user_agent());
        assert_eq!(request.headers["version"], expected.codex_version);
        assert_eq!(request.headers["originator"], expected.originator);
        assert!(!request.headers.contains_key("x-codex-installation-id"));
        assert_eq!(
            store
                .repository()
                .load_runtime_credential(&account)
                .await
                .unwrap()
                .installation_id,
            installation_id
        );
        assert_eq!(
            request.headers["authorization"],
            "Bearer access-acct_profile_catalog"
        );
        assert_eq!(
            request
                .url
                .query_pairs()
                .find(|(key, _)| key == "client_version")
                .unwrap()
                .1,
            expected.codex_version
        );
    }
    server.verify().await;
}

#[tokio::test]
async fn account_catalog_export_freezes_current_profile_and_keeps_custom_selection() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_export_profile").await;
    let installation_id = store
        .repository()
        .load_runtime_credential(&account)
        .await
        .expect("credential")
        .installation_id;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(OFFICIAL_FIXTURE, "application/json")
                .set_delay(Duration::from_millis(25)),
        )
        .expect(4)
        .mount(&server)
        .await;
    let profile = wire_profile();
    let initial = profile.snapshot();
    let service = service_with_profile(&store, server.uri(), profile.clone());
    let first = tokio::spawn({
        let service = service.clone();
        let account = account.clone();
        async move { service.account_catalog_documents(&account).await }
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
    .expect("first export reached loopback");
    let custom = CodexWireProfile::parse_user_agent(
        "Codex Desktop/0.155.1 (Mac OS 15.1; arm64) catalog-contract (Codex Desktop; 26.901.11111)",
    )
    .expect("custom profile");
    profile
        .apply_override(CodexWireProfileOverride::Custom(Box::new(custom.clone())))
        .expect("custom override");
    service
        .account_catalog_documents(&account)
        .await
        .expect("custom export");
    first.await.expect("join").expect("frozen default export");
    profile.update_bundled_release(&CodexBundledReleaseProfile {
        codex_version: "0.156.0".to_owned(),
        desktop_version: "26.902.11111".to_owned(),
        desktop_build: "902".to_owned(),
        verified_at: Utc::now(),
    });
    service
        .account_catalog_documents(&account)
        .await
        .expect("custom preserved");
    profile
        .apply_override(CodexWireProfileOverride::Default)
        .expect("default");
    service
        .account_catalog_documents(&account)
        .await
        .expect("new default");
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 4);
    for (request, expected) in
        requests
            .iter()
            .zip([initial, custom.clone(), custom, profile.snapshot()])
    {
        assert_eq!(request.headers["user-agent"], expected.user_agent());
        assert_eq!(request.headers["version"], expected.codex_version);
        assert_eq!(request.headers["originator"], expected.originator);
        assert!(
            request
                .url
                .query_pairs()
                .any(|(key, value)| { key == "client_version" && value == expected.codex_version })
        );
        assert_eq!(
            request.headers["authorization"],
            "Bearer access-acct_export_profile"
        );
        assert_eq!(
            request.headers["chatgpt-account-id"],
            "chatgpt-acct_export_profile"
        );
        assert!(!request.headers.contains_key("x-codex-installation-id"));
        assert_eq!(
            store
                .repository()
                .load_runtime_credential(&account)
                .await
                .unwrap()
                .installation_id,
            installation_id
        );
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
        .refresh_account_catalog(original.id())
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
        .refresh_account_catalog(original.id())
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
        .refresh_account_catalog(original.id())
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
        .refresh_account_catalog(original.id())
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
        service.client_model_catalog(&scope).await,
        Err(CodexCredentialCatalogError::NoEligibleCredential)
    ));
    store.delete_account(original.id()).await.expect("delete");
    assert!(matches!(
        service.client_model_catalog(&scope).await,
        Err(CodexCredentialCatalogError::NoEligibleCredential)
    ));
    server.verify().await;
}

#[tokio::test]
async fn native_catalog_uses_account_proxy_and_does_not_reuse_direct_catalog() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_catalog_proxy").await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(OFFICIAL_FIXTURE, "application/json"))
        .expect(1)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    service
        .refresh_account_catalog(account.id())
        .await
        .expect("direct");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("proxy");
    let proxy = OutboundProxy::parse(&format!(
        "http://catalog-user:catalog-pass@{}",
        listener.local_addr().expect("proxy address")
    ))
    .expect("proxy");
    let account = account.with_outbound_proxy(Some(proxy));
    replace_account_facts(&store, account.clone()).await;
    let proxy_task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("proxy connection");
            requests.push(read_headers(&mut stream).await);
            write_catalog(&mut stream).await;
        }
        requests
    });
    timeout(
        Duration::from_secs(5),
        service.refresh_account_catalog(account.id()),
    )
    .await
    .expect("proxy deadline")
    .expect("proxied catalog");
    service
        .account_catalog_documents(&account)
        .await
        .expect("proxied admin export");
    let requests = timeout(Duration::from_secs(5), proxy_task)
        .await
        .expect("proxy captured")
        .expect("join");
    for (request, version) in requests.iter().zip(["0.144.0", "0.144.0"]) {
        assert!(request.starts_with(&format!(
            "GET {}/codex/models?client_version={version} HTTP/1.1\r\n",
            server.uri()
        )));
        let request = request.to_ascii_lowercase();
        assert!(
            request.contains("proxy-authorization: basic y2f0ywxvzy11c2vyomnhdgfsb2ctcgfzcw==\r\n")
        );
        assert!(request.contains("authorization: bearer access-acct_catalog_proxy\r\n"));
        assert!(request.contains("chatgpt-account-id: chatgpt-acct_catalog_proxy\r\n"));
    }
    server.verify().await;
}

#[tokio::test]
async fn native_catalog_uses_ipv6_source_and_fences_cached_data_after_route_reload() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_catalog_ipv6").await;
    let egress_store = Arc::new(CatalogEgressStore(Mutex::new(egress_config(&account))));
    let runtime = CodexEgressRuntime::load(egress_store.clone())
        .await
        .expect("runtime");
    let listener = TcpListener::bind("[::1]:0").await.expect("IPv6 loopback");
    let url = format!("http://{}", listener.local_addr().expect("listen address"));
    let service = service_with_catalog_cache(&store, url, catalog_cache())
        .with_egress_runtime(runtime.clone());
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let (mut stream, peer) = listener.accept().await.expect("IPv6 connection");
            assert_eq!(peer.ip(), std::net::IpAddr::V6(Ipv6Addr::LOCALHOST));
            requests.push(read_headers(&mut stream).await);
            write_catalog(&mut stream).await;
        }
        requests.join("\n")
    });
    timeout(
        Duration::from_secs(5),
        service.refresh_account_catalog(account.id()),
    )
    .await
    .expect("catalog deadline")
    .expect("IPv6 catalog");
    service
        .account_catalog_documents(&account)
        .await
        .expect("IPv6 admin export");
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
            service.refresh_account_catalog(account.id()).await,
            Err(CodexCredentialCatalogError::Upstream {
                egress: Some(CodexEgressError::SourceDisabled),
                status: None,
                ..
            })
        ),
        "route revision must invalidate a previously successful catalog"
    );
    assert!(matches!(
        service.account_catalog_documents(&account).await,
        Err(CodexCredentialCatalogError::Upstream {
            egress: Some(CodexEgressError::SourceDisabled),
            status: None,
            ..
        })
    ));
}

#[tokio::test]
async fn native_catalog_explicit_ipv6_never_falls_back_to_ipv4() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_catalog_ipv4").await;
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
            service.refresh_account_catalog(account.id())
        )
        .await
        .expect("deadline"),
        Err(CodexCredentialCatalogError::Upstream {
            egress: Some(_),
            status: None,
            ..
        })
    ));
    assert!(matches!(
        timeout(
            Duration::from_secs(5),
            service.account_catalog_documents(&account)
        )
        .await
        .expect("export deadline"),
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
