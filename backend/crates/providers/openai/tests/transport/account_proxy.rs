use super::*;
use gateway_core::{
    account::{CredentialRevision, OutboundProxy, ProviderAccount, ProviderAccountId},
    routing::ProviderKind,
};
use std::{
    io::{self, Cursor, Read},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, BufWriter, ReadBuf};

fn account(id: &str, proxy: Option<&str>) -> ProviderAccount {
    ProviderAccount::new(
        ProviderAccountId::new(format!("acct_{id}")).unwrap(),
        ProviderKind::new("openai").unwrap(),
        id.to_owned(),
        Some(id.to_owned()),
        "oauth".to_owned(),
        CredentialRevision::new(1).unwrap(),
        None,
    )
    .with_outbound_proxy(proxy.map(|value| OutboundProxy::parse(value).unwrap()))
}

async fn http_exit(label: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_http_request_with_body(&mut stream).await;
        let body = format!(
            "event: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp_{label}\",\"status\":\"completed\",\"output\":[],\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}}}\n\n"
        );
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        let header_end = request
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap();
        String::from_utf8(request[..header_end].to_vec()).unwrap()
    });
    (format!("http://{address}"), task)
}

async fn pooled_http_exit(
    expected_connections: usize,
    expected_requests: usize,
) -> (String, tokio::task::JoinHandle<Vec<usize>>) {
    use std::convert::Infallible;

    use http_body_util::Full;
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let accept = tokio::spawn(async move {
            let mut tasks = Vec::new();
            for connection_id in 0..expected_connections {
                let (stream, _) = listener.accept().await.unwrap();
                let seen_tx = seen_tx.clone();
                tasks.push(tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |_request| {
                        let seen_tx = seen_tx.clone();
                        async move {
                            seen_tx.send(connection_id).unwrap();
                            let body = concat!(
                                "event: response.completed\n",
                                "data: {\"type\":\"response.completed\",\"response\":{",
                                "\"id\":\"resp_pool\",\"status\":\"completed\",\"output\":[],",
                                "\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
                            );
                            Ok::<_, Infallible>(
                                hyper::Response::builder()
                                    .header("content-type", "text/event-stream")
                                    .body(Full::new(bytes::Bytes::from_static(body.as_bytes())))
                                    .unwrap(),
                            )
                        }
                    });
                    http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                        .unwrap();
                }));
            }
            tasks
        });
        let mut connection_ids = Vec::with_capacity(expected_requests);
        for _ in 0..expected_requests {
            connection_ids.push(seen_rx.recv().await.unwrap());
        }
        for task in accept.await.unwrap() {
            task.abort();
        }
        connection_ids
    });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn direct_http_accounts_use_distinct_connection_pools() {
    let (base_url, server) = pooled_http_exit(2, 2).await;
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        base_url,
        test_wire_profile(),
    );
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    for (id, request_id) in [("direct-a", "direct-a"), ("direct-b", "direct-b")] {
        base.for_account(&account(id, None))
            .unwrap()
            .create_response(&request, request_context(request_id, Some(id)))
            .await
            .unwrap();
    }

    let connection_ids = timeout(Duration::from_secs(5), server)
        .await
        .expect("two account-scoped connections")
        .unwrap();
    assert_ne!(connection_ids[0], connection_ids[1]);
}

#[tokio::test]
async fn direct_http_pool_reuses_one_profile_and_rotates_after_user_agent_change() {
    use gateway_core::provider_ports::ProviderUserAgentOverride;

    let (base_url, server) = pooled_http_exit(2, 3).await;
    let account = account("profile-pool", None);
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        &base_url,
        test_wire_profile(),
    );
    let default_client = base.for_account(&account).unwrap();
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    for request_id in ["profile-default-a", "profile-default-b"] {
        default_client
            .create_response(&request, request_context(request_id, Some("profile-pool")))
            .await
            .unwrap();
    }

    let custom_profile = test_wire_profile();
    custom_profile
        .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
            user_agent: "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color".to_owned(),
        })
        .unwrap();
    CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        base_url,
        custom_profile,
    )
    .for_account(&account)
    .unwrap()
    .create_response(
        &request,
        request_context("profile-custom", Some("profile-pool")),
    )
    .await
    .unwrap();

    let connection_ids = timeout(Duration::from_secs(5), server)
        .await
        .expect("default and custom profile connections")
        .unwrap();
    assert_eq!(connection_ids[0], connection_ids[1]);
    assert_ne!(connection_ids[0], connection_ids[2]);
}

#[tokio::test]
async fn account_http_pool_eviction_forces_a_fresh_connection() {
    let (base_url, server) = pooled_http_exit(2, 2).await;
    let account = account("evicted", None);
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        base_url,
        test_wire_profile(),
    );
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    base.for_account(&account)
        .unwrap()
        .create_response(&request, request_context("before-evict", Some("evicted")))
        .await
        .unwrap();
    provider_openai::transport::evict_account_http_clients(account.id().as_str());
    base.for_account(&account)
        .unwrap()
        .create_response(&request, request_context("after-evict", Some("evicted")))
        .await
        .unwrap();

    let connection_ids = timeout(Duration::from_secs(5), server)
        .await
        .expect("connection after account eviction")
        .unwrap();
    assert_ne!(connection_ids[0], connection_ids[1]);
}

#[tokio::test]
async fn http_sse_accounts_use_distinct_proxies_and_clearing_restores_direct_client() {
    let (exit_a, a) = http_exit("exit_a").await;
    let (exit_b, b) = http_exit("exit_b").await;
    let (direct_url, direct) = http_exit("direct").await;
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        &direct_url,
        test_wire_profile(),
    );
    let client_a = base.for_account(&account("a", Some(&exit_a))).unwrap();
    let client_b = base.for_account(&account("b", Some(&exit_b))).unwrap();
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    let (response_a, response_b) = tokio::join!(
        client_a.create_response(&request, request_context("proxy-a", Some("a"))),
        client_b.create_response(&request, request_context("proxy-b", Some("b")))
    );
    assert!(response_a.unwrap().body.contains("resp_exit_a"));
    assert!(response_b.unwrap().body.contains("resp_exit_b"));
    let cleared = client_a.for_account(&account("a", None)).unwrap();
    assert!(
        cleared
            .create_response(&request, request_context("direct", Some("a")))
            .await
            .unwrap()
            .body
            .contains("resp_direct")
    );
    for head in [a.await.unwrap(), b.await.unwrap()] {
        assert!(head.starts_with(&format!("POST {direct_url}/codex/responses HTTP/1.1")));
    }
    assert!(
        direct
            .await
            .unwrap()
            .starts_with("POST /codex/responses HTTP/1.1")
    );
}

async fn websocket_exit(label: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let connect = read_http_request(&mut stream).await;
        assert!(connect.starts_with("CONNECT upstream.invalid:80 HTTP/1.1"));
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .unwrap();
        let mut websocket = accept_codex_test_websocket(stream).await;
        assert!(matches!(websocket.next().await, Some(Ok(Message::Text(_)))));
        websocket
            .send(Message::Text(
                completed_websocket_response(label, 2, 1).into(),
            ))
            .await
            .unwrap();
        connect
    });
    (format!("http://user:pass@{address}"), task)
}

#[tokio::test]
async fn websocket_connect_and_pool_are_isolated_when_account_proxy_changes() {
    let (exit_a, a) = websocket_exit("resp_exit_a").await;
    let (exit_b, b) = websocket_exit("resp_exit_b").await;
    let pool = Arc::new(CodexWebSocketPool::new(Duration::from_mins(1)));
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        "http://upstream.invalid",
        test_wire_profile(),
    )
    .with_websocket_pool(pool);
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.set_previous_response_id(Some("resp_previous".to_owned()));
    request.previous_response_scope = Some(PreviousResponseScope::Persisted);
    for (proxy, expected) in [(exit_a, "resp_exit_a"), (exit_b, "resp_exit_b")] {
        let client = base
            .for_account(&account("same-account", Some(&proxy)))
            .unwrap();
        let response = timeout(
            Duration::from_secs(5),
            client.create_response(&request, request_context("proxy-ws", Some("same-account"))),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(response.body.contains(expected));
    }
    for head in [a.await.unwrap(), b.await.unwrap()] {
        assert_eq!(
            read_header_value(&head, "Proxy-Authorization"),
            Some("Basic dXNlcjpwYXNz")
        );
    }
}

#[tokio::test]
async fn unavailable_proxy_fails_without_direct_fallback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unreachable = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let direct = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        format!("http://{}", direct.local_addr().unwrap()),
        test_wire_profile(),
    );
    let client = base.for_account(&account("a", Some(&unreachable))).unwrap();
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    assert!(
        client
            .create_response(&request, request_context("blocked", Some("a")))
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(30), direct.accept())
            .await
            .is_err()
    );
}

async fn socks_exit(stream: &mut TcpStream) -> (u8, String, u16) {
    assert_eq!(stream.read_u8().await.unwrap(), 5);
    let count = stream.read_u8().await.unwrap();
    let mut methods = vec![0; usize::from(count)];
    stream.read_exact(&mut methods).await.unwrap();
    assert!(methods.contains(&2));
    stream.write_all(&[5, 2]).await.unwrap();
    assert_eq!(stream.read_u8().await.unwrap(), 1);
    let count = stream.read_u8().await.unwrap();
    let mut user = vec![0; usize::from(count)];
    stream.read_exact(&mut user).await.unwrap();
    let count = stream.read_u8().await.unwrap();
    let mut password = vec![0; usize::from(count)];
    stream.read_exact(&mut password).await.unwrap();
    assert_eq!(user, b"user@exit");
    assert_eq!(password, b"pass:word");
    stream.write_all(&[1, 0]).await.unwrap();
    let mut header = [0; 4];
    stream.read_exact(&mut header).await.unwrap();
    assert_eq!(&header[..3], &[5, 1, 0]);
    let host = match header[3] {
        1 => {
            let mut ip = [0; 4];
            stream.read_exact(&mut ip).await.unwrap();
            std::net::Ipv4Addr::from(ip).to_string()
        }
        4 => {
            let mut ip = [0; 16];
            stream.read_exact(&mut ip).await.unwrap();
            std::net::Ipv6Addr::from(ip).to_string()
        }
        3 => {
            let count = stream.read_u8().await.unwrap();
            let mut host = vec![0; usize::from(count)];
            stream.read_exact(&mut host).await.unwrap();
            String::from_utf8(host).unwrap()
        }
        _ => panic!("unexpected SOCKS address type"),
    };
    let port = stream.read_u16().await.unwrap();
    stream
        .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
        .await
        .unwrap();
    (header[3], host, port)
}

#[tokio::test]
async fn socks_account_egress_preserves_dns_mode_auth_and_ipv6_for_sse_and_websocket() {
    for websocket in [false, true] {
        let mut targets = vec![
            ("socks5", "localhost"),
            ("socks5h", "upstream.invalid"),
            ("socks5h", "[::1]"),
        ];
        // reqwest's local resolver does not accept IPv6 literals; use socks5h for those.
        if websocket {
            targets.push(("socks5", "[::1]"));
        }
        for (scheme, host) in targets {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy = format!(
                "{scheme}://user%40exit:pass%3Aword@{}",
                listener.local_addr().unwrap()
            );
            let task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let target = socks_exit(&mut stream).await;
                if websocket {
                    let mut stream = accept_codex_test_websocket(stream).await;
                    assert!(matches!(stream.next().await, Some(Ok(Message::Text(_)))));
                    stream
                        .send(Message::Text(
                            completed_websocket_response("resp_socks", 2, 1).into(),
                        ))
                        .await
                        .unwrap();
                } else {
                    read_http_request_with_body(&mut stream).await;
                    let body = format!(
                        "data: {}\n\n",
                        completed_websocket_response("resp_socks", 2, 1)
                    );
                    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
                target
            });
            let client = CodexBackendClient::new(
                reqwest::Client::builder().no_proxy().build().unwrap(),
                format!("http://{host}"),
                test_wire_profile(),
            )
            .for_account(&account("socks", Some(&proxy)))
            .unwrap();
            let mut request = codex_request("gpt-5.5", "", Vec::new());
            request.force_http_sse = !websocket;
            if websocket {
                request.set_previous_response_id(Some("resp_previous".to_owned()));
                request.previous_response_scope = Some(PreviousResponseScope::Persisted);
            }
            let response = timeout(
                Duration::from_secs(5),
                client.create_response(&request, request_context("socks", Some("socks"))),
            )
            .await
            .unwrap()
            .unwrap_or_else(|error| {
                panic!("websocket={websocket}, scheme={scheme}, host={host}: {error}")
            });
            assert!(response.body.contains("resp_socks"));
            let (address_type, target, port) = task.await.unwrap();
            assert_eq!(port, 80);
            match host {
                "localhost" => {
                    assert!(matches!(address_type, 1 | 4));
                    assert!(target.parse::<std::net::IpAddr>().unwrap().is_loopback());
                }
                "[::1]" => assert_eq!((address_type, target.as_str()), (4, "::1")),
                _ => assert_eq!((address_type, target.as_str()), (3, host)),
            }
        }
    }
}

#[tokio::test]
async fn https_account_proxy_starts_tls_and_rejection_never_falls_back_to_direct() {
    for websocket in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("https://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut header = [0; 5];
            stream.read_exact(&mut header).await.unwrap();
            assert_eq!(&header[..2], &[22, 3]);
            let mut hello = vec![0; usize::from(u16::from_be_bytes([header[3], header[4]]))];
            stream.read_exact(&mut hello).await.unwrap();
            stream.write_all(&[21, 3, 3, 0, 2, 2, 40]).await.unwrap();
        });
        let direct = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = CodexBackendClient::new(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            format!("http://{}", direct.local_addr().unwrap()),
            test_wire_profile(),
        )
        .for_account(&account("tls-proxy", Some(&proxy)))
        .unwrap();
        let mut request = codex_request("gpt-5.5", "", Vec::new());
        request.force_http_sse = !websocket;
        if websocket {
            request.set_previous_response_id(Some("resp_previous".to_owned()));
            request.previous_response_scope = Some(PreviousResponseScope::Persisted);
        }
        assert!(
            timeout(
                Duration::from_secs(5),
                client.create_response(&request, request_context("tls-proxy", Some("tls-proxy")))
            )
            .await
            .unwrap()
            .is_err()
        );
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            timeout(Duration::from_millis(30), direct.accept())
                .await
                .is_err()
        );
    }
}

// 以底层写入调用为边界模拟问题代理，避免将 TCP 单次读取误当成报文边界。
struct GreetingSensitiveProxy {
    greeting: Vec<u8>,
    replies: Cursor<Vec<u8>>,
    writes: Vec<Vec<u8>>,
}

impl GreetingSensitiveProxy {
    fn new(authenticated: bool) -> Self {
        let (greeting, mut replies) = if authenticated {
            (vec![5, 2, 0, 2], vec![5, 2, 1, 0])
        } else {
            (vec![5, 1, 0], vec![5, 0])
        };
        replies.extend_from_slice(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80]);
        Self {
            greeting,
            replies: Cursor::new(replies),
            writes: Vec::new(),
        }
    }
}

impl AsyncRead for GreetingSensitiveProxy {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.writes.first() != Some(&self.greeting) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "proxy rejected partial method negotiation",
            )));
        }
        if buf.remaining() > 0 {
            let mut byte = [0];
            let count = Read::read(&mut self.replies, &mut byte)?;
            buf.put_slice(&byte[..count]);
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for GreetingSensitiveProxy {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.writes.push(buf.to_vec());
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn socks_proxy_flush_contract_coalesces_greeting_without_losing_auth_or_connect() {
    use tokio_tungstenite::proxy::connect_via_proxy;
    use tungstenite::proxy::ProxyConfig;

    for scheme in ["socks5", "socks5h"] {
        for authenticated in [false, true] {
            let authentication = if authenticated { "user:pass@" } else { "" };
            let config =
                ProxyConfig::parse(&format!("{scheme}://{authentication}127.0.0.1:1080")).unwrap();
            let raw = connect_via_proxy(
                GreetingSensitiveProxy::new(authenticated),
                &config,
                "upstream.invalid",
                443,
            )
            .await;
            assert!(matches!(raw, Err(tungstenite::Error::Io(ref error))
                if error.kind() == io::ErrorKind::UnexpectedEof));

            let buffered = connect_via_proxy(
                BufWriter::new(GreetingSensitiveProxy::new(authenticated)),
                &config,
                "upstream.invalid",
                443,
            )
            .await
            .unwrap();
            assert!(buffered.buffer().is_empty());
            let stream = buffered.into_inner();
            let mut expected = vec![stream.greeting];
            if authenticated {
                expected.push(b"\x01\x04user\x04pass".to_vec());
            }
            expected.push(b"\x05\x01\x00\x03\x10upstream.invalid\x01\xbb".to_vec());
            assert_eq!(stream.writes, expected);
            assert_eq!(
                stream.replies.position(),
                stream.replies.get_ref().len() as u64
            );
        }
    }
}
