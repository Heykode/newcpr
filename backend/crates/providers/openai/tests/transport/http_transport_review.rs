//! Bounded synthetic loopback verification, not an upstream throughput benchmark.

use std::{
    collections::BTreeSet,
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
use provider_openai::transport::tls::{
    CODEX_CA_CERT_ENV, SSL_CERT_FILE_ENV, build_reqwest_native_client_with_custom_ca,
};
use tokio::{
    net::TcpListener,
    sync::{Barrier, Notify, oneshot},
    task::JoinSet,
    time::timeout,
};

use super::native_tls::{accept, acceptor, ca_identity, identity_signed_by};

const CONCURRENCY: usize = 4;
const WAVES: usize = 4;
const REQUESTS: usize = 2 + CONCURRENCY * WAVES;

async fn exchange(
    client: &reqwest::Client,
    url: &str,
    index: usize,
    releases: &[Arc<Notify>],
    expected: Version,
    barrier: Option<&Barrier>,
) -> (u128, u128, usize) {
    let request = reqwest::Request::new(
        reqwest::Method::GET,
        format!("{url}/{index}").parse().unwrap(),
    );
    let started = Instant::now();
    let response = client.execute(request).await.unwrap();
    assert_eq!(response.version(), expected);
    let connection = response.headers()["x-test-connection"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
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
    (first, started.elapsed().as_micros(), connection)
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
    let h2 = mode.starts_with("native-h2");
    let expected = if h2 {
        Version::HTTP_2
    } else {
        Version::HTTP_11
    };
    let acceptor = acceptor(&identity, h2);
    let client = build_reqwest_native_client_with_custom_ca(
        reqwest::Client::builder()
            .no_proxy()
            .tcp_nodelay(true)
            .pool_max_idle_per_host(CONCURRENCY)
            .timeout(Duration::from_secs(10)),
    )
    .unwrap();
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
    let (stop, mut stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut handlers = JoinSet::new();
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                completed = handlers.join_next(), if !handlers.is_empty() => {
                    completed.unwrap().unwrap();
                }
                // Speculative or incomplete dials must not block real requests.
                // Reuse is measured on response connection IDs, not accept count.
                incoming = listener.accept() => {
                    let (stream, _) = incoming.unwrap();
                    stream.set_nodelay(true).unwrap();
                    let connection = accepted.fetch_add(1, Ordering::SeqCst) + 1;
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
                                    .header("x-test-connection", connection)
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
    assert_eq!(cold.2, warm.2, "sequential requests must reuse TLS");
    let _pending_dial = if mode.ends_with("-pending") {
        Some(
            tokio::net::TcpStream::connect(url.trim_start_matches("https://"))
                .await
                .unwrap(),
        )
    } else {
        None
    };
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
    let parallel_connections = samples.iter().map(|value| value.2).collect::<BTreeSet<_>>();
    let mut used_connections = parallel_connections.clone();
    used_connections.extend([cold.2, warm.2]);
    if h2 {
        assert_eq!(
            used_connections.len(),
            1,
            "HTTP/2 must multiplex the same TLS connection"
        );
    } else {
        let mut previous_wave = BTreeSet::from([cold.2]);
        for wave in samples.chunks(CONCURRENCY) {
            let connections = wave.iter().map(|value| value.2).collect::<BTreeSet<_>>();
            assert_eq!(
                connections.len(),
                CONCURRENCY,
                "HTTP/1 streaming requests must progress concurrently"
            );
            assert!(
                !connections.is_disjoint(&previous_wave),
                "each HTTP/1 wave must reuse a previously active connection"
            );
            previous_wave = connections;
        }
    }
    let mut first_events = samples.iter().map(|value| value.0).collect::<Vec<_>>();
    first_events.sort_unstable();
    println!(
        "LOOPBACK_REVIEW {}",
        serde_json::json!({
            "mode": mode, "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "http": format!("{expected:?}"), "requests": REQUESTS, "concurrency": CONCURRENCY,
            "accepted_tcp_connections": connections.load(Ordering::SeqCst),
            "used_connections": used_connections.len(),
            "cold_first_event_us": cold.0, "warm_first_event_us": warm.0,
            "pending_dial": mode.ends_with("-pending"),
            "parallel_first_event_p50_us": first_events[first_events.len() / 2],
            "parallel_first_event_p95_us": first_events[(first_events.len() * 95).div_ceil(100) - 1],
            "parallel_elapsed_us": elapsed, "artificial_tail_delay_ms": 10
        })
    );
    stop.send(()).unwrap();
    server.await.unwrap();
}

#[test]
fn unified_transport_streams_and_reuses_h2_and_h1() {
    const CASE: &str = "CPR_TEST_HTTP_TRANSPORT_REVIEW";
    if let Ok(mode) = std::env::var(CASE) {
        assert!(matches!(
            mode.as_str(),
            "native-h2" | "native-h1" | "native-h2-pending" | "native-h1-pending"
        ));
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
    for mode in [
        "native-h2",
        "native-h1",
        "native-h2-pending",
        "native-h1-pending",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "transport::http_transport_review::unified_transport_streams_and_reuses_h2_and_h1",
                "--nocapture",
            ])
            .env(CASE, mode)
            .env(CODEX_CA_CERT_ENV, directory.path().join("ca.pem"))
            .env_remove(SSL_CERT_FILE_ENV)
            .output()
            .unwrap();
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
