use std::{pin::Pin, process::Command, time::Duration};

use openssl::ssl::Ssl;
use provider_openai::transport::tls::{
    CODEX_CA_CERT_ENV, CustomCaError, SSL_CERT_FILE_ENV, build_reqwest_native_client_with_custom_ca,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::timeout,
};
use tokio_openssl::SslStream;

use super::native_tls::{acceptor, ca_identity, identity_signed_by};

#[test]
fn native_reqwest_preserves_custom_ca_and_certificate_validation() {
    const CASE_ENV: &str = "CPR_TEST_NATIVE_REQWEST_CA_CASE";
    const BUNDLE_ENV: &str = "CPR_TEST_NATIVE_REQWEST_CA_BUNDLE";
    const COMPLETED: &str = "native-reqwest-ca-completed:";

    if let Ok(case) = std::env::var(CASE_ENV) {
        let bundle = std::env::var(BUNDLE_ENV).unwrap();
        let root_identity = ca_identity();
        let server_identity = identity_signed_by(&root_identity);
        match case.as_str() {
            "missing" => {}
            "invalid" => std::fs::write(&bundle, "not a certificate").unwrap(),
            "untrusted" => {
                std::fs::write(&bundle, ca_identity().cert.to_pem().unwrap()).unwrap();
            }
            _ => {
                std::fs::write(&bundle, root_identity.cert.to_pem().unwrap()).unwrap();
            }
        }

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let hostname = if case == "wrong-host" {
                    "wrong.invalid"
                } else {
                    "localhost"
                };
                let client = build_reqwest_native_client_with_custom_ca(
                    reqwest::Client::builder()
                        .no_proxy()
                        .resolve(hostname, address)
                        .timeout(Duration::from_secs(5)),
                );
                if matches!(case.as_str(), "missing" | "priority") {
                    assert!(matches!(
                        client,
                        Err(CustomCaError::ReadCaFile {
                            source_env: CODEX_CA_CERT_ENV,
                            ..
                        })
                    ));
                    return;
                }
                if case == "invalid" {
                    assert!(matches!(client, Err(CustomCaError::InvalidCaFile { .. })));
                    return;
                }

                let client = client.expect("custom native TLS client builds");
                let acceptor = acceptor(&server_identity, true);
                let rejected = matches!(case.as_str(), "wrong-host" | "untrusted");
                timeout(Duration::from_secs(10), async {
                    let server = async {
                        let (stream, _) = listener.accept().await.unwrap();
                        let mut stream =
                            SslStream::new(Ssl::new(acceptor.context()).unwrap(), stream).unwrap();
                        let result = Pin::new(&mut stream).accept().await;
                        if rejected {
                            assert!(result.is_err(), "invalid peer must fail TLS verification");
                            return;
                        }
                        result.expect("trusted native TLS handshake");
                        assert!(stream.ssl().selected_alpn_protocol().is_none());
                        let mut request = Vec::new();
                        loop {
                            request.push(stream.read_u8().await.unwrap());
                            if request.ends_with(b"\r\n\r\n") {
                                break;
                            }
                            assert!(request.len() < 8192, "bounded HTTP headers");
                        }
                        stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                            )
                            .await
                            .unwrap();
                    };
                    let request = async {
                        let response = client
                            .get(format!("https://{hostname}:{}/", address.port()))
                            .send()
                            .await;
                        if rejected {
                            let error = response.expect_err("reject untrusted or wrong-host peer");
                            assert!(error.is_connect());
                        } else {
                            let response = response.expect("request with custom CA");
                            assert_eq!(response.status(), 200);
                            assert_eq!(response.text().await.unwrap(), "ok");
                        }
                    };
                    tokio::join!(server, request);
                })
                .await
                .expect("native TLS verification finishes within deadline");
            });
        println!("\n{COMPLETED}{case}");
        return;
    }

    for case in [
        "codex",
        "ssl",
        "wrong-host",
        "untrusted",
        "missing",
        "invalid",
        "priority",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("ca.pem");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg("transport::native_reqwest::native_reqwest_preserves_custom_ca_and_certificate_validation")
            .arg("--nocapture")
            .env(CASE_ENV, case)
            .env(BUNDLE_ENV, &bundle)
            .env_remove(CODEX_CA_CERT_ENV)
            .env_remove(SSL_CERT_FILE_ENV);
        if case == "ssl" {
            command.env(SSL_CERT_FILE_ENV, &bundle);
        } else if case == "priority" {
            command
                .env(CODEX_CA_CERT_ENV, directory.path().join("missing.pem"))
                .env(SSL_CERT_FILE_ENV, &bundle);
        } else {
            command.env(CODEX_CA_CERT_ENV, &bundle);
        }
        let output = command.output().expect("isolated native TLS test");
        assert!(
            output.status.success(),
            "case {case} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout
                .lines()
                .filter(|line| *line == format!("{COMPLETED}{case}"))
                .count(),
            1,
            "child test must execute the production assertions: {stdout}"
        );
    }
}
