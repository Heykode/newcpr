use async_trait::async_trait;
use secrecy::SecretString;

use crate::model::notifications::{BarkLevel, SmtpSecurity};

/// Only fixed descriptions and numeric codes cross the transport boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotificationDeliveryError {
    #[error("通知传输未启用")]
    Disabled,
    #[error("通知传输初始化失败")]
    Initialization,
    #[error("SMTP 配置不合法，请检查地址、端口和登录凭据")]
    SmtpConfiguration,
    #[error("SMTP 认证被拒绝（{0}），请检查登录账号和密码 / 授权码")]
    SmtpAuthentication(u16),
    #[error("SMTP 连接或发送超时，请检查网络和端口")]
    SmtpTimeout,
    #[error("SMTP 加密连接失败，请检查加密方式、端口和证书")]
    SmtpTls,
    #[error("SMTP 连接失败，请检查服务器地址、端口和网络")]
    SmtpConnection,
    #[error("SMTP 服务器拒绝发送（{0}），请检查发件人、收件人及服务器限制")]
    SmtpRejected(u16),
    #[error("SMTP 协议交互失败，请检查端口、加密方式及服务器认证要求")]
    SmtpProtocol,
    #[error("Bark 配置不合法，请检查服务地址")]
    BarkConfiguration,
    #[error("Bark Device Key 格式不正确，只填写设备密钥，不要填写完整链接")]
    BarkDeviceKey,
    #[error("Bark 请求超时，请检查服务地址和网络")]
    BarkTimeout,
    #[error("Bark 连接失败，请检查服务地址、证书和网络")]
    BarkConnection,
    #[error("Bark 服务返回 HTTP {0}，请检查服务地址和 Device Key")]
    BarkHttp(u16),
    #[error("Bark 返回内容不合法或过大，请检查服务地址")]
    BarkResponse,
    #[error("Bark 拒绝通知（{0}），请检查 Device Key 是否有效")]
    BarkRejected(i64),
}

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
        Err(NotificationDeliveryError::Disabled)
    }

    async fn send_bark(&self, _: BarkMessage) -> Result<(), NotificationDeliveryError> {
        Err(NotificationDeliveryError::Disabled)
    }
}
