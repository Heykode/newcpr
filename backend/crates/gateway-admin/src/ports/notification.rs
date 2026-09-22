use async_trait::async_trait;
use secrecy::SecretString;

use crate::model::notifications::{BarkLevel, SmtpSecurity};

#[derive(Debug, thiserror::Error)]
#[error("notification delivery failed")]
pub struct NotificationDeliveryError;

pub struct SmtpMessage {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: Option<String>,
    pub password: Option<SecretString>,
    pub from_name: Option<String>,
    pub from_email: String,
    pub recipient: String,
    pub subject: String,
    pub html_body: String,
}

pub struct BarkMessage {
    pub server_url: String,
    pub device_key: SecretString,
    pub title: String,
    pub body: String,
    pub group: String,
    pub level: BarkLevel,
    pub sound: Option<String>,
    pub volume: u8,
    pub call: bool,
}

#[async_trait]
pub trait NotificationDelivery: Send + Sync {
    async fn send_smtp(&self, message: SmtpMessage) -> Result<(), NotificationDeliveryError>;
    async fn send_bark(&self, message: BarkMessage) -> Result<(), NotificationDeliveryError>;
}

pub struct DisabledNotificationDelivery;

#[async_trait]
impl NotificationDelivery for DisabledNotificationDelivery {
    async fn send_smtp(&self, _: SmtpMessage) -> Result<(), NotificationDeliveryError> {
        Err(NotificationDeliveryError)
    }

    async fn send_bark(&self, _: BarkMessage) -> Result<(), NotificationDeliveryError> {
        Err(NotificationDeliveryError)
    }
}
