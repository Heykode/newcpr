use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use gateway_admin::model::notifications::{
    AlertConditionPolicy, BarkChannelView, BarkLevel, GroupAlertPolicy, NotificationChannelKind,
    NotificationChannelsView, ReplaceNotificationChannels, SmtpChannelView, SmtpSecurity,
    StoredBarkChannel, StoredSmtpChannel, TestNotificationCommand,
};
use gateway_core::routing::AccountGroupId;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};

use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminQuery, AdminResponse, AdminSessionState,
    wire::map_admin_service_error,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelsView {
    smtp: SmtpView,
    bark: BarkView,
    last_test: Option<DeliveryView>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SmtpView {
    enabled: bool,
    host: String,
    port: u16,
    security: String,
    username: Option<String>,
    password_set: bool,
    from_name: Option<String>,
    from_email: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BarkView {
    enabled: bool,
    server_url: String,
    device_key_set: bool,
    level: String,
    sound: Option<String>,
    volume: u8,
    call: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryView {
    id: String,
    group_id: Option<String>,
    channel: String,
    target: String,
    status: String,
    test: bool,
    attempts: u32,
    error: Option<String>,
    created_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

impl From<gateway_admin::model::notifications::NotificationDeliveryRecord> for DeliveryView {
    fn from(value: gateway_admin::model::notifications::NotificationDeliveryRecord) -> Self {
        Self {
            id: value.id,
            group_id: value.group_id,
            channel: value.channel,
            target: value.target,
            status: value.status,
            test: value.test,
            attempts: value.attempts,
            error: value.error,
            created_at: value.created_at,
            finished_at: value.finished_at,
        }
    }
}

impl From<NotificationChannelsView> for ChannelsView {
    fn from(value: NotificationChannelsView) -> Self {
        Self {
            smtp: SmtpView {
                enabled: value.smtp.enabled,
                host: value.smtp.host,
                port: value.smtp.port,
                security: value.smtp.security.as_str().to_owned(),
                username: value.smtp.username,
                password_set: value.smtp.password_set,
                from_name: value.smtp.from_name,
                from_email: value.smtp.from_email,
            },
            bark: BarkView {
                enabled: value.bark.enabled,
                server_url: value.bark.server_url,
                device_key_set: value.bark.device_key_set,
                level: value.bark.level.as_str().to_owned(),
                sound: value.bark.sound,
                volume: value.bark.volume,
                call: value.bark.call,
            },
            last_test: value.last_test.map(Into::into),
            updated_at: value.updated_at,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaceChannelsRequest {
    smtp: ReplaceSmtpRequest,
    bark: ReplaceBarkRequest,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaceSmtpRequest {
    enabled: bool,
    host: String,
    port: u16,
    security: String,
    username: Option<String>,
    password: Option<String>,
    password_set: bool,
    from_name: Option<String>,
    from_email: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaceBarkRequest {
    enabled: bool,
    server_url: String,
    device_key: Option<String>,
    device_key_set: bool,
    level: String,
    sound: Option<String>,
    volume: u8,
    call: bool,
}

impl ReplaceChannelsRequest {
    fn command(self) -> Result<ReplaceNotificationChannels, AdminError> {
        Ok(ReplaceNotificationChannels {
            smtp: StoredSmtpChannel {
                view: SmtpChannelView {
                    enabled: self.smtp.enabled,
                    host: clean(self.smtp.host),
                    port: self.smtp.port,
                    security: SmtpSecurity::parse(&self.smtp.security)
                        .ok_or_else(|| AdminError::bad_request("SMTP 安全模式不合法"))?,
                    username: optional(self.smtp.username),
                    password_set: self.smtp.password_set
                        || self
                            .smtp
                            .password
                            .as_deref()
                            .is_some_and(|value| !value.is_empty()),
                    from_name: optional(self.smtp.from_name),
                    from_email: optional(self.smtp.from_email),
                },
                password: self
                    .smtp
                    .password
                    .filter(|value| !value.is_empty())
                    .map(SecretString::from),
            },
            bark: StoredBarkChannel {
                view: BarkChannelView {
                    enabled: self.bark.enabled,
                    server_url: clean(self.bark.server_url),
                    device_key_set: self.bark.device_key_set
                        || self
                            .bark
                            .device_key
                            .as_deref()
                            .is_some_and(|value| !value.is_empty()),
                    level: BarkLevel::parse(&self.bark.level)
                        .ok_or_else(|| AdminError::bad_request("Bark 提醒等级不合法"))?,
                    sound: optional(self.bark.sound),
                    volume: self.bark.volume,
                    call: self.bark.call,
                },
                device_key: self
                    .bark
                    .device_key
                    .filter(|value| !value.is_empty())
                    .map(SecretString::from),
            },
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GroupQuery {
    group_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeliveriesQuery {
    #[serde(default)]
    group_id: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PolicyView {
    group_id: String,
    enabled: bool,
    concurrency: ConditionView,
    eta: ConditionView,
    quota_zero: ConditionView,
    availability: ConditionView,
    email_enabled: bool,
    email_recipients: Vec<String>,
    bark_enabled: bool,
    bark_level: Option<String>,
    bark_sound: Option<String>,
    bark_volume: Option<u8>,
    bark_call: Option<bool>,
    updated_at: DateTime<Utc>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConditionView {
    enabled: bool,
    threshold: f64,
    confirmation_seconds: u32,
}
impl From<AlertConditionPolicy> for ConditionView {
    fn from(value: AlertConditionPolicy) -> Self {
        Self {
            enabled: value.enabled,
            threshold: value.threshold,
            confirmation_seconds: value.confirmation_seconds,
        }
    }
}
impl From<ConditionView> for AlertConditionPolicy {
    fn from(value: ConditionView) -> Self {
        Self {
            enabled: value.enabled,
            threshold: value.threshold,
            confirmation_seconds: value.confirmation_seconds,
        }
    }
}
impl From<GroupAlertPolicy> for PolicyView {
    fn from(value: GroupAlertPolicy) -> Self {
        Self {
            group_id: value.group_id.to_string(),
            enabled: value.enabled,
            concurrency: value.concurrency.into(),
            eta: value.eta.into(),
            quota_zero: value.quota_zero.into(),
            availability: value.availability.into(),
            email_enabled: value.email_enabled,
            email_recipients: value.email_recipients,
            bark_enabled: value.bark_enabled,
            bark_level: value.bark_level.map(|v| v.as_str().to_owned()),
            bark_sound: value.bark_sound,
            bark_volume: value.bark_volume,
            bark_call: value.bark_call,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplacePolicyRequest {
    group_id: String,
    enabled: bool,
    concurrency: ConditionView,
    eta: ConditionView,
    quota_zero: ConditionView,
    availability: ConditionView,
    email_enabled: bool,
    email_recipients: Vec<String>,
    bark_enabled: bool,
    bark_level: Option<String>,
    bark_sound: Option<String>,
    bark_volume: Option<u8>,
    bark_call: Option<bool>,
}
impl ReplacePolicyRequest {
    fn command(self) -> Result<GroupAlertPolicy, AdminError> {
        let bark_level = self
            .bark_level
            .as_deref()
            .map(|value| {
                BarkLevel::parse(value).ok_or_else(|| AdminError::bad_request("Bark 等级不合法"))
            })
            .transpose()?;
        Ok(GroupAlertPolicy {
            group_id: group_id(&self.group_id)?,
            enabled: self.enabled,
            concurrency: self.concurrency.into(),
            eta: self.eta.into(),
            quota_zero: self.quota_zero.into(),
            availability: self.availability.into(),
            email_enabled: self.email_enabled,
            email_recipients: self.email_recipients,
            bark_enabled: self.bark_enabled,
            bark_level,
            bark_sound: optional(self.bark_sound),
            bark_volume: self.bark_volume,
            bark_call: self.bark_call,
            updated_at: Utc::now(),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TestRequest {
    channel: String,
    target: String,
    group_id: Option<String>,
}

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/notifications/channels", get(channels::<S>))
        .route(
            "/api/admin/notifications/channels/update",
            post(replace_channels::<S>),
        )
        .route("/api/admin/notifications/test", post(test::<S>))
        .route("/api/admin/notifications/deliveries", get(deliveries::<S>))
        .route("/api/admin/account-groups/alert-policy", get(policy::<S>))
        .route(
            "/api/admin/account-groups/alert-policy/update",
            post(replace_policy::<S>),
        )
}

async fn channels<S>(
    _auth: AdminAuth,
    State(state): State<S>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(ChannelsView::from(
            state
                .admin_services()
                .notifications()
                .channels()
                .await
                .map_err(map_admin_service_error)?,
        )),
    ))
}
async fn replace_channels<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<ReplaceChannelsRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .notifications()
        .replace_channels(request.command()?, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(ChannelsView::from(result)),
    ))
}
async fn policy<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<GroupQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .notifications()
        .policy(&group_id(&query.group_id)?)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(PolicyView::from(result)),
    ))
}
async fn replace_policy<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<ReplacePolicyRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .notifications()
        .replace_policy(request.command()?, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(PolicyView::from(result)),
    ))
}
async fn test<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<TestRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let channel = match request.channel.as_str() {
        "email" => NotificationChannelKind::Email,
        "bark" => NotificationChannelKind::Bark,
        _ => return Err(AdminError::bad_request("通知测试类型不合法")),
    };
    let id = state
        .admin_services()
        .notifications()
        .test(TestNotificationCommand {
            channel,
            target: clean(request.target),
            group_id: request.group_id.as_deref().map(group_id).transpose()?,
        })
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(serde_json::json!({"id":id})),
    ))
}
async fn deliveries<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<DeliveriesQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let id = if query.group_id.trim().is_empty() {
        None
    } else {
        Some(group_id(&query.group_id)?)
    };
    let rows = state
        .admin_services()
        .notifications()
        .deliveries(id.as_ref(), 20)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(rows.into_iter().map(DeliveryView::from).collect::<Vec<_>>()),
    ))
}

fn group_id(value: &str) -> Result<AccountGroupId, AdminError> {
    AccountGroupId::new(value.to_owned()).map_err(|_| AdminError::bad_request("分组 ID 不合法"))
}
fn clean(value: String) -> String {
    value.trim().to_owned()
}
fn optional(value: Option<String>) -> Option<String> {
    value.map(clean).filter(|value| !value.is_empty())
}
