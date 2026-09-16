use std::{io, pin::Pin, time::Duration};

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
use provider_openai::transport::native_tls::{
    NativeAlpn, NativeTlsConnector, NativeTransportError,
};
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

pub(super) fn connector(identity: &Identity) -> NativeTlsConnector {
    NativeTlsConnector::from_certificates(&[CertificateDer::from(identity.cert.to_der().unwrap())])
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
async fn native_tls_verifies_dns_and_ip_with_protocol_specific_alpn() {
    let identity = identity();
    let connector = connector(&identity);
    for (host, mode) in [
        ("localhost", NativeAlpn::Http),
        ("127.0.0.1", NativeAlpn::Http1),
        ("[::1]", NativeAlpn::WebSocket),
    ] {
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
                let mut client = connector.connect(client, host, mode).await.unwrap();
                assert_eq!(
                    client.ssl().selected_alpn_protocol(),
                    (mode == NativeAlpn::Http).then_some(b"h2".as_slice()),
                );
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
async fn native_tls_rejects_untrusted_certificate_and_wrong_hostname() {
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
                let result = connector.connect(client, hostname, NativeAlpn::Http).await;
                assert!(matches!(result, Err(NativeTransportError::Connect { .. })));
            };
            tokio::join!(server, client);
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn dropping_native_handshake_releases_stream() {
    let connector = connector(&identity());
    let (client, mut server) = tokio::io::duplex(65536);
    assert!(
        timeout(
            Duration::from_millis(50),
            connector.connect(client, "localhost", NativeAlpn::WebSocket),
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
async fn native_tls_supports_the_product_websocket_handshake_and_echo() {
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
                .connect(client, "localhost", NativeAlpn::WebSocket)
                .await
                .unwrap();
            assert!(stream.ssl().selected_alpn_protocol().is_none());
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
fn native_tls_configuration_fails_closed_and_error_formatting_is_redacted() {
    assert!(matches!(
        NativeTlsConnector::from_certificates(&[]),
        Err(NativeTransportError::Configuration { .. })
    ));
    assert!(matches!(
        NativeTlsConnector::from_certificates(&[CertificateDer::from(vec![1, 2, 3])]),
        Err(NativeTransportError::Configuration { .. })
    ));
    let error = NativeTransportError::Connect {
        source: Box::new(io::Error::other("https://secret:password@proxy/token")),
        timeout: false,
    };
    assert!(error.is_connect());
    assert!(!error.is_configuration());
    for output in [error.to_string(), format!("{error:?}")] {
        assert!(!output.contains("secret"));
        assert!(!output.contains("password"));
        assert!(!output.contains("/token"));
    }
}
