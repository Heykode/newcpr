use std::{sync::Arc, time::Duration};

use gateway_admin::{
    model::notifications::{BarkLevel, SmtpSecurity},
    ports::notification::{
        BarkMessage, NotificationDelivery, NotificationDeliveryError, SmtpMessage,
    },
};
use gateway_host::{
    HostConfig,
    config::{FileLoggingConfig, ListenConfig, LoggingConfig},
    system_update::SystemUpdateConfig,
};
use secrecy::SecretString;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

async fn delivery() -> Arc<dyn NotificationDelivery> {
    static DELIVERY: tokio::sync::OnceCell<Arc<dyn NotificationDelivery>> =
        tokio::sync::OnceCell::const_new();
    DELIVERY
        .get_or_init(|| async {
            let bundle = gateway_host::initialize(HostConfig {
                listen: ListenConfig {
                    host: "127.0.0.1".to_owned(),
                    port: 8080,
                },
                runtime_data_dir: std::env::temp_dir(),
                logging: LoggingConfig {
                    level: "off".to_owned(),
                    stdout: true,
                    file: FileLoggingConfig {
                        enabled: false,
                        directory: std::env::temp_dir(),
                        retention_days: 1,
                        max_file_size_mb: 1,
                    },
                    oauth_recovery: false,
                    request_dump: false,
                    request_dump_retention_days: 1,
                },
                system_update: SystemUpdateConfig::default(),
                drain_timeout_seconds: 1,
                worker_shutdown_timeout_seconds: 1,
            })
            .await
            .unwrap();
            bundle.notification_delivery().unwrap()
        })
        .await
        .clone()
}

#[tokio::test]
async fn bark_delivery_posts_expected_payload_and_checks_response_code() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/push"))
        .and(body_json(serde_json::json!({
            "device_key": "synthetic-device",
            "title": "Synthetic alert",
            "body": "Synthetic body",
            "group": "CPR/Synthetic",
            "level": "critical",
            "sound": "alarm",
            "volume": 8,
            "call": 1
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code": 200})))
        .expect(1)
        .mount(&server)
        .await;
    delivery()
        .await
        .send_bark(BarkMessage {
            server_url: format!("{}/push", server.uri()),
            device_key: SecretString::from(" synthetic-device/ \n"),
            title: "Synthetic alert".to_owned(),
            body: "Synthetic body".to_owned(),
            group: "CPR/Synthetic".to_owned(),
            level: BarkLevel::Critical,
            sound: Some("alarm".to_owned()),
            volume: 8,
            call: true,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn bark_error_and_invalid_smtp_address_fail_closed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code": 500})))
        .mount(&server)
        .await;
    let delivery = delivery().await;
    assert!(
        delivery
            .send_bark(BarkMessage {
                server_url: server.uri(),
                device_key: SecretString::from("synthetic-device"),
                title: "test".to_owned(),
                body: "test".to_owned(),
                group: "CPR".to_owned(),
                level: BarkLevel::Active,
                sound: None,
                volume: 5,
                call: false,
            })
            .await
            .is_err()
    );
    assert!(
        delivery
            .send_smtp(SmtpMessage {
                host: "smtp.example.com".to_owned(),
                port: 587,
                security: SmtpSecurity::StartTls,
                username: None,
                password: None,
                from_name: None,
                from_email: "invalid".to_owned(),
                recipient: "ops@example.com".to_owned(),
                subject: "test".to_owned(),
                html_body: "test".to_owned(),
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn smtp_delivers_to_a_local_synthetic_server() {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        write.write_all(b"220 localhost test\r\n").await.unwrap();
        let mut data = false;
        let mut received = String::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            if data && line != "." {
                received.push_str(&line);
                continue;
            }
            let reply = if line.starts_with("EHLO") {
                "250 localhost\r\n"
            } else if line == "DATA" {
                data = true;
                "354 continue\r\n"
            } else if data && line == "." {
                write.write_all(b"250 accepted\r\n").await.unwrap();
                return received;
            } else {
                "250 ok\r\n"
            };
            write.write_all(reply.as_bytes()).await.unwrap();
        }
        received
    });
    delivery()
        .await
        .send_smtp(SmtpMessage {
            host: "127.0.0.1".to_owned(),
            port,
            security: SmtpSecurity::None,
            username: None,
            password: None,
            from_name: Some("CPR".to_owned()),
            from_email: "alerts@example.com".to_owned(),
            recipient: "ops@example.com".to_owned(),
            subject: "Synthetic SMTP test".to_owned(),
            html_body: "<p>synthetic delivery</p>".to_owned(),
        })
        .await
        .unwrap();
    let message = tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    assert!(message.contains("Subject: Synthetic SMTP test"));
    assert!(message.contains("synthetic delivery"));
}

fn bark(server_url: String, key: &str) -> BarkMessage {
    BarkMessage {
        server_url,
        device_key: SecretString::from(key.to_owned()),
        title: "test".to_owned(),
        body: "test".to_owned(),
        group: "CPR".to_owned(),
        level: BarkLevel::Active,
        sound: None,
        volume: 5,
        call: false,
    }
}

#[tokio::test]
async fn bark_diagnostics_are_bounded_and_never_echo_keys_or_upstream_text() {
    let server = MockServer::start().await;
    let sender = delivery().await;
    for (response, expected) in [
        (
            ResponseTemplate::new(401).set_body_string("synthetic-private-value"),
            NotificationDeliveryError::BarkHttp(401),
        ),
        (
            ResponseTemplate::new(302).insert_header("location", "https://push.example.com"),
            NotificationDeliveryError::BarkHttp(302),
        ),
        (
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"code": 400, "message": "synthetic-private-value"}),
            ),
            NotificationDeliveryError::BarkRejected(400),
        ),
        (
            ResponseTemplate::new(200).set_body_string("x".repeat(16_385)),
            NotificationDeliveryError::BarkResponse,
        ),
        (
            ResponseTemplate::new(200).set_body_string("not-json"),
            NotificationDeliveryError::BarkResponse,
        ),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let error = sender
            .send_bark(bark(server.uri(), "synthetic-device"))
            .await
            .unwrap_err();
        assert_eq!(error, expected);
        assert!(!error.to_string().contains("synthetic"));
        assert!(!format!("{error:?}").contains("synthetic"));
    }
    server.reset().await;
    for key in [
        "https://push.example.com/key",
        "key/path",
        "key\nbad",
        "/",
        "key?query",
    ] {
        assert_eq!(
            sender.send_bark(bark(server.uri(), key)).await.unwrap_err(),
            NotificationDeliveryError::BarkDeviceKey
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn smtp_authentication_failure_preserves_only_numeric_code() {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        write.write_all(b"220 localhost test\r\n").await.unwrap();
        while let Some(line) = lines.next_line().await.unwrap() {
            if line.starts_with("AUTH") {
                write
                    .write_all(b"535 5.7.8 rejected synthetic-private-value\r\n")
                    .await
                    .unwrap();
                return;
            }
            write
                .write_all(b"250-localhost\r\n250 AUTH PLAIN LOGIN\r\n")
                .await
                .unwrap();
        }
    });
    let error = delivery()
        .await
        .send_smtp(SmtpMessage {
            host: "127.0.0.1".to_owned(),
            port,
            security: SmtpSecurity::None,
            username: Some("login@example.com".to_owned()),
            password: Some(SecretString::from("synthetic-password")),
            from_name: None,
            from_email: "sender@example.com".to_owned(),
            recipient: "ops@example.com".to_owned(),
            subject: "test".to_owned(),
            html_body: "test".to_owned(),
        })
        .await
        .unwrap_err();
    assert_eq!(error, NotificationDeliveryError::SmtpAuthentication(535));
    assert!(error.to_string().contains("535"));
    assert!(!error.to_string().contains("synthetic"));
    assert!(!format!("{error:?}").contains("example.com"));
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}
