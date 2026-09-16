use std::{pin::Pin, sync::Arc, time::Duration};

use openssl::{
    asn1::Asn1Time,
    bn::{BigNum, MsbOption},
    hash::MessageDigest,
    pkey::{PKey, Private},
    rsa::Rsa,
    ssl::{AlpnError, Ssl, SslAcceptor, SslMethod, select_next_proto},
    x509::{
        X509, X509NameBuilder,
        extension::{BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName},
    },
};
use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::CertificateDer;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::timeout,
};
use tokio_openssl::SslStream;

pub(super) struct Identity {
    pub cert: X509,
    pub(super) key: PKey<Private>,
}

pub(super) fn identity() -> Identity {
    build_identity(false, None)
}

pub(super) fn ca_identity() -> Identity {
    build_identity(true, None)
}

pub(super) fn identity_signed_by(issuer: &Identity) -> Identity {
    build_identity(false, Some(issuer))
}

fn build_identity(is_ca: bool, issuer: Option<&Identity>) -> Identity {
    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text(
        "CN",
        if is_ca {
            "CPR synthetic root"
        } else {
            "localhost"
        },
    )
    .unwrap();
    let name = name.build();
    let mut cert = X509::builder().unwrap();
    cert.set_version(2).unwrap();
    let mut serial = BigNum::new().unwrap();
    serial.rand(128, MsbOption::MAYBE_ZERO, false).unwrap();
    cert.set_serial_number(&serial.to_asn1_integer().unwrap())
        .unwrap();
    cert.set_subject_name(&name).unwrap();
    cert.set_issuer_name(issuer.map_or(name.as_ref(), |issuer| issuer.cert.subject_name()))
        .unwrap();
    cert.set_pubkey(&key).unwrap();
    cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    cert.set_not_after(&Asn1Time::days_from_now(2).unwrap())
        .unwrap();
    let mut constraints = BasicConstraints::new();
    constraints.critical();
    if is_ca {
        constraints.ca();
    }
    cert.append_extension(constraints.build().unwrap()).unwrap();
    if is_ca {
        cert.append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .build()
                .unwrap(),
        )
        .unwrap();
    } else if issuer.is_some() {
        cert.append_extension(
            KeyUsage::new()
                .critical()
                .digital_signature()
                .key_encipherment()
                .build()
                .unwrap(),
        )
        .unwrap();
        cert.append_extension(ExtendedKeyUsage::new().server_auth().build().unwrap())
            .unwrap();
    }
    let san = SubjectAlternativeName::new()
        .dns("localhost")
        .ip("127.0.0.1")
        .ip("::1")
        .build(&cert.x509v3_context(None, None))
        .unwrap();
    cert.append_extension(san).unwrap();
    cert.sign(
        issuer.map_or(&key, |issuer| &issuer.key),
        MessageDigest::sha256(),
    )
    .unwrap();
    Identity {
        cert: cert.build(),
        key,
    }
}

pub(super) fn connector(identity: &Identity) -> tokio_rustls::TlsConnector {
    provider_openai::ensure_rustls_provider();
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(identity.cert.to_der().unwrap()))
        .unwrap();
    tokio_rustls::TlsConnector::from(Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

pub(super) fn http_client(identity: &Identity, builder: reqwest::ClientBuilder) -> reqwest::Client {
    provider_openai::transport::tls::build_reqwest_native_client_with_custom_ca(
        builder.add_root_certificate(
            reqwest::Certificate::from_der(&identity.cert.to_der().unwrap()).unwrap(),
        ),
    )
    .unwrap()
}

pub(super) fn acceptor(identity: &Identity, h2: bool) -> SslAcceptor {
    let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls_server()).unwrap();
    acceptor.set_private_key(&identity.key).unwrap();
    acceptor.set_certificate(&identity.cert).unwrap();
    if h2 {
        acceptor.set_alpn_select_callback(|_, client| {
            select_next_proto(b"\x02h2\x08http/1.1", client).ok_or(AlpnError::NOACK)
        });
    }
    acceptor.build()
}

pub(super) async fn accept<S>(acceptor: &SslAcceptor, stream: S) -> SslStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut stream = SslStream::new(Ssl::new(acceptor.context()).unwrap(), stream).unwrap();
    Pin::new(&mut stream).accept().await.unwrap();
    stream
}

#[tokio::test]
async fn websocket_rustls_verifies_dns_and_ip_without_http_alpn() {
    let identity = identity();
    let connector = connector(&identity);
    for host in ["localhost", "127.0.0.1", "::1"] {
        let acceptor = acceptor(&identity, true);
        let (client, server) = tokio::io::duplex(65536);
        timeout(Duration::from_secs(5), async {
            let server = async {
                let mut server = accept(&acceptor, server).await;
                let mut request = [0; 4];
                server.read_exact(&mut request).await.unwrap();
                assert_eq!(&request, b"ping");
                server.write_all(b"pong").await.unwrap();
            };
            let client = async {
                let mut client = connector
                    .connect(host.try_into().unwrap(), client)
                    .await
                    .unwrap();
                assert_eq!(client.get_ref().1.alpn_protocol(), None,);
                client.write_all(b"ping").await.unwrap();
                let mut response = [0; 4];
                client.read_exact(&mut response).await.unwrap();
                assert_eq!(&response, b"pong");
            };
            tokio::join!(server, client);
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn websocket_rustls_rejects_untrusted_certificate_and_wrong_hostname() {
    let trusted = identity();
    let untrusted = identity();
    let connector = connector(&trusted);
    for (identity, hostname) in [(&untrusted, "localhost"), (&trusted, "wrong.invalid")] {
        let acceptor = acceptor(identity, false);
        let (client, server) = tokio::io::duplex(65536);
        timeout(Duration::from_secs(5), async {
            let server = async {
                let mut server =
                    SslStream::new(Ssl::new(acceptor.context()).unwrap(), server).unwrap();
                let _ = Pin::new(&mut server).accept().await;
            };
            let client = async {
                let result = connector
                    .connect(hostname.try_into().unwrap(), client)
                    .await;
                assert!(result.is_err());
            };
            tokio::join!(server, client);
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn dropping_websocket_rustls_handshake_releases_stream() {
    let connector = connector(&identity());
    let (client, mut server) = tokio::io::duplex(65536);
    assert!(
        timeout(
            Duration::from_millis(50),
            connector.connect("localhost".try_into().unwrap(), client),
        )
        .await
        .is_err()
    );
    let mut hello = Vec::new();
    timeout(Duration::from_secs(1), server.read_to_end(&mut hello))
        .await
        .unwrap()
        .unwrap();
    assert!(
        !hello.is_empty(),
        "actual handshake started before cancellation"
    );
}

#[tokio::test]
async fn rustls_supports_the_product_websocket_handshake_and_echo() {
    use futures::{SinkExt, StreamExt};
    timeout(Duration::from_secs(5), async {
        let identity = identity();
        let connector = connector(&identity);
        let acceptor = acceptor(&identity, false);
        let (client, server) = tokio::io::duplex(65536);
        let server = async {
            let server = accept(&acceptor, server).await;
            let mut websocket = tokio_tungstenite::accept_async(server).await.unwrap();
            let message = websocket.next().await.unwrap().unwrap();
            websocket.send(message).await.unwrap();
        };
        let client = async {
            let stream = connector
                .connect("localhost".try_into().unwrap(), client)
                .await
                .unwrap();
            assert!(stream.get_ref().1.alpn_protocol().is_none());
            let (mut websocket, response) = tokio_tungstenite::client_async_with_config(
                "wss://localhost/responses",
                stream,
                None,
            )
            .await
            .unwrap();
            assert_eq!(response.status(), 101);
            websocket
                .send(tungstenite::Message::Text("hello".into()))
                .await
                .unwrap();
            assert_eq!(
                websocket.next().await.unwrap().unwrap().to_text().unwrap(),
                "hello"
            );
        };
        tokio::join!(server, client);
    })
    .await
    .unwrap();
}

#[test]
fn websocket_rustls_configuration_rejects_malformed_certificates() {
    let mut roots = RootCertStore::empty();
    assert!(roots.add(CertificateDer::from(vec![1, 2, 3])).is_err());
}
