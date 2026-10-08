use super::*;
use std::collections::BTreeMap;

use crate::{
    config::OpenAiConfig,
    transport::{
        CodexBackendClient, CodexBackendTransport, CodexRequestContext,
        protocol::responses::CodexResponsesRequest,
        session_proxy::SessionProxyHold,
        websocket::{CodexWebSocketExchangeError, CodexWebSocketPool},
    },
};
use futures::{SinkExt, StreamExt, TryStreamExt, future::BoxFuture};
use gateway_core::provider_ports::session_proxy::{SessionProxyLease, SessionProxyOutcome};
use gateway_core::{
    account::{CredentialRevision, OutboundProxy},
    provider_ports::{ProviderStoreError, egress::ProviderEgressAddress},
    routing::ProviderKind,
};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct Store(Mutex<ProviderEgressConfig>);

impl ProviderEgressStorePort for Store {
    fn load(&self) -> BoxFuture<'_, Result<Arc<ProviderEgressConfig>, ProviderStoreError>> {
        Box::pin(async { Ok(Arc::new(self.0.lock().unwrap().clone())) })
    }

    fn ensure_fixed_affinity(
        &self,
        _: &ProviderAccountId,
    ) -> BoxFuture<'_, Result<Option<Ipv6Addr>, ProviderStoreError>> {
        Box::pin(async { panic!("routing must not allocate fixed affinity") })
    }
}

fn account(id: usize) -> ProviderAccount {
    ProviderAccount::new(
        ProviderAccountId::new(format!("acct_round_robin_{id}")).unwrap(),
        ProviderKind::new("openai").unwrap(),
        format!("round-robin-{id}"),
        Some(format!("round-robin-{id}")),
        "oauth".to_owned(),
        CredentialRevision::new(1).unwrap(),
        None,
    )
}

fn addresses() -> [Ipv6Addr; 4] {
    [1, 2, 3, 4].map(|suffix| Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, suffix))
}

async fn fixture(
    sources: &[Ipv6Addr],
    mode: EgressMode,
) -> (Arc<Store>, Arc<CodexEgressRuntime>, [ProviderAccount; 3]) {
    let accounts = [account(0), account(1), account(2)];
    let store = Arc::new(Store(Mutex::new(ProviderEgressConfig {
        revision: 1,
        default_mode: mode,
        addresses: sources
            .iter()
            .enumerate()
            .map(|(index, address)| ProviderEgressAddress {
                id: format!("source-{index}"),
                address: *address,
                enabled: true,
            })
            .collect(),
        account_overrides: accounts.iter().map(|a| (a.id().clone(), None)).collect(),
        fixed_bindings: accounts
            .iter()
            .filter_map(|a| sources.first().map(|source| (a.id().clone(), *source)))
            .collect(),
    })));
    let runtime = CodexEgressRuntime::load(store.clone()).await.unwrap();
    (store, runtime, accounts)
}

fn selected(runtime: &CodexEgressRuntime, account: &ProviderAccount) -> Ipv6Addr {
    runtime.select(account).unwrap().unwrap().source
}

#[tokio::test]
async fn rotating_modes_and_accounts_share_one_cursor_across_reload() {
    let sources = addresses();
    let (store, runtime, accounts) = fixture(&sources, EgressMode::RandomIpv6Reuse).await;
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
    {
        let mut state = store.0.lock().unwrap();
        state.revision += 1;
        state
            .account_overrides
            .insert(accounts[1].id().clone(), Some(EgressMode::RandomIpv6Fresh));
    }
    runtime.reload().await.unwrap();
    for (account, source) in [
        (&accounts[1], sources[1]),
        (&accounts[2], sources[2]),
        (&accounts[0], sources[3]),
        (&accounts[2], sources[0]),
        (&accounts[1], sources[1]),
    ] {
        assert_eq!(selected(&runtime, account), source);
    }
}

#[tokio::test]
async fn parallel_accounts_consume_each_source_once_per_cycle() {
    let sources = addresses();
    let (_, runtime, accounts) = fixture(&sources, EgressMode::RandomIpv6Reuse).await;
    let barrier = Arc::new(std::sync::Barrier::new(32));
    let workers: Vec<_> = (0..32)
        .map(|index| {
            let runtime = runtime.clone();
            let account = accounts[index % accounts.len()].clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                (0..8)
                    .map(|_| selected(&runtime, &account))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut counts = BTreeMap::new();
    for source in workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
    {
        *counts.entry(source).or_insert(0) += 1;
    }
    assert_eq!(
        counts,
        sources.into_iter().map(|source| (source, 64)).collect()
    );
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
}

#[tokio::test]
async fn disabled_and_cooling_sources_are_skipped_without_reindexing_the_cursor() {
    let sources = addresses();
    let (store, runtime, accounts) = fixture(&sources, EgressMode::RandomIpv6Reuse).await;
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
    store.0.lock().unwrap().addresses[0].enabled = false;
    runtime.reload().await.unwrap();
    runtime.source_health.lock().unwrap().insert(
        sources[2],
        SourceHealth {
            available_until: None,
            blocked_until: Some(Instant::now() + Duration::from_secs(60)),
        },
    );
    assert_eq!(selected(&runtime, &accounts[1]), sources[1]);
    assert_eq!(selected(&runtime, &accounts[2]), sources[3]);
    assert_eq!(selected(&runtime, &accounts[0]), sources[1]);
    runtime
        .source_health
        .lock()
        .unwrap()
        .get_mut(&sources[2])
        .unwrap()
        .blocked_until = None;
    assert_eq!(selected(&runtime, &accounts[1]), sources[2]);
}

#[tokio::test]
async fn fixed_default_and_invalid_accounts_do_not_advance_rotation() {
    let sources = addresses();
    let (store, runtime, accounts) = fixture(&sources, EgressMode::RandomIpv6Reuse).await;
    {
        let mut state = store.0.lock().unwrap();
        state
            .account_overrides
            .insert(accounts[1].id().clone(), Some(EgressMode::FixedIpv6Reuse));
        state
            .account_overrides
            .insert(accounts[2].id().clone(), Some(EgressMode::Unchanged));
    }
    runtime.reload().await.unwrap();
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
    assert_eq!(selected(&runtime, &accounts[1]), sources[0]);
    assert!(runtime.select(&accounts[2]).unwrap().is_none());
    assert_eq!(
        runtime.select(&account(99)),
        Err(CodexEgressError::ConfigurationChanged)
    );
    let proxied = accounts[0].clone().with_outbound_proxy(Some(
        gateway_core::account::OutboundProxy::parse("http://127.0.0.1:9").unwrap(),
    ));
    assert_eq!(
        runtime.select(&proxied),
        Err(CodexEgressError::ProxyConflict)
    );
    assert_eq!(selected(&runtime, &accounts[0]), sources[1]);
}

#[tokio::test]
async fn empty_single_source_and_all_cooling_pools_keep_existing_error_semantics() {
    let sources = addresses();
    let (store, runtime, accounts) = fixture(&[], EgressMode::RandomIpv6Fresh).await;
    assert_eq!(
        runtime.select(&accounts[0]),
        Err(CodexEgressError::EmptyPool)
    );
    store
        .0
        .lock()
        .unwrap()
        .addresses
        .push(ProviderEgressAddress {
            id: "only".into(),
            address: sources[0],
            enabled: true,
        });
    runtime.reload().await.unwrap();
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
    assert_eq!(selected(&runtime, &accounts[1]), sources[0]);
    runtime.source_health.lock().unwrap().insert(
        sources[0],
        SourceHealth {
            available_until: None,
            blocked_until: Some(Instant::now() + Duration::from_secs(60)),
        },
    );
    assert_eq!(selected(&runtime, &accounts[2]), sources[0]);
    assert_eq!(
        runtime.ensure_source_ready(sources[0]),
        Err(CodexEgressError::SourceUnavailable)
    );
    store.0.lock().unwrap().addresses[0].enabled = false;
    runtime.reload().await.unwrap();
    assert_eq!(
        runtime.select(&accounts[0]),
        Err(CodexEgressError::EmptyPool)
    );
}

#[tokio::test]
async fn reordered_pool_follows_last_address_and_removed_cursor_starts_at_front() {
    let sources = addresses();
    let (store, runtime, accounts) = fixture(&sources, EgressMode::RandomIpv6Reuse).await;
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
    store.0.lock().unwrap().addresses.reverse();
    runtime.reload().await.unwrap();
    assert_eq!(selected(&runtime, &accounts[1]), sources[3]);
    store.0.lock().unwrap().addresses.remove(0);
    runtime.reload().await.unwrap();
    assert_eq!(selected(&runtime, &accounts[2]), sources[2]);
}

#[tokio::test]
async fn excel_ipv6_http_sse_reuses_connections_but_isolates_accounts() {
    use crate::{
        config::OpenAiConfig,
        transport::{
            CodexBackendClient, CodexBackendTransport, CodexRequestContext,
            excel::{ClientTools, ExcelPreparedRequest, RESPONSES_PATH, prepare_request},
            protocol::responses::CodexResponsesRequest,
        },
    };
    use serde_json::json;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::timeout,
    };

    async fn reply(stream: &mut TcpStream) {
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = stream.read(&mut buffer).await.unwrap();
            assert_ne!(
                count, 0,
                "expected another HTTP request on the selected connection"
            );
            bytes.extend_from_slice(&buffer[..count]);
            assert!(bytes.len() < 64 * 1024);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let head = std::str::from_utf8(&bytes[..end]).unwrap();
                assert!(head.starts_with("POST /basispoints/api/responses HTTP/1.1"));
                assert!(!head.to_ascii_lowercase().contains("upgrade: websocket"));
                let length: usize = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let body = "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_ipv6_fixture\",\"status\":\"completed\",\"output\":[]}}\n\n";
        stream.write_all(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}", body.len(),
        ).as_bytes()).await.unwrap();
    }

    for mode in [
        EgressMode::FixedIpv6Reuse,
        EgressMode::RandomIpv6Reuse,
        EgressMode::FixedIpv6Fresh,
        EgressMode::RandomIpv6Fresh,
    ] {
        let listener = TcpListener::bind("[::1]:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (_, runtime, accounts) = fixture(&[Ipv6Addr::LOCALHOST], mode).await;
        let server = tokio::spawn(async move {
            let (mut first, peer) = listener.accept().await.unwrap();
            assert_eq!(peer.ip(), IpAddr::V6(Ipv6Addr::LOCALHOST));
            reply(&mut first).await;
            if mode.is_fresh() {
                let (mut second, _) = listener.accept().await.unwrap();
                reply(&mut second).await;
            } else {
                reply(&mut first).await;
            }
            // Account B must open its own pool even though its source is also ::1.
            let (mut other, peer) = listener.accept().await.unwrap();
            assert_eq!(peer.ip(), IpAddr::V6(Ipv6Addr::LOCALHOST));
            reply(&mut other).await;
        });
        let client = CodexBackendClient::new(
            Client::builder().no_proxy().build().unwrap(),
            &base,
            OpenAiConfig::default().wire_profile_state(),
        )
        .with_egress_runtime(runtime);
        let mut request = CodexResponsesRequest::from_body(
            json!({"model":"gpt-5.6-sol","input":"fixture","stream":false})
                .as_object()
                .unwrap()
                .clone(),
        );
        // Excel must still choose HTTP even when downstream originally asked for WS.
        request.use_websocket = true;
        let tools = ClientTools::default();
        request.excel = Some(ExcelPreparedRequest {
            image_policy: None,
            exit_lease: None,
            body: prepare_request(request.body(), &tools, &BTreeMap::new(), None).unwrap(),
            tools,
            structured: None,
            _image_lease: None,
            image_limits: Default::default(),
            completed: Default::default(),
            usage: Default::default(),
            replay: None,
            endpoint: format!("{base}{RESPONSES_PATH}"),
        });
        for account in [&accounts[0], &accounts[0], &accounts[1]] {
            let response = timeout(
                Duration::from_secs(5),
                client
                    .for_account(account)
                    .unwrap()
                    .create_response_stream_with_pool_account(
                        &request,
                        CodexRequestContext::auxiliary(
                            "Bearer fixture",
                            Some("workspace"),
                            "ipv6-reuse",
                            None,
                        ),
                        Some(account.id().as_str()),
                    ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response.transport, CodexBackendTransport::HttpSse);
            let body = timeout(
                Duration::from_secs(5),
                response.body.try_collect::<Vec<_>>(),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(
                String::from_utf8(body.concat())
                    .unwrap()
                    .contains("resp_ipv6_fixture")
            );
        }
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn payload_recovery_pins_source_and_profile_without_advancing_rotation() {
    for mode in [EgressMode::RandomIpv6Reuse, EgressMode::RandomIpv6Fresh] {
        let sources = addresses();
        let (_, runtime, accounts) = fixture(&sources, mode).await;
        let ordinary = managed_proxy_client()
            .with_egress_runtime(runtime.clone())
            .for_account(&accounts[0])
            .unwrap();
        let client = ordinary.clone().with_same_attempt_recovery();
        let request = managed_proxy_request(true);
        let context =
            CodexRequestContext::auxiliary("Bearer synthetic", Some("workspace"), "repair", None);
        let first = client
            .prepare_response_transport_with_pool_account(
                &request,
                context,
                Some(accounts[0].id().as_str()),
            )
            .await
            .unwrap();
        let first = first.attempt_client.unwrap();
        assert_eq!(first.egress_route.as_ref().unwrap().source, sources[0]);
        let profile = first.profile.snapshot().user_agent();
        client
            .profile
            .apply_user_agent_override(
                &gateway_core::provider_ports::ProviderUserAgentOverride::Custom {
                    user_agent: "codex-tui/0.140.0 (Ubuntu 24.04; x86_64) xterm-256color".into(),
                },
            )
            .unwrap();
        assert_ne!(client.profile.snapshot().user_agent(), profile);
        let retry = client
            .clone()
            .prepare_response_transport_with_pool_account(
                &request,
                context,
                Some(accounts[0].id().as_str()),
            )
            .await
            .unwrap();
        let retry = retry.attempt_client.unwrap();
        assert_eq!(retry.egress_route.as_ref().unwrap().source, sources[0]);
        assert_eq!(retry.profile.snapshot().user_agent(), profile);
        let next = ordinary
            .prepare_response_transport_with_pool_account(
                &request,
                context,
                Some(accounts[0].id().as_str()),
            )
            .await
            .unwrap();
        assert_eq!(
            next.attempt_client.unwrap().egress_route.unwrap().source,
            sources[1]
        );
        assert!(
            client
                .for_account(&accounts[1])
                .unwrap()
                .recovery_route
                .is_none()
        );
        assert_eq!(selected(&runtime, &accounts[1]), sources[2]);
    }
}

#[tokio::test]
async fn payload_recovery_refuses_invalidated_route_without_selecting_another() {
    for remove_account in [false, true] {
        let sources = addresses();
        let (store, runtime, accounts) = fixture(&sources, EgressMode::RandomIpv6Fresh).await;
        let client = managed_proxy_client()
            .with_egress_runtime(runtime.clone())
            .for_account(&accounts[0])
            .unwrap()
            .with_same_attempt_recovery();
        let request = managed_proxy_request(true);
        let context =
            CodexRequestContext::auxiliary("Bearer synthetic", Some("workspace"), "repair", None);
        let first = client
            .prepare_response_transport_with_pool_account(
                &request,
                context,
                Some(accounts[0].id().as_str()),
            )
            .await
            .unwrap();
        assert_eq!(
            first.attempt_client.unwrap().egress_route.unwrap().source,
            sources[0]
        );
        {
            let mut state = store.0.lock().unwrap();
            if remove_account {
                state.account_overrides.remove(accounts[0].id());
            } else {
                state.addresses[0].enabled = false;
            }
        }
        runtime.reload().await.unwrap();
        let error = client
            .prepare_response_transport_with_pool_account(
                &request,
                context,
                Some(accounts[0].id().as_str()),
            )
            .await
            .err()
            .expect("invalidated recovery must fail closed");
        assert!(matches!(
            error,
            crate::transport::CodexClientError::Egress(
                CodexEgressError::SourceDisabled | CodexEgressError::ConfigurationChanged
            )
        ));
        assert_eq!(selected(&runtime, &accounts[1]), sources[1]);
    }
}

#[tokio::test]
async fn payload_recovery_preserves_fresh_websocket_pool_identity() {
    let listener = TcpListener::bind("[::1]:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (_, runtime, accounts) = fixture(&[Ipv6Addr::LOCALHOST], EgressMode::RandomIpv6Fresh).await;
    let server = tokio::spawn(async move {
        // Prepared sockets are dropped without sending a payload in this test.
        for _ in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            use tokio_tungstenite::tungstenite::{
                extensions::{ExtensionsConfig, compression::deflate::DeflateConfig},
                protocol::WebSocketConfig,
            };
            let mut extensions = ExtensionsConfig::default();
            extensions.permessage_deflate = Some(DeflateConfig::default());
            let mut config = WebSocketConfig::default();
            config.extensions = extensions;
            let mut socket = tokio_tungstenite::accept_async_with_config(stream, Some(config))
                .await
                .unwrap();
            while let Some(message) = socket.next().await {
                if message.is_err() || message.unwrap().is_close() {
                    break;
                }
            }
        }
    });
    let pool = Arc::new(CodexWebSocketPool::new(Duration::from_secs(60)));
    let client = CodexBackendClient::new(
        Client::builder().no_proxy().build().unwrap(),
        base,
        OpenAiConfig::default().wire_profile_state(),
    )
    .with_egress_runtime(runtime)
    .with_websocket_pool(pool.clone())
    .for_account(&accounts[0])
    .unwrap()
    .with_same_attempt_recovery();
    let request = managed_proxy_request(false);
    let context =
        CodexRequestContext::auxiliary("Bearer synthetic", Some("workspace"), "repair", None);
    let mut keys = Vec::new();
    for _ in 0..2 {
        let prepared = tokio::time::timeout(
            Duration::from_secs(5),
            client.prepare_response_transport_with_pool_account(
                &request,
                context,
                Some(accounts[0].id().as_str()),
            ),
        )
        .await
        .unwrap()
        .unwrap_or_else(|error| panic!("synthetic websocket preparation failed: {error}"));
        keys.push(
            prepared
                .attempt_client
                .as_ref()
                .unwrap()
                .forced_pool_key
                .clone()
                .unwrap(),
        );
        drop(prepared);
    }
    assert_eq!(keys[0], keys[1]);
    pool.shutdown().await;
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

struct Exit {
    proxy: OutboundProxy,
    reports: Mutex<Vec<SessionProxyOutcome>>,
}
impl SessionProxyLease for Exit {
    fn proxy(&self) -> &OutboundProxy {
        &self.proxy
    }
    fn node_id(&self) -> &str {
        "synthetic-node"
    }
    fn report(&self, outcome: SessionProxyOutcome) {
        self.reports.lock().unwrap().push(outcome);
    }
}
fn managed_proxy_account() -> ProviderAccount {
    ProviderAccount::new(
        ProviderAccountId::new("acct_native_exit").unwrap(),
        ProviderKind::new("openai").unwrap(),
        "synthetic".into(),
        Some("workspace".into()),
        "oauth".into(),
        CredentialRevision::new(1).unwrap(),
        None,
    )
}
fn managed_proxy_client() -> CodexBackendClient {
    CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        "http://unresolvable.invalid",
        OpenAiConfig::default().wire_profile_state(),
    )
}
fn managed_proxy_request(http: bool) -> CodexResponsesRequest {
    let mut request = CodexResponsesRequest::from_body(
        json!({"model":"synthetic-model","input":"fixture","stream":true,"store":false})
            .as_object()
            .unwrap()
            .clone(),
    );
    request.force_http_sse = http;
    request.use_websocket = !http;
    request.local_conversation_id = Some("synthetic-session".into());
    request.client_api_key_id = Some("synthetic-key".into());
    request.downstream_websocket_connection_id = (!http).then(|| "synthetic-downstream".into());
    request
}
fn completed(id: &str) -> String {
    json!({"type":"response.completed","response":{"id":id,"status":"completed","output":[]}})
        .to_string()
}

#[tokio::test]
async fn native_http_uses_selected_proxy_and_holds_lease_until_body_drop() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
    let proxy = MockServer::start().await;
    Mock::given(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {}\n\n", completed("resp_native"))),
        )
        .mount(&proxy)
        .await;
    let exit = Arc::new(Exit {
        proxy: OutboundProxy::parse(&proxy.uri()).unwrap(),
        reports: Mutex::new(Vec::new()),
    });
    let weak = Arc::downgrade(&exit);
    let account = managed_proxy_account();
    let client = managed_proxy_client()
        .for_session_proxy(&account, exit)
        .unwrap();
    let response = client
        .create_response_stream_with_pool_account(
            &managed_proxy_request(true),
            CodexRequestContext::auxiliary(
                "Bearer synthetic",
                Some("workspace"),
                "native-http",
                None,
            ),
            Some(account.id().as_str()),
        )
        .await
        .unwrap();
    assert_eq!(response.transport, CodexBackendTransport::HttpSse);
    drop(client);
    assert!(weak.upgrade().is_some());
    let body = response.body.try_collect::<Vec<_>>().await.unwrap();
    assert!(!body.is_empty());
    assert!(weak.upgrade().is_none());
    let requests = proxy.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].headers.contains_key("user-agent"));
    assert_eq!(requests[0].headers["authorization"], "Bearer synthetic");
}

#[tokio::test]
async fn native_ws_tunnels_through_proxy_reuses_socket_and_releases_idle_lease() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy =
        OutboundProxy::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut opening = Vec::new();
        while !opening.ends_with(b"\r\n\r\n") {
            opening.push(stream.read_u8().await.unwrap());
            assert!(opening.len() < 8192);
        }
        assert!(opening.starts_with(b"CONNECT unresolvable.invalid:80 HTTP/1.1\r\n"));
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .unwrap();
        use tokio_tungstenite::tungstenite::{
            extensions::{ExtensionsConfig, compression::deflate::DeflateConfig},
            protocol::WebSocketConfig,
        };
        let mut extensions = ExtensionsConfig::default();
        extensions.permessage_deflate = Some(DeflateConfig::default());
        let mut config = WebSocketConfig::default();
        config.extensions = extensions;
        let mut socket = tokio_tungstenite::accept_async_with_config(stream, Some(config))
            .await
            .unwrap();
        for id in ["resp_first", "resp_second"] {
            let message = socket.next().await.unwrap().unwrap();
            assert!(message.is_text());
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    completed(id).into(),
                ))
                .await
                .unwrap();
        }
        while let Some(message) = socket.next().await {
            if message.is_err() || message.unwrap().is_close() {
                break;
            }
        }
    });
    let pool = Arc::new(CodexWebSocketPool::new(Duration::from_secs(60)));
    let base = managed_proxy_client().with_websocket_pool(pool.clone());
    let account = managed_proxy_account();
    for expected in ["new", "reuse"] {
        let exit = Arc::new(Exit {
            proxy: proxy.clone(),
            reports: Mutex::new(Vec::new()),
        });
        let weak = Arc::downgrade(&exit);
        let client = base.for_session_proxy(&account, exit).unwrap();
        let mut request = managed_proxy_request(false);
        if expected == "reuse" {
            request.set_previous_response_id(Some("resp_first".into()));
            request.previous_response_scope =
                Some(crate::transport::protocol::responses::PreviousResponseScope::ConnectionLocal);
        }
        let response = client
            .create_response_stream_with_pool_account(
                &request,
                CodexRequestContext::auxiliary(
                    "Bearer synthetic",
                    Some("workspace"),
                    "native-ws",
                    None,
                ),
                Some(account.id().as_str()),
            )
            .await
            .unwrap();
        assert_eq!(response.transport, CodexBackendTransport::WebSocket);
        assert_eq!(response.websocket_pool_decision.unwrap().kind(), expected);
        drop(client);
        response.body.try_collect::<Vec<_>>().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("idle websocket must not retain an active exit lease");
    }
    pool.shutdown().await;
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn local_ws_failures_do_not_penalize_the_proxy() {
    let exit = Arc::new(Exit {
        proxy: OutboundProxy::parse("http://127.0.0.1:18000").unwrap(),
        reports: Mutex::new(Vec::new()),
    });
    let guard = SessionProxyHold(exit.clone());
    guard.report_websocket_error(&CodexWebSocketExchangeError::OriginCircuitOpen);
    guard.report_websocket_error(&CodexWebSocketExchangeError::SharedConnectFailed);
    assert!(exit.reports.lock().unwrap().is_empty());
    guard.report_websocket_error(&CodexWebSocketExchangeError::ConnectTimeout {
        timeout: Duration::from_secs(1),
    });
    assert_eq!(
        *exit.reports.lock().unwrap(),
        vec![SessionProxyOutcome::NetworkFailure]
    );
}
