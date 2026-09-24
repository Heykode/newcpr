use chrono::{DateTime, Utc};
use gateway_core::routing::AccountGroupId;
use secrecy::SecretString;

/// Shared by configuration writes and delivery of previously saved keys.
#[must_use]
pub fn normalized_bark_device_key(value: &str) -> Option<&str> {
    let key = value.trim().trim_end_matches('/');
    (!key.is_empty()
        && key.len() <= 256
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    .then_some(key)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpSecurity {
    None,
    StartTls,
    Tls,
}

impl SmtpSecurity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::StartTls => "starttls",
            Self::Tls => "tls",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "starttls" => Some(Self::StartTls),
            "tls" => Some(Self::Tls),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarkLevel {
    Passive,
    Active,
    TimeSensitive,
    Critical,
}

impl BarkLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passive => "passive",
            Self::Active => "active",
            Self::TimeSensitive => "timeSensitive",
            Self::Critical => "critical",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "passive" => Some(Self::Passive),
            "active" => Some(Self::Active),
            "timeSensitive" => Some(Self::TimeSensitive),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpChannelView {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: Option<String>,
    pub password_set: bool,
    pub from_name: Option<String>,
    pub from_email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarkChannelView {
    pub enabled: bool,
    pub server_url: String,
    pub device_key_set: bool,
    pub level: BarkLevel,
    pub sound: Option<String>,
    pub volume: u8,
    pub call: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationChannelsView {
    pub smtp: SmtpChannelView,
    pub bark: BarkChannelView,
    pub last_test: Option<NotificationDeliveryRecord>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct StoredSmtpChannel {
    pub view: SmtpChannelView,
    pub password: Option<SecretString>,
}

impl std::fmt::Debug for StoredSmtpChannel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredSmtpChannel")
            .field("view", &self.view)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Clone)]
pub struct StoredBarkChannel {
    pub view: BarkChannelView,
    pub device_key: Option<SecretString>,
}

impl std::fmt::Debug for StoredBarkChannel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredBarkChannel")
            .field("view", &self.view)
            .field(
                "device_key",
                &self.device_key.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct StoredNotificationChannels {
    pub smtp: StoredSmtpChannel,
    pub bark: StoredBarkChannel,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct ReplaceNotificationChannels {
    pub smtp: StoredSmtpChannel,
    pub bark: StoredBarkChannel,
}

impl std::fmt::Debug for ReplaceNotificationChannels {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReplaceNotificationChannels")
            .field("smtp", &self.smtp)
            .field("bark", &self.bark)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertConditionKind {
    Concurrency,
    Eta,
    QuotaZero,
    Availability,
}

impl AlertConditionKind {
    pub const ALL: [Self; 4] = [
        Self::Concurrency,
        Self::Eta,
        Self::QuotaZero,
        Self::Availability,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Concurrency => "concurrency",
            Self::Eta => "eta",
            Self::QuotaZero => "quota_zero",
            Self::Availability => "availability",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlertConditionPolicy {
    pub enabled: bool,
    pub threshold: f64,
    pub confirmation_seconds: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupAlertPolicy {
    pub group_id: AccountGroupId,
    pub enabled: bool,
    pub concurrency: AlertConditionPolicy,
    pub eta: AlertConditionPolicy,
    pub quota_zero: AlertConditionPolicy,
    pub availability: AlertConditionPolicy,
    pub email_enabled: bool,
    pub email_recipients: Vec<String>,
    pub bark_enabled: bool,
    pub bark_level: Option<BarkLevel>,
    pub bark_sound: Option<String>,
    pub bark_volume: Option<u8>,
    pub bark_call: Option<bool>,
    pub updated_at: DateTime<Utc>,
}

impl GroupAlertPolicy {
    #[must_use]
    pub fn defaults(group_id: AccountGroupId, now: DateTime<Utc>) -> Self {
        Self {
            group_id,
            enabled: false,
            concurrency: AlertConditionPolicy {
                enabled: true,
                threshold: 90.0,
                confirmation_seconds: 20,
            },
            eta: AlertConditionPolicy {
                enabled: true,
                threshold: 10.0,
                confirmation_seconds: 30,
            },
            quota_zero: AlertConditionPolicy {
                enabled: true,
                threshold: 0.0,
                confirmation_seconds: 0,
            },
            availability: AlertConditionPolicy {
                enabled: true,
                threshold: 0.0,
                confirmation_seconds: 0,
            },
            email_enabled: false,
            email_recipients: Vec::new(),
            bark_enabled: false,
            bark_level: None,
            bark_sound: None,
            bark_volume: None,
            bark_call: None,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlertObservation {
    pub group_id: AccountGroupId,
    pub group_name: String,
    pub kind: AlertConditionKind,
    pub active: bool,
    pub current_value: Option<f64>,
    pub threshold: f64,
    pub confirmation_seconds: u32,
    pub email_recipients: Vec<String>,
    pub bark_enabled: bool,
    pub bark_level: BarkLevel,
    pub bark_sound: Option<String>,
    pub bark_volume: u8,
    pub bark_call: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationChannelKind {
    Email,
    Bark,
}

impl NotificationChannelKind {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Bark => "bark",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedNotificationDelivery {
    pub id: String,
    pub group_id: Option<AccountGroupId>,
    pub condition: Option<AlertConditionKind>,
    pub channel: NotificationChannelKind,
    pub target: String,
    pub subject: String,
    pub body: String,
    pub bark_level: BarkLevel,
    pub bark_sound: Option<String>,
    pub bark_volume: u8,
    pub bark_call: bool,
    pub test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationDeliveryRecord {
    pub id: String,
    pub group_id: Option<String>,
    pub channel: String,
    pub target: String,
    pub status: String,
    pub test: bool,
    pub attempts: u32,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestNotificationCommand {
    pub channel: NotificationChannelKind,
    pub target: String,
    pub group_id: Option<AccountGroupId>,
}
