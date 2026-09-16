use super::*;
use std::{collections::BTreeMap, net::Ipv6Addr};

use futures::future::BoxFuture;
use gateway_core::{
    account::{CredentialRevision, ProviderAccount, ProviderAccountId},
    provider_ports::{
        ProviderStoreError, ProviderStoreErrorKind,
        egress::{
            EgressMode, ProviderEgressAddress, ProviderEgressConfig, ProviderEgressStorePort,
        },
    },
    routing::ProviderKind,
};
use provider_openai::transport::egress::{CodexEgressError, CodexEgressRuntime};

struct EgressStore {
    config: Mutex<ProviderEgressConfig>,
    fail: std::sync::atomic::AtomicBool,
}

impl ProviderEgressStorePort for EgressStore {
    fn load(&self) -> BoxFuture<'_, Result<Arc<ProviderEgressConfig>, ProviderStoreError>> {
        Box::pin(async {
            if self.fail.load(Ordering::SeqCst) {
                return Err(ProviderStoreError::new(
                    ProviderStoreErrorKind::Unavailable,
                    "isolated test failure",
                ));
            }
            Ok(Arc::new(self.config.lock().unwrap().clone()))
        })
    }

    fn ensure_fixed_affinity(
        &self,
        _account_id: &ProviderAccountId,
    ) -> BoxFuture<'_, Result<Option<Ipv6Addr>, ProviderStoreError>> {
        Box::pin(async { panic!("request routing must not allocate or write affinity") })
    }
}

fn account(id: &str) -> ProviderAccount {
    ProviderAccount::new(
        ProviderAccountId::new(format!("acct_{id}")).unwrap(),
        ProviderKind::new("openai").unwrap(),
        id.to_owned(),
        Some(id.to_owned()),
        "oauth".to_owned(),
        CredentialRevision::new(1).unwrap(),
        None,
    )
}

fn config(mode: EgressMode, accounts: &[ProviderAccount]) -> ProviderEgressConfig {
    ProviderEgressConfig {
        revision: 1,
        default_mode: mode,
        addresses: vec![ProviderEgressAddress {
            id: "loopback".to_owned(),
            address: Ipv6Addr::LOCALHOST,
            enabled: true,
        }],
        account_overrides: accounts.iter().map(|a| (a.id().clone(), None)).collect(),
        fixed_bindings: accounts
            .iter()
            .map(|a| (a.id().clone(), Ipv6Addr::LOCALHOST))
            .collect(),
    }
}

async fn runtime(config: ProviderEgressConfig) -> (Arc<EgressStore>, Arc<CodexEgressRuntime>) {
    let store = Arc::new(EgressStore {
        config: Mutex::new(config),
        fail: std::sync::atomic::AtomicBool::new(false),
    });
    let runtime = CodexEgressRuntime::load(store.clone()).await.unwrap();
    (store, runtime)
}

struct ReloadStep {
    result: Result<Arc<ProviderEgressConfig>, ProviderStoreError>,
    release: Option<tokio::sync::oneshot::Receiver<()>>,
}

struct OrderedReloadStore(Mutex<std::collections::VecDeque<ReloadStep>>);

impl ProviderEgressStorePort for OrderedReloadStore {
    fn load(&self) -> BoxFuture<'_, Result<Arc<ProviderEgressConfig>, ProviderStoreError>> {
        Box::pin(async {
            let step = self.0.lock().unwrap().pop_front().unwrap();
            if let Some(release) = step.release {
                release.await.unwrap();
            }
            step.result
        })
    }

    fn ensure_fixed_affinity(
        &self,
        _: &ProviderAccountId,
    ) -> BoxFuture<'_, Result<Option<Ipv6Addr>, ProviderStoreError>> {
        Box::pin(async { panic!("reload must not allocate affinity") })
    }
}

async fn assert_reload_publication_is_ordered(first_fails: bool) {
    let account = account("ordered-reload");
    let initial = Arc::new(config(
        EgressMode::Unchanged,
        std::slice::from_ref(&account),
    ));
    let mut removed_account = (*initial).clone();
    removed_account.revision += 1;
    removed_account.account_overrides.clear();
    removed_account.fixed_bindings.clear();
    let failure = || {
        Err(ProviderStoreError::new(
            ProviderStoreErrorKind::Unavailable,
            "controlled reload failure",
        ))
    };
    let (release, blocked) = tokio::sync::oneshot::channel();
    let store = Arc::new(OrderedReloadStore(Mutex::new(
        [
            ReloadStep {
                result: Ok(initial.clone()),
                release: None,
            },
            ReloadStep {
                result: if first_fails { failure() } else { Ok(initial) },
                release: Some(blocked),
            },
            ReloadStep {
                result: if first_fails {
                    Ok(Arc::new(removed_account))
                } else {
                    failure()
                },
                release: None,
            },
        ]
        .into(),
    )));
    let runtime = CodexEgressRuntime::load(store.clone()).await.unwrap();
    let first = runtime.reload();
    let second = runtime.reload();
    tokio::pin!(first, second);
    assert!(futures::poll!(&mut first).is_pending());
    assert!(futures::poll!(&mut second).is_pending());
    assert_eq!(
        store.0.lock().unwrap().len(),
        1,
        "the second store read must wait for the first publication"
    );
    release.send(()).unwrap();
    assert_eq!(first.await.is_err(), first_fails);
    assert_eq!(second.await.is_err(), !first_fails);
    let backend = client("http://127.0.0.1:1", runtime.clone())
        .for_account(&account)
        .unwrap();
    let mut request = codex_request("gpt-test", "fixture", Vec::new());
    request.force_http_sse = true;
    let error = backend
        .create_response(&request, request_context("ordered-reload", Some("reload")))
        .await
        .expect_err("the old account route must remain blocked before dispatch");
    assert!(matches!(
        (first_fails, error),
        (
            true,
            CodexClientError::Egress(CodexEgressError::ConfigurationChanged)
        ) | (
            false,
            CodexClientError::Egress(CodexEgressError::Unavailable)
        )
    ));
}

#[tokio::test]
async fn delayed_reload_cannot_restore_routes_after_a_newer_reload_failure() {
    timeout(
        Duration::from_secs(5),
        assert_reload_publication_is_ordered(false),
    )
    .await
    .expect("reload ordering must complete");
}

#[tokio::test]
async fn delayed_reload_failure_cannot_erase_a_newer_successful_reload() {
    timeout(
        Duration::from_secs(5),
        assert_reload_publication_is_ordered(true),
    )
    .await
    .expect("reload ordering must complete");
}

fn client(url: &str, runtime: Arc<CodexEgressRuntime>) -> CodexBackendClient {
    CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        url,
        test_wire_profile(),
    )
    .with_egress_runtime(runtime)
}

async fn http_reply(stream: &mut TcpStream, id: &str, close: bool) {
    let event = completed_websocket_response(id, 1, 1);
    let body = format!("event: response.completed\ndata: {event}\n\n");
    stream.write_all(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n{body}",
        body.len(), if close { "close" } else { "keep-alive" },
    ).as_bytes()).await.unwrap();
}

#[tokio::test]
async fn ipv6_http_reuse_and_fresh_modes_bind_real_source_and_isolate_accounts() {
    for mode in [
        EgressMode::FixedIpv6Reuse,
        EgressMode::RandomIpv6Reuse,
        EgressMode::FixedIpv6Fresh,
        EgressMode::RandomIpv6Fresh,
    ] {
        let listener = TcpListener::bind("[::1]:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let a = account("a");
        let b = account("b");
        let (_, runtime) = runtime(config(mode, &[a.clone(), b.clone()])).await;
        let base = client(&url, runtime);
        let server = tokio::spawn(async move {
            let (mut first, peer) = listener.accept().await.unwrap();
            assert_eq!(peer.ip(), std::net::IpAddr::V6(Ipv6Addr::LOCALHOST));
            let head = read_http_request_with_body(&mut first).await;
            assert!(head.starts_with(b"POST /codex/responses HTTP/1.1"));
            http_reply(&mut first, "resp_first", false).await;
            if mode.is_fresh() {
                let (mut next, peer) = listener.accept().await.unwrap();
                assert_eq!(peer.ip(), std::net::IpAddr::V6(Ipv6Addr::LOCALHOST));
                assert!(!read_http_request_with_body(&mut next).await.is_empty());
                http_reply(&mut next, "resp_second", true).await;
            } else {
                assert!(!read_http_request_with_body(&mut first).await.is_empty());
                http_reply(&mut first, "resp_second", false).await;
            }
            // Sharing an exit does not mean sharing an account's connection pool.
            let (mut other, _) = listener.accept().await.unwrap();
            assert!(!read_http_request_with_body(&mut other).await.is_empty());
            http_reply(&mut other, "resp_other", true).await;
        });
        let mut request = codex_request("gpt-test", "fixture", Vec::new());
        request.force_http_sse = true;
        for (account, expected) in [(&a, "resp_first"), (&a, "resp_second"), (&b, "resp_other")] {
            let response = timeout(
                Duration::from_secs(5),
                base.for_account(account).unwrap().create_response(
                    &request,
                    request_context("egress-http", Some(account.id().as_str())),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(response.body.contains(expected));
        }
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn ipv6_ws_modes_reuse_or_open_fresh_but_exact_continuation_keeps_its_socket() {
    for mode in [
        EgressMode::FixedIpv6Reuse,
        EgressMode::RandomIpv6Reuse,
        EgressMode::FixedIpv6Fresh,
        EgressMode::RandomIpv6Fresh,
    ] {
        let listener = TcpListener::bind("[::1]:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let account = account("ws");
        let (store, runtime) = runtime(config(mode, std::slice::from_ref(&account))).await;
        let pool = Arc::new(CodexWebSocketPool::new(Duration::from_mins(1)));
        let base = client(&url, runtime.clone()).with_websocket_pool(pool.clone());
        let backend = base.for_account(&account).unwrap();
        let server = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            assert_eq!(peer.ip(), std::net::IpAddr::V6(Ipv6Addr::LOCALHOST));
            let mut ws = accept_codex_test_websocket(stream).await;
            assert!(matches!(ws.next().await, Some(Ok(Message::Text(_)))));
            ws.send(Message::Text(
                completed_websocket_response("resp_first", 1, 1).into(),
            ))
            .await
            .unwrap();
            let mut second = if mode.is_fresh() {
                let (stream, _) = listener.accept().await.unwrap();
                accept_codex_test_websocket(stream).await
            } else {
                ws
            };
            assert!(matches!(second.next().await, Some(Ok(Message::Text(_)))));
            second
                .send(Message::Text(
                    completed_websocket_response("resp_second", 1, 1).into(),
                ))
                .await
                .unwrap();
            let Some(Ok(Message::Text(payload))) = second.next().await else {
                panic!("exact continuation must arrive on the second socket");
            };
            let payload: Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(payload["previous_response_id"], "resp_second");
            second
                .send(Message::Text(
                    completed_websocket_response("resp_third", 1, 1).into(),
                ))
                .await
                .unwrap();
            // Keep the connection alive until the client returns it to the pool.
            while let Some(Ok(message)) = second.next().await {
                if matches!(message, Message::Close(_)) {
                    break;
                }
            }
        });
        let mut request = codex_request("gpt-test", "fixture", Vec::new());
        request.use_websocket = true;
        request.local_conversation_id = Some("egress-conversation".to_owned());
        for expected in ["resp_first", "resp_second"] {
            let response = timeout(
                Duration::from_secs(5),
                backend.create_response(
                    &request,
                    request_context("egress-ws", Some(account.id().as_str())),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response.transport, CodexBackendTransport::WebSocket);
            assert!(response.body.contains(expected));
        }
        {
            let mut config = store.config.lock().unwrap();
            config.default_mode = EgressMode::RandomIpv6Fresh;
            config.revision += 1;
        }
        runtime.reload().await.unwrap();
        request.set_previous_response_id(Some("resp_second".to_owned()));
        request.previous_response_scope = Some(PreviousResponseScope::ConnectionLocal);
        let response = timeout(
            Duration::from_secs(5),
            backend.create_response(
                &request,
                request_context("egress-continuation", Some(account.id().as_str())),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(response.body.contains("resp_third"));
        assert!(response.websocket_pool_decision.unwrap().is_reuse());
        pool.shutdown().await;
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn unavailable_ipv6_fails_without_silent_ipv4_or_proxy_fallback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let account = account("blocked");
    let mut state = config(EgressMode::FixedIpv6Reuse, std::slice::from_ref(&account));
    let unavailable: Ipv6Addr = "2001:db8:ffff::1".parse().unwrap();
    state.addresses[0].address = unavailable;
    state
        .fixed_bindings
        .insert(account.id().clone(), unavailable);
    let (_, runtime) = runtime(state).await;
    let backend = client(&url, runtime).for_account(&account).unwrap();
    let mut request = codex_request("gpt-test", "fixture", Vec::new());
    request.force_http_sse = true;
    assert!(matches!(
        backend
            .create_response(&request, request_context("missing-source", Some("a")))
            .await,
        Err(CodexClientError::Egress(
            CodexEgressError::SourceUnavailable
        ))
    ));
    assert!(
        timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn failed_runtime_reload_blocks_stale_routes_and_recovers_after_reload() {
    let account = account("reload");
    let (store, runtime) = runtime(config(
        EgressMode::FixedIpv6Reuse,
        std::slice::from_ref(&account),
    ))
    .await;
    let backend = client("http://[::1]:9", runtime.clone())
        .for_account(&account)
        .unwrap();
    store.fail.store(true, Ordering::SeqCst);
    assert!(runtime.reload().await.is_err());
    let mut request = codex_request("gpt-test", "fixture", Vec::new());
    request.force_http_sse = true;
    assert!(matches!(
        backend
            .create_response(&request, request_context("reload", Some("a")))
            .await,
        Err(CodexClientError::Egress(CodexEgressError::Unavailable))
    ));
    store.fail.store(false, Ordering::SeqCst);
    runtime.reload().await.unwrap();
}

#[tokio::test]
async fn unchanged_mode_needs_no_ipv6_pool_and_keeps_original_ipv4_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let account = account("unchanged");
    let (_, runtime) = runtime(ProviderEgressConfig {
        account_overrides: BTreeMap::from([(account.id().clone(), None)]),
        ..ProviderEgressConfig::default()
    })
    .await;
    let server = tokio::spawn(async move {
        let (mut stream, peer) = listener.accept().await.unwrap();
        assert!(peer.is_ipv4());
        assert!(!read_http_request_with_body(&mut stream).await.is_empty());
        http_reply(&mut stream, "resp_unchanged", true).await;
    });
    let mut request = codex_request("gpt-test", "fixture", Vec::new());
    request.force_http_sse = true;
    let response = client(&url, runtime)
        .for_account(&account)
        .unwrap()
        .create_response(&request, request_context("unchanged", Some("a")))
        .await
        .unwrap();
    assert!(response.body.contains("resp_unchanged"));
    server.await.unwrap();
}

#[tokio::test]
async fn ipv6_http_cannot_connect_to_an_ipv4_only_destination() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let account = account("v6-only");
    let (_, runtime) = runtime(config(
        EgressMode::FixedIpv6Reuse,
        std::slice::from_ref(&account),
    ))
    .await;
    let mut request = codex_request("gpt-test", "fixture", Vec::new());
    request.force_http_sse = true;
    let response = timeout(
        Duration::from_secs(3),
        client(&url, runtime)
            .for_account(&account)
            .unwrap()
            .create_response(&request, request_context("v6-only", Some("a"))),
    )
    .await
    .unwrap();
    assert!(response.is_err());
    assert!(
        timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn ipv6_ws_safe_http_fallback_preserves_source_and_frozen_user_agent() {
    let listener = TcpListener::bind("[::1]:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let account = account("fallback");
    let (_, runtime) = runtime(config(
        EgressMode::RandomIpv6Fresh,
        std::slice::from_ref(&account),
    ))
    .await;
    let profile = test_wire_profile();
    let original_ua = profile.snapshot().user_agent();
    let (opened, opening) = tokio::sync::oneshot::channel();
    let (updated, update) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut ws, ws_peer) = listener.accept().await.unwrap();
        let ws_head = read_http_request(&mut ws).await;
        assert!(ws_head.starts_with("GET /codex/responses HTTP/1.1"));
        opened.send(()).unwrap();
        update.await.unwrap();
        // Do not complete the opening. The bounded pre-payload fallback is safe.
        let (mut http, http_peer) = listener.accept().await.unwrap();
        let http_raw = read_http_request_with_body(&mut http).await;
        let http_head = String::from_utf8_lossy(&http_raw);
        assert!(http_head.starts_with("POST /codex/responses HTTP/1.1"));
        assert_eq!(ws_peer.ip(), http_peer.ip());
        assert_eq!(ws_peer.ip(), std::net::IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(
            read_header_value(&ws_head, "user-agent"),
            Some(original_ua.as_str())
        );
        assert_eq!(
            read_header_value(&http_head, "user-agent"),
            Some(original_ua.as_str())
        );
        assert_eq!(
            read_header_value(&ws_head, "chatgpt-account-id"),
            read_header_value(&http_head, "chatgpt-account-id")
        );
        http_reply(&mut http, "resp_fallback", true).await;
    });
    let pool = Arc::new(CodexWebSocketPool::new(Duration::from_mins(1)));
    let backend = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        &url,
        profile.clone(),
    )
    .with_egress_runtime(runtime)
    .with_websocket_pool(pool.clone())
    .for_account(&account)
    .unwrap();
    let response = tokio::spawn(async move {
        let mut request = codex_request("gpt-test", "fixture", Vec::new());
        request.local_conversation_id = Some("fallback-session".to_owned());
        backend
            .create_response(&request, request_context("fallback", Some("a")))
            .await
    });
    timeout(Duration::from_secs(3), opening)
        .await
        .unwrap()
        .unwrap();
    profile.update_bundled_release(&CodexBundledReleaseProfile {
        codex_version: "9.9.9".to_owned(),
        desktop_version: "9.9.9".to_owned(),
        desktop_build: "999".to_owned(),
        verified_at: Utc::now(),
    });
    updated.send(()).unwrap();
    let response = timeout(Duration::from_secs(5), response)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.transport, CodexBackendTransport::HttpSse);
    assert!(response.body.contains("resp_fallback"));
    server.await.unwrap();
    pool.shutdown().await;
}

#[tokio::test]
async fn ipv6_and_proxy_conflict_is_local_and_does_not_make_a_network_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let account = account("proxy").with_outbound_proxy(Some(
        gateway_core::account::OutboundProxy::parse(&url).unwrap(),
    ));
    let (_, runtime) = runtime(config(
        EgressMode::RandomIpv6Reuse,
        std::slice::from_ref(&account),
    ))
    .await;
    let mut request = codex_request("gpt-test", "fixture", Vec::new());
    request.force_http_sse = true;
    let response = client(&url, runtime)
        .for_account(&account)
        .unwrap()
        .create_response(&request, request_context("conflict", Some("a")))
        .await;
    assert!(matches!(
        response,
        Err(CodexClientError::Egress(CodexEgressError::ProxyConflict))
    ));
    assert!(
        timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}
