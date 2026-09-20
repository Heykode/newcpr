use super::*;

use std::{collections::BTreeMap, net::Ipv6Addr};

use futures::future::BoxFuture;
use gateway_core::{
    account::{CredentialRevision, ProviderAccount, ProviderAccountId},
    provider_ports::{
        ProviderStoreError,
        egress::{
            EgressMode, ProviderEgressAddress, ProviderEgressConfig, ProviderEgressStorePort,
        },
    },
    routing::ProviderKind,
};
use provider_openai::transport::egress::{CodexEgressError, CodexEgressRuntime};

struct MutableEgressStore(Mutex<ProviderEgressConfig>);

impl ProviderEgressStorePort for MutableEgressStore {
    fn load(&self) -> BoxFuture<'_, Result<Arc<ProviderEgressConfig>, ProviderStoreError>> {
        Box::pin(async { Ok(Arc::new(self.0.lock().unwrap().clone())) })
    }

    fn ensure_fixed_affinity(
        &self,
        _: &ProviderAccountId,
    ) -> BoxFuture<'_, Result<Option<Ipv6Addr>, ProviderStoreError>> {
        Box::pin(async { panic!("transport must not allocate affinity") })
    }
}

async fn fixture(
    base_url: &str,
    mode: EgressMode,
) -> (
    CodexBackendClient,
    Arc<MutableEgressStore>,
    Arc<CodexEgressRuntime>,
    Arc<CodexWebSocketPool>,
) {
    let account = ProviderAccount::new(
        ProviderAccountId::new("acct_review").unwrap(),
        ProviderKind::new("openai").unwrap(),
        "review".to_owned(),
        Some("review".to_owned()),
        "oauth".to_owned(),
        CredentialRevision::new(1).unwrap(),
        None,
    );
    let store = Arc::new(MutableEgressStore(Mutex::new(ProviderEgressConfig {
        revision: 1,
        default_mode: mode,
        addresses: vec![ProviderEgressAddress {
            id: "loopback".to_owned(),
            address: Ipv6Addr::LOCALHOST,
            enabled: true,
        }],
        account_overrides: BTreeMap::from([(account.id().clone(), None)]),
        fixed_bindings: BTreeMap::from([(account.id().clone(), Ipv6Addr::LOCALHOST)]),
    })));
    let runtime = CodexEgressRuntime::load(store.clone()).await.unwrap();
    let pool = Arc::new(CodexWebSocketPool::new(Duration::from_mins(1)));
    let backend = CodexBackendClient::new(
        provider_openai::transport::build_reqwest_client().unwrap(),
        base_url,
        test_wire_profile(),
    )
    .with_egress_runtime(runtime.clone())
    .with_websocket_pool(pool.clone())
    .for_account(&account)
    .unwrap();
    (backend, store, runtime, pool)
}

#[tokio::test]
async fn default_ipv4_and_explicit_ipv6_switch_real_http_and_ws_connections() {
    for websocket in [false, true] {
        let ipv4 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = ipv4.local_addr().unwrap().port();
        let ipv6 = TcpListener::bind((Ipv6Addr::LOCALHOST, port))
            .await
            .unwrap();
        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut servers = Vec::new();
        for listener in [ipv4, ipv6] {
            let seen_tx = seen_tx.clone();
            servers.push(tokio::spawn(async move {
                let mut connections = tokio::task::JoinSet::new();
                loop {
                    let (mut stream, peer) = listener.accept().await.unwrap();
                    let seen_tx = seen_tx.clone();
                    connections.spawn(async move {
                        if websocket {
                            let mut socket = accept_codex_test_websocket(stream).await;
                            while let Some(Ok(Message::Text(_))) = socket.next().await {
                                seen_tx.send(peer.ip()).unwrap();
                                socket
                                    .send(Message::Text(
                                        completed_websocket_response("resp_mode", 1, 1).into(),
                                    ))
                                    .await
                                    .unwrap();
                            }
                        } else {
                            read_http_request(&mut stream).await;
                            seen_tx.send(peer.ip()).unwrap();
                            let body = format!(
                                "data: {}\n\n",
                                completed_websocket_response("resp_mode", 1, 1)
                            );
                            stream
                                .write_all(
                                    format!(
                                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                        body.len()
                                    )
                                    .as_bytes(),
                                )
                                .await
                                .unwrap();
                        }
                    });
                }
            }));
        }
        let (backend, store, runtime, pool) =
            fixture(&format!("http://localhost:{port}"), EgressMode::Unchanged).await;
        let mut request = codex_request("gpt-test", "fixture", Vec::new());
        request.use_websocket = websocket;
        request.force_http_sse = !websocket;
        request.local_conversation_id = Some("mode-switch".to_owned());
        for mode in [
            EgressMode::Unchanged,
            EgressMode::FixedIpv6Reuse,
            EgressMode::Unchanged,
        ] {
            {
                let mut state = store.0.lock().unwrap();
                state.default_mode = mode;
                state.revision += 1;
            }
            runtime.reload().await.unwrap();
            let response = timeout(
                Duration::from_secs(5),
                backend.create_response(&request, request_context("mode-switch", Some("review"))),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                response.transport,
                if websocket {
                    CodexBackendTransport::WebSocket
                } else {
                    CodexBackendTransport::HttpSse
                }
            );
            let peer = timeout(Duration::from_secs(1), seen_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(peer.is_ipv4(), mode == EgressMode::Unchanged);
        }
        pool.shutdown().await;
        for server in servers {
            server.abort();
            let _ = server.await;
        }
    }
}

#[tokio::test]
async fn default_websocket_egress_rejects_implicit_ipv6() {
    let listener = TcpListener::bind("[::1]:0").await.unwrap();
    let (backend, _, _, pool) = fixture(
        &format!("http://{}", listener.local_addr().unwrap()),
        EgressMode::Unchanged,
    )
    .await;
    let mut request = codex_request("gpt-test", "fixture", Vec::new());
    request.use_websocket = true;
    request.set_previous_response_id(Some("resp_ipv4_only".to_owned()));
    request.previous_response_scope = Some(PreviousResponseScope::Persisted);
    assert!(
        timeout(
            Duration::from_secs(5),
            backend.create_response(&request, request_context("ipv4-only", Some("review")))
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(
        timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    pool.shutdown().await;
}

#[tokio::test]
async fn local_source_failures_do_not_open_the_websocket_origin_breaker() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (backend, store, runtime, pool) = fixture(&base_url, EgressMode::FixedIpv6Reuse).await;
    let mut request = codex_request("gpt-test", "fixture", Vec::new());
    request.local_conversation_id = Some("local-failure".to_owned());
    for _ in 0..4 {
        let error = backend
            .create_response(&request, request_context("local-failure", Some("review")))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            CodexClientError::Egress(CodexEgressError::DestinationUnavailable)
        ));
    }
    {
        let mut state = store.0.lock().unwrap();
        state.default_mode = EgressMode::Unchanged;
        state.revision += 1;
    }
    runtime.reload().await.unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut websocket = accept_codex_test_websocket(stream).await;
        assert!(matches!(websocket.next().await, Some(Ok(Message::Text(_)))));
        websocket
            .send(Message::Text(
                completed_websocket_response("resp_recovered", 1, 1).into(),
            ))
            .await
            .unwrap();
    });
    let response = timeout(
        Duration::from_secs(5),
        backend.create_response(&request, request_context("recovered", Some("review"))),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.transport, CodexBackendTransport::WebSocket);
    server.await.unwrap();
    pool.shutdown().await;
}

#[tokio::test]
async fn deleted_unchanged_account_cannot_dispatch_or_reuse_its_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (backend, store, runtime, pool) = fixture(&base_url, EgressMode::Unchanged).await;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut websocket = accept_codex_test_websocket(stream).await;
        assert!(matches!(websocket.next().await, Some(Ok(Message::Text(_)))));
        websocket
            .send(Message::Text(
                completed_websocket_response("resp_before_delete", 1, 1).into(),
            ))
            .await
            .unwrap();
        while let Some(Ok(message)) = websocket.next().await {
            assert!(!matches!(message, Message::Text(_)));
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    });
    let mut request = codex_request("gpt-test", "fixture", Vec::new());
    request.local_conversation_id = Some("deleted-account".to_owned());
    request.use_websocket = true;
    backend
        .create_response(&request, request_context("before-delete", Some("review")))
        .await
        .unwrap();
    {
        let mut state = store.0.lock().unwrap();
        state.account_overrides.clear();
        state.fixed_bindings.clear();
        state.revision += 1;
    }
    runtime.reload().await.unwrap();
    request.set_previous_response_id(Some("resp_before_delete".to_owned()));
    request.previous_response_scope = Some(PreviousResponseScope::ConnectionLocal);
    assert!(matches!(
        backend
            .create_response(&request, request_context("after-delete", Some("review")))
            .await,
        Err(CodexClientError::Egress(
            CodexEgressError::ConfigurationChanged
        ))
    ));
    request.set_previous_response_id(None);
    request.previous_response_scope = None;
    request.force_http_sse = true;
    assert!(matches!(
        backend
            .create_response(
                &request,
                request_context("after-delete-http", Some("review"))
            )
            .await,
        Err(CodexClientError::Egress(
            CodexEgressError::ConfigurationChanged
        ))
    ));
    pool.shutdown().await;
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn exact_ipv6_continuation_keeps_its_owner_after_downstream_reconnect() {
    let listener = TcpListener::bind("[::1]:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (backend, store, runtime, pool) = fixture(&base_url, EgressMode::FixedIpv6Fresh).await;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut websocket = accept_codex_test_websocket(stream).await;
        for response_id in ["resp_before_reconnect", "resp_after_reconnect"] {
            assert!(matches!(websocket.next().await, Some(Ok(Message::Text(_)))));
            websocket
                .send(Message::Text(
                    completed_websocket_response(response_id, 1, 1).into(),
                ))
                .await
                .unwrap();
        }
    });
    let mut request = codex_request("gpt-test", "fixture", Vec::new());
    request.use_websocket = true;
    request.local_conversation_id = Some("reconnected-client".to_owned());
    request.downstream_websocket_connection_id = Some("downstream-before".to_owned());
    let first = backend
        .create_response(
            &request,
            request_context("before-reconnect", Some("review")),
        )
        .await
        .unwrap();
    {
        let mut state = store.0.lock().unwrap();
        state.default_mode = EgressMode::RandomIpv6Fresh;
        state.revision += 1;
    }
    runtime.reload().await.unwrap();
    request.set_previous_response_id(Some("resp_before_reconnect".to_owned()));
    request.previous_response_scope = Some(PreviousResponseScope::ConnectionLocal);
    request.downstream_websocket_connection_id = Some("downstream-after".to_owned());
    let second = timeout(
        Duration::from_secs(5),
        backend.create_response(&request, request_context("after-reconnect", Some("review"))),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!first.websocket_pool_decision.unwrap().is_reuse());
    assert!(second.websocket_pool_decision.unwrap().is_reuse());
    server.await.unwrap();
    pool.shutdown().await;
}

#[tokio::test]
async fn published_source_disable_retires_idle_socket_without_ipv4_fallback() {
    let listener = TcpListener::bind("[::1]:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (backend, store, runtime, pool) = fixture(&base_url, EgressMode::FixedIpv6Reuse).await;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut websocket = accept_codex_test_websocket(stream).await;
        assert!(matches!(websocket.next().await, Some(Ok(Message::Text(_)))));
        websocket
            .send(Message::Text(
                completed_websocket_response("resp_before_disable", 1, 1).into(),
            ))
            .await
            .unwrap();
        while let Some(Ok(message)) = websocket.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    });
    let mut request = codex_request("gpt-test", "fixture", Vec::new());
    request.use_websocket = true;
    request.local_conversation_id = Some("source-retirement".to_owned());
    backend
        .create_response(&request, request_context("before-disable", Some("review")))
        .await
        .unwrap();
    {
        let mut state = store.0.lock().unwrap();
        state.addresses[0].enabled = false;
        state.revision += 1;
    }
    runtime.reload().await.unwrap();
    pool.retire_unavailable_egress(&runtime).await.unwrap();
    request.set_previous_response_id(Some("resp_before_disable".to_owned()));
    request.previous_response_scope = Some(PreviousResponseScope::ConnectionLocal);
    assert!(matches!(
        backend
            .create_response(&request, request_context("after-disable", Some("review")))
            .await,
        Err(CodexClientError::Egress(CodexEgressError::SourceDisabled))
    ));
    pool.shutdown().await;
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}
