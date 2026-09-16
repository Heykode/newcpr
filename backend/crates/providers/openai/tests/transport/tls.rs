use std::collections::BTreeMap;

use bytes::{Buf as _, Bytes};

use super::*;

// CPR HTTP uses the linked platform native-tls backend; CPR WebSocket retains its
// captured Rustls contract. QX stays on the independently selected OpenSSL transport.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct ClientHello {
    cipher_suites: Vec<u16>,
    extensions: Vec<u16>,
    groups: Vec<u16>,
    signature_algorithms: Vec<u16>,
    key_shares: Vec<(u16, usize)>,
    alpn: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
struct CapturedClientHello {
    normalized: ClientHello,
    extension_order: Vec<u16>,
    stable_extension_payloads: BTreeMap<u16, String>,
    record_version: u16,
    record_length: usize,
}

#[test]
fn rustls_provider_preserves_cpr_profile_and_preinstalled_host() {
    use std::{process::Command, sync::Arc};

    use rustls::crypto::{CryptoProvider, aws_lc_rs};

    const CHILD: &str = "CPR_TEST_TLS_PROVIDER_CHILD";
    const COMPLETED: &str = "tls-provider-case-completed:";
    let Ok(mode) = std::env::var(CHILD) else {
        for mode in ["cpr", "host"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "transport::tls::rustls_provider_preserves_cpr_profile_and_preinstalled_host",
                    "--nocapture",
                ])
                .env(CHILD, mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            let marker = format!("{COMPLETED}{mode}");
            assert_eq!(
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter(|line| *line == marker)
                    .count(),
                1,
                "child must execute exactly once",
            );
        }
        return;
    };
    assert!(CryptoProvider::get_default().is_none());
    let original = aws_lc_rs::default_provider();
    let host = if mode == "host" {
        let mut host = original.clone();
        host.kx_groups = vec![aws_lc_rs::kx_group::X25519];
        host.install_default().unwrap();
        Some(CryptoProvider::get_default().unwrap().clone())
    } else {
        assert_eq!(mode, "cpr");
        None
    };
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(provider_openai::ensure_rustls_provider);
        }
    });
    let installed = CryptoProvider::get_default().unwrap();
    provider_openai::ensure_rustls_provider();
    assert!(Arc::ptr_eq(
        installed,
        CryptoProvider::get_default().unwrap()
    ));
    if let Some(host) = host {
        assert!(
            Arc::ptr_eq(installed, &host),
            "host provider is authoritative"
        );
        assert_eq!(
            installed
                .signature_verification_algorithms
                .supported_schemes(),
            original
                .signature_verification_algorithms
                .supported_schemes(),
        );
        println!("\n{COMPLETED}{mode}");
        return;
    }
    let algorithms = installed.signature_verification_algorithms;
    assert_eq!(
        algorithms
            .supported_schemes()
            .into_iter()
            .map(u16::from)
            .collect::<Vec<_>>(),
        official_websocket_hello().signature_algorithms,
    );
    assert!(std::ptr::eq(
        algorithms.all,
        original.signature_verification_algorithms.all,
    ));
    for (scheme, implementations) in algorithms.mapping {
        let (_, expected) = original
            .signature_verification_algorithms
            .mapping
            .iter()
            .find(|(candidate, _)| candidate == scheme)
            .unwrap();
        assert!(std::ptr::eq(*implementations, *expected));
    }
    assert_eq!(installed.cipher_suites, original.cipher_suites);
    assert_eq!(
        installed
            .kx_groups
            .iter()
            .map(|group| group.name())
            .collect::<Vec<_>>(),
        original
            .kx_groups
            .iter()
            .map(|group| group.name())
            .collect::<Vec<_>>(),
    );
    assert!(std::ptr::eq(
        installed.secure_random,
        original.secure_random
    ));
    assert!(std::ptr::eq(installed.key_provider, original.key_provider));
    println!("\n{COMPLETED}{mode}");
}

#[test]
fn cpr_tls_rejects_plaintext_extensions_across_key_change() {
    use std::{io::Cursor, sync::Arc};

    use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection};
    use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName};

    provider_openai::ensure_rustls_provider();
    let identity = super::native_tls::identity();
    let certificate = CertificateDer::from(identity.cert.to_der().unwrap());
    let mut roots = RootCertStore::empty();
    roots.add(certificate.clone()).unwrap();
    let client_config = Arc::new(
        ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let server_config = Arc::new(
        ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate],
                PrivatePkcs8KeyDer::from(identity.key.private_key_to_pkcs8().unwrap()).into(),
            )
            .unwrap(),
    );
    for inject_plaintext in [false, true] {
        let mut client = ClientConnection::new(
            client_config.clone(),
            ServerName::try_from("localhost").unwrap(),
        )
        .unwrap();
        let mut server = ServerConnection::new(server_config.clone()).unwrap();
        let mut flight = Vec::new();
        client.write_tls(&mut flight).unwrap();
        server.read_tls(&mut Cursor::new(flight)).unwrap();
        server.process_new_packets().unwrap();
        let mut flight = Vec::new();
        server.write_tls(&mut flight).unwrap();
        if inject_plaintext {
            assert_eq!(flight[0], 22, "first record is a plaintext handshake");
            assert_eq!(flight[5], 2, "first handshake is ServerHello");
            let length = usize::from(u16::from_be_bytes([flight[3], flight[4]]));
            assert_eq!(
                length,
                4 + usize::from(u16::from_be_bytes([flight[7], flight[8]])),
            );
            flight.truncate(5 + length);
            // RUSTSEC-2026-0285: append a complete plaintext EncryptedExtensions
            // behind ServerHello in the same record, crossing the key boundary.
            flight.extend_from_slice(&[8, 0, 0, 2, 0, 0]);
            flight[3..5].copy_from_slice(&u16::try_from(length + 6).unwrap().to_be_bytes());
        }
        let mut input = Cursor::new(&flight);
        while input.position() < flight.len() as u64 {
            assert!(client.read_tls(&mut input).unwrap() > 0);
            let result = client.process_new_packets();
            if inject_plaintext {
                assert!(matches!(
                    result,
                    Err(rustls::Error::PeerMisbehaved(
                        rustls::PeerMisbehaved::KeyEpochWithPendingFragment
                    )),
                ));
                let mut alert = Vec::new();
                client.write_tls(&mut alert).unwrap();
                assert!(alert.ends_with(&[21, 3, 3, 0, 2, 2, 10]));
            } else {
                result.unwrap();
            }
        }
        if !inject_plaintext {
            assert!(!client.is_handshaking(), "valid server flight must succeed");
        }
    }
}

#[tokio::test]
async fn cpr_and_qx_https_proxy_verify_both_tls_layers() {
    use std::pin::Pin;

    use gateway_core::account::OutboundProxy;
    use openssl::ssl::Ssl;
    use provider_openai::transport::native_http::{NativeHttpClient, NativeHttpConfig};
    use tokio_openssl::SslStream;

    let trusted = super::native_tls::identity();
    let untrusted = super::native_tls::identity();
    for qx in [false, true] {
        for case in ["valid", "untrusted_proxy", "untrusted_origin"] {
            timeout(Duration::from_secs(10), async {
                let proxy_identity = if case == "untrusted_proxy" {
                    &untrusted
                } else {
                    &trusted
                };
                let origin_identity = if case == "untrusted_origin" {
                    &untrusted
                } else {
                    &trusted
                };
                let proxy_acceptor = super::native_tls::acceptor(proxy_identity, false);
                let origin_acceptor = super::native_tls::acceptor(origin_identity, false);
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let proxy_url = format!(
                    "https://user:pass@localhost:{}",
                    listener.local_addr().unwrap().port()
                );
                let server = async {
                    let (stream, _) = listener.accept().await.unwrap();
                    let mut outer =
                        SslStream::new(Ssl::new(proxy_acceptor.context()).unwrap(), stream)
                            .unwrap();
                    let handshake = Pin::new(&mut outer).accept().await;
                    if case == "untrusted_proxy" {
                        assert!(handshake.is_err());
                        return;
                    }
                    handshake.unwrap();
                    let mut headers = Vec::new();
                    while !headers.ends_with(b"\r\n\r\n") {
                        assert!(headers.len() < 8192);
                        headers.push(outer.read_u8().await.unwrap());
                    }
                    let headers = String::from_utf8(headers).unwrap().to_ascii_lowercase();
                    assert!(headers.starts_with("connect localhost:9443 "));
                    assert!(headers.contains("proxy-authorization: basic "));
                    outer
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await
                        .unwrap();
                    let mut inner =
                        SslStream::new(Ssl::new(origin_acceptor.context()).unwrap(), outer)
                            .unwrap();
                    let handshake = Pin::new(&mut inner).accept().await;
                    if case == "untrusted_origin" {
                        assert!(handshake.is_err());
                        return;
                    }
                    handshake.unwrap();
                    let mut headers = Vec::new();
                    while !headers.ends_with(b"\r\n\r\n") {
                        assert!(headers.len() < 8192);
                        headers.push(inner.read_u8().await.unwrap());
                    }
                    let headers = String::from_utf8(headers).unwrap().to_ascii_lowercase();
                    assert!(headers.starts_with("get /probe "));
                    assert!(!headers.contains("proxy-authorization"));
                    inner
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        )
                        .await
                        .unwrap();
                    inner.shutdown().await.unwrap();
                };
                let client = async {
                    let target = "https://localhost:9443/probe";
                    if qx {
                        let client = NativeHttpClient::with_tls(
                            NativeHttpConfig {
                                proxy: Some(OutboundProxy::parse(&proxy_url).unwrap()),
                                ..Default::default()
                            },
                            super::native_tls::connector(&trusted),
                        )
                        .unwrap();
                        let result = client
                            .execute(reqwest::Client::new().get(target).build().unwrap())
                            .await;
                        if case == "valid" {
                            assert_eq!(result.unwrap().text().await.unwrap(), "ok");
                        } else {
                            assert!(result.is_err(), "{case}: QX must reject untrusted TLS");
                        }
                    } else {
                        let client = provider_openai::build_reqwest_client_with_custom_ca(
                            reqwest::Client::builder()
                                .use_rustls_tls()
                                .no_proxy()
                                .proxy(reqwest::Proxy::all(&proxy_url).unwrap())
                                .add_root_certificate(
                                    reqwest::Certificate::from_der(&trusted.cert.to_der().unwrap())
                                        .unwrap(),
                                ),
                        )
                        .unwrap();
                        let result = client.get(target).send().await;
                        if case == "valid" {
                            assert_eq!(result.unwrap().text().await.unwrap(), "ok");
                        } else {
                            assert!(result.is_err(), "{case}: CPR must reject untrusted TLS");
                        }
                    }
                };
                tokio::join!(server, client);
            })
            .await
            .expect("bounded nested proxy TLS verification");
        }
    }
}

#[tokio::test]
#[ignore = "explicit loopback-only TLS upgrade evidence; emits JSON to stdout"]
async fn capture_tls_upgrade_matrix() {
    provider_openai::ensure_rustls_provider();
    println!(
        "TLS_ENV {}",
        json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "openssl": openssl::version::version(),
        })
    );
    for route in [
        "cpr_http",
        "cpr_websocket",
        "qx_http",
        "qx_websocket",
        "cpr_https_proxy",
        "qx_https_proxy",
    ] {
        for sample in 0..10 {
            let capture = timeout(Duration::from_secs(10), capture_route(route))
                .await
                .expect("loopback capture within deadline");
            println!(
                "TLS_CAPTURE {}",
                json!({"route": route, "sample": sample, "capture": capture})
            );
        }
    }
}

async fn capture_route(route: &str) -> CapturedClientHello {
    use gateway_core::account::OutboundProxy;
    use provider_openai::transport::{
        native_http::{NativeHttpClient, NativeHttpConfig},
        native_tls::{NativeAlpn, NativeTlsConnector},
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("https://localhost:{port}/");
    let send = async {
        match route {
            "cpr_http" => {
                let client = provider_openai::transport::build_reqwest_client().unwrap();
                assert!(client.get(&url).send().await.is_err());
            }
            "cpr_websocket" => {
                let connector =
                    provider_openai::transport::tls::maybe_build_rustls_client_config_with_custom_ca()
                        .unwrap()
                        .map(tokio_tungstenite::Connector::Rustls);
                assert!(
                    tokio_tungstenite::connect_async_tls_with_config(
                        format!("wss://localhost:{port}/"),
                        None,
                        false,
                        connector,
                    )
                    .await
                    .is_err()
                );
            }
            "qx_http" | "qx_https_proxy" => {
                let proxy =
                    (route == "qx_https_proxy").then(|| OutboundProxy::parse(&url).unwrap());
                let client = NativeHttpClient::new(NativeHttpConfig {
                    proxy,
                    ..Default::default()
                })
                .unwrap();
                let target = if route == "qx_https_proxy" {
                    "http://origin.invalid/"
                } else {
                    &url
                };
                let request = reqwest::Client::new().get(target).build().unwrap();
                assert!(client.execute(request).await.is_err());
            }
            "qx_websocket" => {
                let connector = NativeTlsConnector::from_environment().unwrap();
                let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .unwrap();
                assert!(
                    connector
                        .connect(stream, "localhost", NativeAlpn::WebSocket)
                        .await
                        .is_err()
                );
            }
            "cpr_https_proxy" => {
                let client = provider_openai::build_reqwest_client_with_custom_ca(
                    reqwest::Client::builder()
                        .use_rustls_tls()
                        .no_proxy()
                        .proxy(reqwest::Proxy::all(&url).unwrap()),
                )
                .unwrap();
                assert!(client.get("http://origin.invalid/").send().await.is_err());
            }
            _ => unreachable!("fixed capture route"),
        }
    };
    let (capture, ()) = tokio::join!(read_client_hello_capture(listener), send);
    capture
}

#[tokio::test]
async fn cpr_tls_http_and_websocket_verify_certificates_and_exchange_data() {
    use std::{pin::Pin, sync::Arc};

    use futures::{SinkExt, StreamExt};
    use openssl::ssl::Ssl;
    use rustls::{ClientConfig, RootCertStore};
    use rustls_pki_types::CertificateDer;
    use tokio_openssl::SslStream;

    provider_openai::ensure_rustls_provider();
    let trusted = super::native_tls::identity();
    let untrusted = super::native_tls::identity();
    for websocket in [false, true] {
        for case in ["valid", "untrusted", "wrong_hostname"] {
            timeout(Duration::from_secs(10), async {
                let identity = if case == "untrusted" { &untrusted } else { &trusted };
                let acceptor = super::native_tls::acceptor(identity, false);
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let host = if case == "wrong_hostname" { "wrong.invalid" } else { "localhost" };
                let valid = case == "valid";
                let server = async {
                    let (stream, _) = listener.accept().await.unwrap();
                    let mut stream = SslStream::new(Ssl::new(acceptor.context()).unwrap(), stream).unwrap();
                    let handshake = Pin::new(&mut stream).accept().await;
                    if !valid {
                        assert!(handshake.is_err(), "{case}: client must reject TLS");
                        return;
                    }
                    handshake.unwrap();
                    if websocket {
                        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                        let message = socket.next().await.unwrap().unwrap();
                        socket.send(message).await.unwrap();
                    } else {
                        let mut headers = Vec::new();
                        while !headers.ends_with(b"\r\n\r\n") {
                            assert!(headers.len() < 8192);
                            headers.push(stream.read_u8().await.unwrap());
                        }
                        assert!(headers.starts_with(b"GET /probe "));
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
                        stream.shutdown().await.unwrap();
                    }
                };
                let client = async {
                    let certificate = trusted.cert.to_der().unwrap();
                    if websocket {
                        let mut roots = RootCertStore::empty();
                        roots.add(CertificateDer::from(certificate)).unwrap();
                        let config = ClientConfig::builder()
                            .with_root_certificates(roots)
                            .with_no_client_auth();
                        let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
                        let result = tokio_tungstenite::client_async_tls_with_config(
                            format!("wss://{host}:{}/probe", address.port()),
                            tcp,
                            None,
                            Some(tokio_tungstenite::Connector::Rustls(Arc::new(config))),
                        ).await;
                        if valid {
                            let (mut socket, response) = result.unwrap();
                            assert_eq!(response.status(), 101);
                            socket.send(tungstenite::Message::Text("probe".into())).await.unwrap();
                            assert_eq!(socket.next().await.unwrap().unwrap().to_text().unwrap(), "probe");
                        } else {
                            assert!(result.is_err(), "{case}: unverified WSS must fail");
                        }
                    } else {
                        let client = provider_openai::build_reqwest_client_with_custom_ca(
                            reqwest::Client::builder()
                                .use_rustls_tls()
                                .no_proxy()
                                .resolve(host, address)
                                .add_root_certificate(reqwest::Certificate::from_der(&certificate).unwrap()),
                        ).unwrap();
                        let result = client.get(format!("https://{host}:{}/probe", address.port())).send().await;
                        if valid {
                            let response = result.unwrap();
                            assert_eq!(response.status(), 200);
                            assert_eq!(response.text().await.unwrap(), "ok");
                        } else {
                            assert!(result.is_err(), "{case}: unverified HTTPS must fail");
                        }
                    }
                };
                tokio::join!(server, client);
            }).await.expect("bounded local TLS/WS certificate contract");
        }
    }
}

#[tokio::test]
async fn http_client_hello_should_match_linked_native_tls_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "https://localhost:{}/",
        listener.local_addr().unwrap().port()
    );
    let client = provider_openai::transport::build_reqwest_client().unwrap();
    let (hello, response) = timeout(Duration::from_secs(10), async {
        tokio::join!(read_client_hello(listener), client.get(url).send())
    })
    .await
    .expect("HTTP ClientHello within timeout");
    assert!(
        response.is_err(),
        "capture endpoint rejects TLS after ClientHello"
    );

    // Compare the actual linked backend on every OS, not a different OpenSSL build's vector.
    let reference_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = reference_listener.local_addr().unwrap();
    let reference = tokio::task::spawn_blocking(move || {
        let stream =
            std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert!(
            ::native_tls::TlsConnector::new()
                .unwrap()
                .connect("localhost", stream)
                .is_err()
        );
    });
    let (expected, result) = timeout(Duration::from_secs(10), async {
        tokio::join!(read_client_hello(reference_listener), reference)
    })
    .await
    .expect("reference native TLS ClientHello within timeout");
    result.unwrap();
    assert_eq!(hello, expected);
    assert!(
        hello.alpn.is_empty(),
        "native HTTP keeps the official no-ALPN policy"
    );
}

#[tokio::test]
async fn legacy_reqwest_helper_keeps_rustls_with_native_tls_enabled() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "https://localhost:{}/",
        listener.local_addr().unwrap().port()
    );
    let client =
        provider_openai::build_reqwest_client_with_custom_ca(reqwest::Client::builder().no_proxy())
            .unwrap();
    let (mut hello, response) = timeout(Duration::from_secs(10), async {
        tokio::join!(read_client_hello(listener), client.get(url).send())
    })
    .await
    .expect("legacy client ClientHello within timeout");
    assert!(response.is_err());
    hello.extensions.sort_unstable();
    let mut expected = official_websocket_hello();
    expected.extensions.insert(5, 16);
    expected.alpn = vec!["h2".to_owned(), "http/1.1".to_owned()];
    assert_eq!(hello, expected);
}

#[tokio::test]
async fn websocket_client_hello_should_match_official_rustls_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("wss://localhost:{}/", listener.local_addr().unwrap().port());
    let connector =
        provider_openai::transport::tls::maybe_build_rustls_client_config_with_custom_ca()
            .unwrap()
            .map(tokio_tungstenite::Connector::Rustls);
    let (mut hello, response) = timeout(Duration::from_secs(10), async {
        tokio::join!(
            read_client_hello(listener),
            tokio_tungstenite::connect_async_tls_with_config(url, None, false, connector)
        )
    })
    .await
    .expect("WebSocket ClientHello within timeout");
    assert!(
        response.is_err(),
        "capture endpoint rejects TLS after ClientHello"
    );
    hello.extensions.sort_unstable();
    assert_eq!(hello, official_websocket_hello());
}

#[tokio::test]
async fn independently_selected_tls_is_used_for_both_ua_formats_and_session_policies() {
    use gateway_core::provider_ports::{
        ProviderSessionPolicy, ProviderTlsProfile, ProviderUserAgentOverride,
    };
    let mut baselines = BTreeMap::new();
    for tls in [ProviderTlsProfile::Cpr, ProviderTlsProfile::QxCompatible] {
        for websocket in [false, true] {
            for user_agent in [
                "Codex Desktop/0.146.0 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.901.1)",
                "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color",
            ] {
                for session in [
                    ProviderSessionPolicy::Native,
                    ProviderSessionPolicy::QxCompatible,
                ] {
                    let profile = test_wire_profile();
                    profile
                        .apply_user_agent_override(&ProviderUserAgentOverride::Independent {
                            user_agent: Some(user_agent.to_owned()),
                            tls_profile: tls,
                            session_policy: session,
                        })
                        .unwrap();
                    assert_eq!(profile.snapshot().user_agent(), user_agent);
                    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let client = CodexBackendClient::new(
                        provider_openai::transport::build_reqwest_client().unwrap(),
                        format!(
                            "https://localhost:{}",
                            listener.local_addr().unwrap().port()
                        ),
                        profile,
                    )
                    .with_websocket_pool(Arc::new(CodexWebSocketPool::new(Duration::from_secs(
                        60,
                    ))));
                    let mut request = CodexResponsesRequest::from_body(codex_request_body(
                        "gpt-test",
                        "test",
                        vec![json!({"role":"user","content":"hello"})],
                    ));
                    if websocket {
                        request = websocket_only_request(request);
                    } else {
                        request.force_http_sse = true;
                    }
                    let mut hello = timeout(Duration::from_secs(10), async {
                        // Stop after the opening is observed; the rejecting fixture is not a
                        // server for the product's subsequent connection-restart attempts.
                        tokio::select! {
                            hello = read_client_hello(listener) => hello,
                            result = client.create_response(
                                &request, request_context("tls-matrix", Some("tls-test-account"))
                            ) => match result {
                                Err(CodexClientError::WebSocket(error)) => panic!(
                                    "request ended before ClientHello: tls={tls:?}, ws={websocket}, {error:?}"
                                ),
                                other => panic!("request ended before ClientHello: {other:?}"),
                            },
                        }
                    })
                    .await
                    .unwrap_or_else(|_| {
                        panic!("ClientHello timeout: tls={tls:?}, websocket={websocket}")
                    });
                    if websocket && tls == ProviderTlsProfile::Cpr {
                        hello.extensions.sort_unstable();
                    }
                    let key = (tls.as_str(), websocket);
                    if let Some(expected) = baselines.get(&key) {
                        assert_eq!(
                            &hello, expected,
                            "UA or session policy changed selected TLS"
                        );
                    } else {
                        baselines.insert(key, hello);
                    }
                }
            }
        }
    }
    assert_ne!(
        baselines[&("cpr", false)],
        baselines[&("qx-compatible", false)]
    );
    assert_ne!(
        baselines[&("cpr", true)],
        baselines[&("qx-compatible", true)]
    );
}

fn official_websocket_hello() -> ClientHello {
    ClientHello {
        cipher_suites: vec![
            4866, 4865, 4867, 49196, 49195, 52393, 49200, 49199, 52392, 255,
        ],
        extensions: vec![0, 5, 10, 11, 13, 23, 35, 43, 45, 51],
        groups: vec![4588, 29, 23, 24],
        signature_algorithms: vec![1283, 1027, 1539, 2055, 2054, 2053, 2052, 1537, 1281, 1025],
        key_shares: vec![(4588, 1216), (29, 32)],
        alpn: Vec::new(),
    }
}

async fn read_client_hello(listener: TcpListener) -> ClientHello {
    let capture = read_client_hello_capture(listener).await;
    // Native/QX assertions retain wire order; only evidence samples normalize it.
    ClientHello {
        extensions: capture.extension_order,
        ..capture.normalized
    }
}

async fn read_client_hello_capture(listener: TcpListener) -> CapturedClientHello {
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut header = [0; 5];
    stream.read_exact(&mut header).await.unwrap();
    assert_eq!(header[0], 22, "TLS handshake record");
    let length = usize::from(u16::from_be_bytes([header[3], header[4]]));
    assert!(length <= 16_384, "bounded ClientHello record");
    let mut record = vec![0; length];
    stream.read_exact(&mut record).await.unwrap();
    stream.write_all(&[21, 3, 3, 0, 2, 2, 40]).await.unwrap();

    let mut hello = Bytes::from(record);
    assert_eq!(hello.get_u8(), 1, "ClientHello message");
    hello.advance(3 + 2 + 32); // Message length, legacy version, random.
    let session_id_len = usize::from(hello.get_u8());
    hello.advance(session_id_len);
    let cipher_suites = u16_values(take_vector(&mut hello));
    let compression_len = usize::from(hello.get_u8());
    hello.advance(compression_len);
    let mut extensions = take_vector(&mut hello);
    let mut extension_order = Vec::new();
    let mut values = BTreeMap::new();
    while extensions.has_remaining() {
        let kind = extensions.get_u16();
        extension_order.push(kind);
        assert!(values.insert(kind, take_vector(&mut extensions)).is_none());
    }
    // Key-share bytes are ephemeral; compare their group and length below.
    let stable_extension_payloads = values
        .iter()
        .filter(|(kind, _)| **kind != 51)
        .map(|(kind, value)| (*kind, hex::encode(value)))
        .collect();

    let groups = values
        .get(&10)
        .map(|value| {
            let mut value = value.clone();
            u16_values(take_vector(&mut value))
        })
        .unwrap_or_default();
    let signature_algorithms = values
        .get(&13)
        .map(|value| {
            let mut value = value.clone();
            u16_values(take_vector(&mut value))
        })
        .unwrap_or_default();
    let mut shares = values.get(&51).cloned().unwrap_or_default();
    let mut shares = if shares.has_remaining() {
        take_vector(&mut shares)
    } else {
        Bytes::new()
    };
    let mut key_shares = Vec::new();
    while shares.has_remaining() {
        let group = shares.get_u16();
        key_shares.push((group, take_vector(&mut shares).len()));
    }
    let mut alpn = Vec::new();
    if let Some(protocols) = values.get(&16) {
        let mut protocols = protocols.clone();
        let mut protocols = take_vector(&mut protocols);
        while protocols.has_remaining() {
            let length = usize::from(protocols.get_u8());
            alpn.push(String::from_utf8(protocols.split_to(length).to_vec()).unwrap());
        }
    }
    CapturedClientHello {
        normalized: ClientHello {
            cipher_suites,
            extensions: values.into_keys().collect(),
            groups,
            signature_algorithms,
            key_shares,
            alpn,
        },
        extension_order,
        stable_extension_payloads,
        record_version: u16::from_be_bytes([header[1], header[2]]),
        record_length: length,
    }
}

fn take_vector(bytes: &mut Bytes) -> Bytes {
    let length = usize::from(bytes.get_u16());
    bytes.split_to(length)
}

fn u16_values(mut bytes: Bytes) -> Vec<u16> {
    let mut values = Vec::new();
    while bytes.has_remaining() {
        values.push(bytes.get_u16());
    }
    values
}
