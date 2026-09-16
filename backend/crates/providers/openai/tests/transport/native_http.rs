use std::{convert::Infallible, time::Duration};

use bytes::Bytes;
use futures::StreamExt;
use gateway_core::account::OutboundProxy;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::{TokioExecutor, TokioIo};
use provider_openai::transport::native_http::{
    NativeHttpClient, NativeHttpConfig, NativeRequestPolicy, NativeTransportError,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use super::native_tls::{accept, acceptor, connector, identity};

const LIMIT: Duration = Duration::from_secs(10);

async fn headers<S: AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 32768);
        bytes.push(stream.read_u8().await.unwrap());
    }
    String::from_utf8(bytes).unwrap()
}

fn request(url: &str) -> reqwest::Request {
    reqwest::Client::new().get(url).build().unwrap()
}

#[tokio::test]
async fn native_http_preserves_json_response_url_headers_and_reuses_h1_connection() {
    timeout(LIMIT, async {
        let identity = identity();
        let client = NativeHttpClient::with_tls(NativeHttpConfig::default(), connector(&identity)).unwrap();
        let acceptor = acceptor(&identity, false);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://localhost:{}/v1/responses?x=1", listener.local_addr().unwrap().port());
        let server = async {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = accept(&acceptor, stream).await;
            for _ in 0..2 {
                let headers = headers(&mut stream).await;
                assert!(headers.starts_with("POST /v1/responses?x=1 HTTP/1.1\r\n"));
                assert!(headers.to_ascii_lowercase().contains("content-type: application/json"));
                let mut body = [0; 7];
                stream.read_exact(&mut body).await.unwrap();
                assert_eq!(&body, br#"{"x":1}"#);
                stream.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 11\r\nX-Test: preserved\r\n\r\n{\"ok\":true}").await.unwrap();
            }
        };
        let requests = async {
            for _ in 0..2 {
                let request = reqwest::Client::new().post(&url).json(&serde_json::json!({"x":1})).build().unwrap();
                let response = client.execute(request).await.unwrap();
                assert_eq!(response.url().as_str(), url);
                assert_eq!(response.status(), 201);
                assert_eq!(response.headers()["x-test"], "preserved");
                assert_eq!(response.json::<serde_json::Value>().await.unwrap(), serde_json::json!({"ok":true}));
            }
        };
        tokio::join!(server, requests);
    }).await.unwrap();
}

#[tokio::test]
async fn native_http_negotiates_h2_and_preserves_streamed_upload() {
    timeout(LIMIT, async {
        let identity = identity();
        let client = NativeHttpClient::with_tls(NativeHttpConfig::default(), connector(&identity)).unwrap();
        let acceptor = acceptor(&identity, true);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://localhost:{}/", listener.local_addr().unwrap().port());
        let (done, stopped) = oneshot::channel();
        let server = async {
            let (stream, _) = listener.accept().await.unwrap();
            let stream = accept(&acceptor, stream).await;
            assert_eq!(stream.ssl().selected_alpn_protocol(), Some(b"h2".as_slice()));
            let service = hyper::service::service_fn(|request: hyper::Request<hyper::body::Incoming>| async {
                assert_eq!(request.into_body().collect().await.unwrap().to_bytes(), "onetwo");
                Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from_static(b"ok"))))
            });
            tokio::select! {
                result = hyper::server::conn::http2::Builder::new(TokioExecutor::new()).serve_connection(TokioIo::new(stream), service) => { result.unwrap(); }
                _ = stopped => {}
            }
        };
        let requests = async {
            let body = reqwest::Body::wrap_stream(futures::stream::iter([
                Ok::<_, std::io::Error>(Bytes::from_static(b"one")),
                Ok(Bytes::from_static(b"two")),
            ]));
            let request = reqwest::Client::new().post(url).body(body).build().unwrap();
            let response = client.execute(request).await.unwrap();
            assert_eq!(response.version(), hyper::Version::HTTP_2);
            assert_eq!(response.text().await.unwrap(), "ok");
            done.send(()).unwrap();
        };
        tokio::join!(server, requests);
    }).await.unwrap();
}

#[tokio::test]
async fn native_http_delivers_sse_before_completion_and_enforces_body_deadline() {
    timeout(LIMIT, async {
        let identity = identity();
        let client = NativeHttpClient::with_tls(NativeHttpConfig::default(), connector(&identity)).unwrap();
        let acceptor = acceptor(&identity, false);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://localhost:{}/", listener.local_addr().unwrap().port());
        let (release, wait) = oneshot::channel();
        let server = async {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = accept(&acceptor, stream).await;
            headers(&mut stream).await;
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n9\r\ndata: 1\n\n\r\n").await.unwrap();
            wait.await.unwrap();
            stream.write_all(b"9\r\ndata: 2\n\n\r\n").await.unwrap();
            let mut byte = [0];
            let _ = stream.read(&mut byte).await;
        };
        let requests = async {
            let request = reqwest::Client::new().get(url).timeout(Duration::from_secs(1)).build().unwrap();
            let mut stream = client.execute(request).await.unwrap().bytes_stream();
            assert_eq!(stream.next().await.unwrap().unwrap(), "data: 1\n\n");
            assert!(timeout(Duration::from_millis(30), stream.next()).await.is_err());
            release.send(()).unwrap();
            assert_eq!(stream.next().await.unwrap().unwrap(), "data: 2\n\n");
            let error = stream.next().await.unwrap().unwrap_err();
            assert!(error.is_timeout(), "{error}");
        };
        tokio::join!(server, requests);
    }).await.unwrap();
}

#[tokio::test]
async fn native_http_cancellation_after_headers_drops_body_and_pre_cancel_never_dials() {
    timeout(LIMIT, async {
        let identity = identity();
        let client =
            NativeHttpClient::with_tls(NativeHttpConfig::default(), connector(&identity)).unwrap();
        let acceptor = acceptor(&identity, false);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "https://localhost:{}/",
            listener.local_addr().unwrap().port()
        );
        let token = CancellationToken::new();
        token.cancel();
        let policy = NativeRequestPolicy {
            deadline: None,
            cancellation: Some(token),
        };
        assert!(
            client
                .execute_with_policy(request(&url), policy)
                .await
                .is_err()
        );
        assert!(
            timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
        let server = async {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = accept(&acceptor, stream).await;
            headers(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\n")
                .await
                .unwrap();
            let mut byte = [0];
            assert!(!matches!(stream.read(&mut byte).await, Ok(1..)));
        };
        let requests = async {
            let token = CancellationToken::new();
            let policy = NativeRequestPolicy {
                deadline: None,
                cancellation: Some(token.clone()),
            };
            let response = client
                .execute_with_policy(request(&url), policy)
                .await
                .unwrap();
            token.cancel();
            let error = response.bytes().await.unwrap_err();
            assert!(!error.is_timeout());
        };
        tokio::join!(server, requests);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_http_distinguishes_connect_from_ambiguous_send_without_retry() {
    timeout(LIMIT, async {
        let identity = identity();
        let client =
            NativeHttpClient::with_tls(NativeHttpConfig::default(), connector(&identity)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        drop(listener);
        let error = client.execute(request(&url)).await.unwrap_err();
        assert!(error.is_connect(), "{error:?}");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = async {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert!(headers(&mut stream).await.starts_with("GET / "));
        };
        let requests = async {
            let error = client.execute(request(&url)).await.unwrap_err();
            assert!(matches!(error, NativeTransportError::Request { .. }));
        };
        tokio::join!(server, requests);
        assert!(
            timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_http_proxy_connect_keeps_credentials_out_of_origin() {
    timeout(LIMIT, async {
        let identity = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = NativeHttpConfig {
            proxy: Some(
                OutboundProxy::parse(&format!(
                    "http://user:password@{}",
                    listener.local_addr().unwrap()
                ))
                .unwrap(),
            ),
            ..Default::default()
        };
        let client = NativeHttpClient::with_tls(config, connector(&identity)).unwrap();
        let acceptor = acceptor(&identity, false);
        let server = async {
            let (mut stream, _) = listener.accept().await.unwrap();
            let connect = headers(&mut stream).await.to_ascii_lowercase();
            assert!(connect.starts_with("connect localhost:443 http/1.1"));
            assert!(connect.contains("proxy-authorization: basic "));
            stream
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await
                .unwrap();
            let mut stream = accept(&acceptor, stream).await;
            let origin = headers(&mut stream).await.to_ascii_lowercase();
            assert!(!origin.contains("proxy-authorization"));
            assert!(!origin.contains("password"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        };
        let requests = async {
            let response = client.execute(request("https://localhost/")).await.unwrap();
            assert_eq!(response.text().await.unwrap(), "ok");
        };
        tokio::join!(server, requests);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_http_forward_proxy_and_https_proxy_outer_tls() {
    timeout(LIMIT, async {
        let identity = identity();
        for secure_proxy in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let scheme = if secure_proxy { "https" } else { "http" };
            let config = NativeHttpConfig {
                proxy: Some(
                    OutboundProxy::parse(&format!(
                        "{scheme}://user:pass@localhost:{}",
                        listener.local_addr().unwrap().port()
                    ))
                    .unwrap(),
                ),
                ..Default::default()
            };
            let client = NativeHttpClient::with_tls(config, connector(&identity)).unwrap();
            let acceptor = acceptor(&identity, false);
            let server = async {
                let (mut stream, _) = listener.accept().await.unwrap();
                if secure_proxy {
                    let mut stream = accept(&acceptor, stream).await;
                    serve_forward(&mut stream).await;
                } else {
                    serve_forward(&mut stream).await;
                }
            };
            let requests = async {
                let response = client
                    .execute(request("http://origin.invalid/path?x=1"))
                    .await
                    .unwrap();
                assert_eq!(response.text().await.unwrap(), "ok");
            };
            tokio::join!(server, requests);
        }
    })
    .await
    .unwrap();
}

async fn serve_forward<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S) {
    let request = headers(stream).await.to_ascii_lowercase();
    assert!(request.starts_with("get http://origin.invalid/path?x=1 http/1.1"));
    assert!(request.contains("proxy-authorization: basic "));
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
        .await
        .unwrap();
}

#[tokio::test]
async fn native_http_socks_preserves_local_and_remote_dns() {
    timeout(LIMIT, async {
        let identity = identity();
        for remote in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let scheme = if remote { "socks5h" } else { "socks5" };
            let host = if remote {
                "unresolved.invalid"
            } else {
                "localhost"
            };
            let config = NativeHttpConfig {
                proxy: Some(
                    OutboundProxy::parse(&format!("{scheme}://{}", listener.local_addr().unwrap()))
                        .unwrap(),
                ),
                ..Default::default()
            };
            let client = NativeHttpClient::with_tls(config, connector(&identity)).unwrap();
            let server = async {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(stream.read_u8().await.unwrap(), 5);
                let count = stream.read_u8().await.unwrap();
                let mut methods = vec![0; usize::from(count)];
                stream.read_exact(&mut methods).await.unwrap();
                stream.write_all(&[5, 0]).await.unwrap();
                let mut request = [0; 4];
                stream.read_exact(&mut request).await.unwrap();
                assert_eq!(&request[..3], &[5, 1, 0]);
                if remote {
                    assert_eq!(request[3], 3);
                    let length = stream.read_u8().await.unwrap();
                    let mut domain = vec![0; usize::from(length)];
                    stream.read_exact(&mut domain).await.unwrap();
                    assert_eq!(domain, host.as_bytes());
                } else {
                    assert!(matches!(request[3], 1 | 4), "local DNS must send an IP");
                    let mut ip = vec![0; if request[3] == 1 { 4 } else { 16 }];
                    stream.read_exact(&mut ip).await.unwrap();
                }
                assert_eq!(stream.read_u16().await.unwrap(), 80);
                stream
                    .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                    .await
                    .unwrap();
                assert!(headers(&mut stream).await.starts_with("GET / HTTP/1.1"));
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await
                    .unwrap();
            };
            let requests = async {
                assert_eq!(
                    client
                        .execute(request(&format!("http://{host}/")))
                        .await
                        .unwrap()
                        .text()
                        .await
                        .unwrap(),
                    "ok"
                );
            };
            tokio::join!(server, requests);
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_http_ipv6_source_is_preserved_and_proxy_conflict_fails_closed() {
    timeout(LIMIT, async {
        let identity = identity();
        let config = NativeHttpConfig {
            source: Some(std::net::Ipv6Addr::LOCALHOST),
            ..Default::default()
        };
        let client = NativeHttpClient::with_tls(config.clone(), connector(&identity)).unwrap();
        let conflict = NativeHttpConfig {
            proxy: Some(OutboundProxy::parse("http://localhost:8080").unwrap()),
            ..config
        };
        let error = match NativeHttpClient::with_tls(conflict, connector(&identity)) {
            Ok(_) => panic!("proxy/source conflict accepted"),
            Err(error) => error,
        };
        assert!(error.is_configuration());
        assert_eq!(
            error.egress_error(),
            Some(provider_openai::transport::egress::CodexEgressError::ProxyConflict)
        );
        let listener = TcpListener::bind("[::1]:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = async {
            let (mut stream, peer) = listener.accept().await.unwrap();
            assert_eq!(peer.ip(), std::net::Ipv6Addr::LOCALHOST);
            headers(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        };
        let requests = async {
            assert_eq!(
                client
                    .execute(request(&url))
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap(),
                "ok"
            );
        };
        tokio::join!(server, requests);
        let error = client
            .execute(request("http://127.0.0.1:1/"))
            .await
            .unwrap_err();
        assert_eq!(
            error.egress_error(),
            Some(provider_openai::transport::egress::CodexEgressError::DestinationUnavailable)
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_http_cached_identity_isolates_pools_and_fresh_never_reuses() {
    timeout(LIMIT, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let key = uuid::Uuid::new_v4().to_string();
        let config = NativeHttpConfig {
            cache_key: key.clone(),
            ..Default::default()
        };
        let other = NativeHttpConfig {
            cache_key: format!("{key}-other"),
            ..config.clone()
        };
        let fresh = NativeHttpConfig {
            fresh: true,
            ..config.clone()
        };
        let server = async {
            let mut retained = Vec::new();
            for count in [2, 2, 1, 1] {
                let (mut stream, _) = listener.accept().await.unwrap();
                for _ in 0..count {
                    headers(&mut stream).await;
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                        .await
                        .unwrap();
                }
                retained.push(stream);
            }
        };
        let requests = async {
            for config in [config, other, fresh] {
                for _ in 0..2 {
                    let client = NativeHttpClient::cached_async(config.clone())
                        .await
                        .unwrap();
                    assert_eq!(
                        client
                            .execute(request(&url))
                            .await
                            .unwrap()
                            .text()
                            .await
                            .unwrap(),
                        "ok"
                    );
                }
            }
        };
        tokio::join!(server, requests);
    })
    .await
    .unwrap();
}
