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
