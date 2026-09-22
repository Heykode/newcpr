use std::time::Duration;

use async_trait::async_trait;
use gateway_admin::ports::notification::{
    BarkMessage, NotificationDelivery, NotificationDeliveryError, SmtpMessage,
};
use lettre::{
    AsyncSmtpTransport, AsyncTransport as _, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::{authentication::Credentials, client::Tls},
};
use secrecy::ExposeSecret as _;
use serde_json::json;

use gateway_admin::model::notifications::SmtpSecurity;

pub struct HostNotificationDelivery {
    client: reqwest::Client,
}

impl HostNotificationDelivery {
    pub fn new() -> Result<Self, NotificationDeliveryError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| NotificationDeliveryError)?;
        Ok(Self { client })
    }
}

#[async_trait]
impl NotificationDelivery for HostNotificationDelivery {
    async fn send_smtp(&self, message: SmtpMessage) -> Result<(), NotificationDeliveryError> {
        let from_address = message
            .from_email
            .parse()
            .map_err(|_| NotificationDeliveryError)?;
        let from = Mailbox::new(message.from_name, from_address);
        let to = message
            .recipient
            .parse()
            .map_err(|_| NotificationDeliveryError)?;
        let email = Message::builder()
            .from(from)
            .to(to)
            .subject(message.subject)
            .header(ContentType::TEXT_HTML)
            .body(message.html_body)
            .map_err(|_| NotificationDeliveryError)?;
        let mut builder = match message.security {
            SmtpSecurity::None => {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&message.host)
            }
            SmtpSecurity::StartTls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&message.host)
                    .map_err(|_| NotificationDeliveryError)?
            }
            SmtpSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&message.host)
                .map_err(|_| NotificationDeliveryError)?,
        }
        .port(message.port)
        .timeout(Some(Duration::from_secs(10)));
        if message.security == SmtpSecurity::None {
            builder = builder.tls(Tls::None);
        }
        if let Some(username) = message.username {
            let password = message.password.ok_or(NotificationDeliveryError)?;
            builder = builder.credentials(Credentials::new(
                username,
                password.expose_secret().to_owned(),
            ));
        }
        tokio::time::timeout(Duration::from_secs(20), builder.build().send(email))
            .await
            .map_err(|_| NotificationDeliveryError)?
            .map_err(|_| NotificationDeliveryError)?;
        Ok(())
    }

    async fn send_bark(&self, message: BarkMessage) -> Result<(), NotificationDeliveryError> {
        let mut url =
            reqwest::Url::parse(&message.server_url).map_err(|_| NotificationDeliveryError)?;
        let path = url.path().trim_end_matches('/');
        if !path.ends_with("/push") {
            url.set_path(&format!("{path}/push"));
        }
        let mut response = self
            .client
            .post(url)
            .json(&json!({
                "device_key": message.device_key.expose_secret(),
                "title": message.title,
                "body": message.body,
                "group": message.group,
                "level": message.level.as_str(),
                "sound": message.sound,
                "volume": message.volume,
                "call": if message.call { 1 } else { 0 },
            }))
            .send()
            .await
            .map_err(|_| NotificationDeliveryError)?;
        if !response.status().is_success() {
            return Err(NotificationDeliveryError);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| NotificationDeliveryError)?
        {
            if body.len() + chunk.len() > 16_384 {
                return Err(NotificationDeliveryError);
            }
            body.extend_from_slice(&chunk);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| NotificationDeliveryError)?;
        if value.get("code").and_then(serde_json::Value::as_i64) != Some(200) {
            return Err(NotificationDeliveryError);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_admin::model::notifications::BarkLevel;
    use secrecy::SecretString;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, method, path},
    };

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
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"code": 200})),
            )
            .expect(1)
            .mount(&server)
            .await;
        HostNotificationDelivery::new()
            .unwrap()
            .send_bark(BarkMessage {
                server_url: format!("{}/push", server.uri()),
                device_key: SecretString::from("synthetic-device"),
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
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"code": 500})),
            )
            .mount(&server)
            .await;
        let delivery = HostNotificationDelivery::new().unwrap();
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
        HostNotificationDelivery::new()
            .unwrap()
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
}
