use std::{convert::Infallible, time::Duration};

use bytes::Bytes;
use futures::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use super::native_tls::{accept, acceptor, ca_identity, http_client, identity_signed_by};

const LIMIT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn native_http2_refused_stream_is_not_replayed_by_the_client() {
    timeout(LIMIT, async {
        let root = ca_identity();
        let identity = identity_signed_by(&root);
        let client = http_client(&root, reqwest::Client::builder().no_proxy());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "https://localhost:{}/",
            listener.local_addr().unwrap().port()
        );
        let acceptor = acceptor(&identity, true);
        let (finished, mut finish) = oneshot::channel();
        let server = async {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = accept(&acceptor, stream).await;
            assert_eq!(
                stream.ssl().selected_alpn_protocol(),
                Some(b"h2".as_slice())
            );
            let mut preface = [0; 24];
            stream.read_exact(&mut preface).await.unwrap();
            assert_eq!(&preface, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
            stream
                .write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0])
                .await
                .unwrap();
            let mut streams = 0;
            let mut received = Vec::new();
            loop {
                let mut header = [0; 9];
                tokio::select! {
                    _ = &mut finish => break,
                    read = stream.read_exact(&mut header) => { read.unwrap(); }
                }
                let length = usize::from(header[0]) << 16
                    | usize::from(header[1]) << 8
                    | usize::from(header[2]);
                assert!(length <= 16384);
                let mut payload = vec![0; length];
                stream.read_exact(&mut payload).await.unwrap();
                if header[3] == 4 && header[4] == 0 {
                    stream
                        .write_all(&[0, 0, 0, 4, 1, 0, 0, 0, 0])
                        .await
                        .unwrap();
                }
                if header[3] == 1 {
                    streams += 1;
                    assert_eq!(
                        streams, 1,
                        "HTTP/2 refusal must not silently replay a request"
                    );
                }
                if header[3] == 0 {
                    received.extend_from_slice(&payload);
                    if header[4] & 1 != 0 {
                        assert_eq!(received, b"synthetic-oauth-payload");
                        // RST_STREAM(REFUSED_STREAM): a retryable protocol fact, not permission
                        // for the transport to bypass the provider's retry owner.
                        let mut reset = vec![0, 0, 4, 3, 0];
                        reset.extend_from_slice(&header[5..]);
                        reset.extend_from_slice(&7_u32.to_be_bytes());
                        stream.write_all(&reset).await.unwrap();
                    }
                }
            }
            assert_eq!(streams, 1);
            assert!(
                timeout(Duration::from_millis(50), listener.accept())
                    .await
                    .is_err()
            );
        };
        let send = async {
            let error = client
                .post(url)
                .body("synthetic-oauth-payload")
                .send()
                .await
                .unwrap_err();
            assert!(!error.is_connect(), "payload already reached the server");
            finished.send(()).unwrap();
        };
        tokio::join!(server, send);
    })
    .await
    .unwrap();
}

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
        let root = ca_identity();
        let identity = identity_signed_by(&root);
        let client = http_client(&root, reqwest::Client::builder().no_proxy());
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
        let root = ca_identity();
        let identity = identity_signed_by(&root);
        let client = http_client(&root, reqwest::Client::builder().no_proxy());
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
        let root = ca_identity();
        let identity = identity_signed_by(&root);
        let client = http_client(&root, reqwest::Client::builder().no_proxy());
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
        let root = ca_identity();
        let identity = identity_signed_by(&root);
        let client = http_client(&root, reqwest::Client::builder().no_proxy());
        let acceptor = acceptor(&identity, false);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "https://localhost:{}/",
            listener.local_addr().unwrap().port()
        );
        let token = CancellationToken::new();
        token.cancel();
        tokio::select! {
            biased;
            _ = token.cancelled() => {}
            _ = client.execute(request(&url)) => panic!("cancelled request was sent"),
        }
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
            let response = client.execute(request(&url)).await.unwrap();
            drop(response);
        };
        tokio::join!(server, requests);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_http_distinguishes_connect_from_ambiguous_send_without_retry() {
    timeout(LIMIT, async {
        let root = ca_identity();
        let client = http_client(&root, reqwest::Client::builder().no_proxy());
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
            assert!(!error.is_connect(), "request was sent: {error:?}");
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
        let root = ca_identity();
        let identity = identity_signed_by(&root);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = http_client(
            &root,
            reqwest::Client::builder().no_proxy().proxy(
                reqwest::Proxy::all(format!(
                    "http://user:password@{}",
                    listener.local_addr().unwrap()
                ))
                .unwrap(),
            ),
        );
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
        let root = ca_identity();
        let identity = identity_signed_by(&root);
        for secure_proxy in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let scheme = if secure_proxy { "https" } else { "http" };
            let client = http_client(
                &root,
                reqwest::Client::builder().no_proxy().proxy(
                    reqwest::Proxy::all(format!(
                        "{scheme}://user:pass@localhost:{}",
                        listener.local_addr().unwrap().port()
                    ))
                    .unwrap(),
                ),
            );
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
        let root = ca_identity();
        for remote in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let scheme = if remote { "socks5h" } else { "socks5" };
            let host = if remote {
                "unresolved.invalid"
            } else {
                "localhost"
            };
            let client = http_client(
                &root,
                reqwest::Client::builder().no_proxy().proxy(
                    reqwest::Proxy::all(format!("{scheme}://{}", listener.local_addr().unwrap()))
                        .unwrap(),
                ),
            );
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
async fn native_http_ipv6_source_is_preserved_and_ipv4_never_used() {
    timeout(LIMIT, async {
        let root = ca_identity();
        let client = http_client(
            &root,
            reqwest::Client::builder()
                .no_proxy()
                .local_address(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
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
        assert!(error.is_connect());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_http_separate_clients_isolate_pools_and_fresh_never_reuses() {
    timeout(LIMIT, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let root = ca_identity();
        let account = http_client(&root, reqwest::Client::builder().no_proxy());
        let other = http_client(&root, reqwest::Client::builder().no_proxy());
        let fresh = http_client(
            &root,
            reqwest::Client::builder()
                .no_proxy()
                .pool_max_idle_per_host(0),
        );
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
            for client in [account, other, fresh] {
                for _ in 0..2 {
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
