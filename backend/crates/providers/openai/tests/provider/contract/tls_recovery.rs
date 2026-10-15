use super::*;

#[tokio::test]
async fn refused_http_connection_requests_bounded_same_account_recovery() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let mut stream = provider_with_base_url(&store, base)
        .execute(
            planned_request("openai", http_generate_operation()),
            context("req_connection_refused", CancellationToken::new()),
        )
        .await
        .unwrap();
    let (_, error) = timeout(
        Duration::from_secs(5),
        super::precommit::stream_failure(&mut stream),
    )
    .await
    .unwrap();
    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
    assert!(error.replay_is_safe());
    assert!(!error.retry_is_prohibited());
    assert!(matches!(
        error.pre_delivery_retry(),
        Some(PreDeliveryRetry::SameAccountConnectionRetry {
            transport: AttemptTransport::Fallback
        })
    ));
}

fn isolated_native_tls() -> bool {
    const CHILD: &str = "CPR_TEST_TLS_RECOVERY_CHILD";
    if std::env::var_os(CHILD).is_some() {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", std::thread::current().name().unwrap()])
        .env(CHILD, "1")
        .env_remove(provider_openai::transport::tls::CODEX_CA_CERT_ENV)
        .env_remove(provider_openai::transport::tls::SSL_CERT_FILE_ENV)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    true
}

async fn handshake_failure(alert: bool) -> gateway_core::error::ProviderError {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut header = [0_u8; 5];
        socket.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0], 22);
        let mut hello = vec![0; usize::from(u16::from_be_bytes([header[3], header[4]]))];
        socket.read_exact(&mut hello).await.unwrap();
        if alert {
            socket.write_all(&[21, 3, 3, 0, 2, 2, 40]).await.unwrap();
        }
        socket.shutdown().await.unwrap();
    });
    let mut stream = provider_with_base_url(&store, base)
        .execute(
            planned_request("openai", http_generate_operation()),
            context("req_tls_recovery", CancellationToken::new()),
        )
        .await
        .unwrap();
    let (_, error) = timeout(
        Duration::from_secs(5),
        super::precommit::stream_failure(&mut stream),
    )
    .await
    .unwrap();
    server.await.unwrap();
    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
    error
}

#[tokio::test]
async fn handshake_alert_never_requests_recovery() {
    if isolated_native_tls() {
        return;
    }
    let error = handshake_failure(true).await;
    assert!(error.retry_is_prohibited());
    assert!(error.pre_delivery_retry().is_none());
}

#[tokio::test]
async fn untrusted_certificate_never_requests_recovery() {
    if isolated_native_tls() {
        return;
    }
    use openssl::{
        asn1::Asn1Time,
        hash::MessageDigest,
        pkey::PKey,
        rsa::Rsa,
        ssl::{SslAcceptor, SslMethod},
        x509::{X509, X509NameBuilder},
    };
    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "localhost").unwrap();
    let name = name.build();
    let mut certificate = X509::builder().unwrap();
    certificate.set_version(2).unwrap();
    certificate.set_subject_name(&name).unwrap();
    certificate.set_issuer_name(&name).unwrap();
    certificate.set_pubkey(&key).unwrap();
    certificate
        .set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    certificate
        .set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    certificate.sign(&key, MessageDigest::sha256()).unwrap();
    let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
    acceptor.set_private_key(&key).unwrap();
    acceptor.set_certificate(&certificate.build()).unwrap();
    let acceptor = acceptor.build();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let listener = TcpListener::from_std(listener).unwrap();
    let base = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let server = tokio::spawn(async move {
        let (socket, _) = timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut stream = tokio_openssl::SslStream::new(
            openssl::ssl::Ssl::new(acceptor.context()).unwrap(),
            socket,
        )
        .unwrap();
        assert!(
            timeout(
                Duration::from_secs(5),
                std::pin::Pin::new(&mut stream).accept()
            )
            .await
            .unwrap()
            .is_err()
        );
    });
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let mut stream = provider_with_base_url(&store, base)
        .execute(
            planned_request("openai", http_generate_operation()),
            context("req_tls_certificate", CancellationToken::new()),
        )
        .await
        .unwrap();
    let (_, error) = timeout(
        Duration::from_secs(5),
        super::precommit::stream_failure(&mut stream),
    )
    .await
    .unwrap();
    server.await.unwrap();
    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
    assert!(error.retry_is_prohibited());
    assert!(error.pre_delivery_retry().is_none());
}

// The native TLS backend differs on macOS; this exception specifically targets
// the production Linux OpenSSL EOF, not arbitrary platform handshake errors.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn native_handshake_eof_is_replay_safe_and_bounded() {
    if isolated_native_tls() {
        return;
    }
    let error = handshake_failure(false).await;
    assert!(error.replay_is_safe());
    assert!(!error.retry_is_prohibited());
    assert!(matches!(
        error.pre_delivery_retry(),
        Some(PreDeliveryRetry::SameAccountConnectionRetry {
            transport: AttemptTransport::Fallback
        })
    ));
}
