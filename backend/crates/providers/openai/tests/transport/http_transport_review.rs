//! Bounded synthetic loopback verification, not an upstream throughput benchmark.

use std::{
    convert::Infallible,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use futures::{StreamExt, future::join_all, stream};
use http_body_util::StreamBody;
use hyper::{Version, body::Frame};
use hyper_util::rt::{TokioExecutor, TokioIo};
use openssl::{
    stack::Stack,
    x509::{X509StoreContext, store::X509StoreBuilder},
};
use provider_openai::transport::{
    native_http::{NativeHttpClient, NativeHttpConfig},
    tls::{CODEX_CA_CERT_ENV, SSL_CERT_FILE_ENV, build_reqwest_native_client_with_custom_ca},
};
use tokio::{
    net::TcpListener,
    sync::{Barrier, Notify, oneshot},
    task::JoinSet,
    time::timeout,
};

use super::native_tls::{accept, acceptor, ca_identity, connector, identity_signed_by};

const CONCURRENCY: usize = 4;
const WAVES: usize = 4;
const REQUESTS: usize = 2 + CONCURRENCY * WAVES;

enum Client {
    Native(reqwest::Client),
    Qx(Box<NativeHttpClient>),
}

impl Client {
    async fn execute(&self, request: reqwest::Request) -> reqwest::Response {
        match self {
            Self::Native(client) => client.execute(request).await.unwrap(),
            Self::Qx(client) => client.execute(request).await.unwrap(),
        }
    }
}

async fn exchange(
    client: &Client,
    url: &str,
    index: usize,
    releases: &[Arc<Notify>],
    expected: Version,
    barrier: Option<&Barrier>,
) -> (u128, u128) {
    let request = reqwest::Request::new(
        reqwest::Method::GET,
        format!("{url}/{index}").parse().unwrap(),
    );
    let started = Instant::now();
    let response = client.execute(request).await;
    assert_eq!(response.version(), expected);
    let mut body = response.bytes_stream();
    let mut event = Vec::new();
    while event.len() < b"data: first\n\n".len() {
        event.extend_from_slice(&body.next().await.unwrap().unwrap());
    }
    assert_eq!(event, b"data: first\n\n");
    let first = started.elapsed().as_micros();
    // The server cannot finish until the client sees the first event.
    if let Some(barrier) = barrier {
        barrier.wait().await;
    }
    releases[index].notify_one();
    let mut tail = Vec::new();
    while let Some(chunk) = body.next().await {
        tail.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(tail, b"data: done\n\n");
    (first, started.elapsed().as_micros())
}

async fn review(mode: &str) {
    let root = ca_identity();
    let identity = identity_signed_by(&root);
    let mut trust = X509StoreBuilder::new().unwrap();
    trust.add_cert(root.cert.clone()).unwrap();
    X509StoreContext::new()
        .unwrap()
        .init(
            &trust.build(),
            &identity.cert,
            &Stack::new().unwrap(),
            |context| {
                assert!(
                    context.verify_cert()?,
                    "test certificate chain: {}",
                    context.error()
                );
                Ok(())
            },
        )
        .unwrap();
    std::fs::write(
        std::env::var(CODEX_CA_CERT_ENV).unwrap(),
        root.cert.to_pem().unwrap(),
    )
    .unwrap();
    let h2 = mode == "qx-h2";
    let expected = if h2 {
        Version::HTTP_2
    } else {
        Version::HTTP_11
    };
    let acceptor = acceptor(&identity, mode != "qx-h1");
    let client = if mode == "native" {
        Client::Native(
            build_reqwest_native_client_with_custom_ca(
                reqwest::Client::builder()
                    .no_proxy()
                    .tcp_nodelay(true)
                    .pool_max_idle_per_host(CONCURRENCY)
                    .timeout(Duration::from_secs(10)),
            )
            .unwrap(),
        )
    } else {
        Client::Qx(Box::new(
            NativeHttpClient::with_tls(
                NativeHttpConfig {
                    timeout: Some(Duration::from_secs(10)),
                    ..Default::default()
                },
                connector(&root),
            )
            .unwrap(),
        ))
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://{}", listener.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let accepted = Arc::clone(&connections);
    let releases = Arc::new(
        (0..REQUESTS)
            .map(|_| Arc::new(Notify::new()))
            .collect::<Vec<_>>(),
    );
    let server_releases = Arc::clone(&releases);
    let (allow_parallel, mut parallel_allowed) = oneshot::channel();
    let (stop, mut stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut handlers = JoinSet::new();
        let mut parallel = false;
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                _ = &mut parallel_allowed, if !parallel => parallel = true,
                completed = handlers.join_next(), if !handlers.is_empty() => {
                    completed.unwrap().unwrap();
                }
                // Hyper may speculatively connect while waiting for an idle socket.
                // Cap completed handshakes so progress proves real pool reuse,
                // without depending on when those background dials finish.
                incoming = listener.accept(), if accepted.load(Ordering::SeqCst)
                    < if parallel && !h2 { CONCURRENCY } else { 1 } => {
                    let (stream, _) = incoming.unwrap();
                    stream.set_nodelay(true).unwrap();
                    accepted.fetch_add(1, Ordering::SeqCst);
                    let acceptor = acceptor.clone();
                    let releases = Arc::clone(&server_releases);
                    handlers.spawn(async move {
                        let stream = accept(&acceptor, stream).await;
                        assert_eq!(
                            stream.ssl().selected_alpn_protocol(),
                            h2.then_some(b"h2".as_slice())
                        );
                        let service = hyper::service::service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                            assert_eq!(request.version(), expected);
                            let index: usize = request.uri().path().trim_start_matches('/').parse().unwrap();
                            let release = Arc::clone(&releases[index]);
                            let first = stream::iter([Ok::<_, Infallible>(Frame::data(Bytes::from_static(b"data: first\n\n")))]);
                            let tail = stream::once(async move {
                                release.notified().await;
                                tokio::time::sleep(Duration::from_millis(10)).await;
                                Ok::<_, Infallible>(Frame::data(Bytes::from_static(b"data: done\n\n")))
                            });
                            async move {
                                Ok::<_, Infallible>(hyper::Response::builder()
                                    .header("content-type", "text/event-stream")
                                    .body(StreamBody::new(first.chain(tail))).unwrap())
                            }
                        });
                        if h2 {
                            hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                                .serve_connection(TokioIo::new(stream), service).await.unwrap();
                        } else {
                            hyper::server::conn::http1::Builder::new()
                                .serve_connection(TokioIo::new(stream), service).await.unwrap();
                        }
                    });
                }
            }
        }
        handlers.shutdown().await;
    });
    let cold = exchange(&client, &url, 0, &releases, expected, None).await;
    let warm = exchange(&client, &url, 1, &releases, expected, None).await;
    assert_eq!(
        connections.load(Ordering::SeqCst),
        1,
        "sequential requests must reuse TLS"
    );
    allow_parallel.send(()).unwrap();
    let started = Instant::now();
    let mut samples = Vec::new();
    for wave in 0..WAVES {
        let barrier = Barrier::new(CONCURRENCY);
        samples.extend(
            join_all((0..CONCURRENCY).map(|index| {
                exchange(
                    &client,
                    &url,
                    2 + wave * CONCURRENCY + index,
                    &releases,
                    expected,
                    Some(&barrier),
                )
            }))
            .await,
        );
    }
    let elapsed = started.elapsed().as_micros();
    let count = connections.load(Ordering::SeqCst);
    assert_eq!(
        count,
        if h2 { 1 } else { CONCURRENCY },
        "all requests complete using the bounded accepted connection set"
    );
    let mut first_events = samples.iter().map(|value| value.0).collect::<Vec<_>>();
    first_events.sort_unstable();
    println!(
        "LOOPBACK_REVIEW {}",
        serde_json::json!({
            "mode": mode, "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "http": format!("{expected:?}"), "requests": REQUESTS, "concurrency": CONCURRENCY,
            "tls_connections": count, "cold_first_event_us": cold.0, "warm_first_event_us": warm.0,
            "server_accept_limit": if h2 { 1 } else { CONCURRENCY },
            "parallel_first_event_p50_us": first_events[first_events.len() / 2],
            "parallel_first_event_p95_us": first_events[(first_events.len() * 95).div_ceil(100) - 1],
            "parallel_elapsed_us": elapsed, "artificial_tail_delay_ms": 10
        })
    );
    stop.send(()).unwrap();
    server.await.unwrap();
}

#[test]
fn current_transports_stream_and_reuse_without_changing_native_alpn() {
    const CASE: &str = "CPR_TEST_HTTP_TRANSPORT_REVIEW";
    if let Ok(mode) = std::env::var(CASE) {
        assert!(matches!(mode.as_str(), "native" | "qx-h2" | "qx-h1"));
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                timeout(Duration::from_secs(20), review(&mode))
                    .await
                    .expect("bounded loopback review");
            });
        return;
    }
    for mode in ["native", "qx-h2", "qx-h1"] {
        let directory = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "transport::http_transport_review::current_transports_stream_and_reuse_without_changing_native_alpn", "--nocapture"])
            .env(CASE, mode)
            .env(CODEX_CA_CERT_ENV, directory.path().join("ca.pem"))
            .env_remove(SSL_CERT_FILE_ENV)
            .output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{mode}\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            stdout
                .lines()
                .filter(|line| line.starts_with("LOOPBACK_REVIEW "))
                .count(),
            1
        );
        print!("{stdout}");
    }
}
