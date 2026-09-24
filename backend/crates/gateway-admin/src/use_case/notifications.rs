use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use secrecy::{ExposeSecret as _, SecretString};

use crate::{
    model::{
        AdminError, MutationContext,
        group_monitor::{GroupMonitorItem, GroupMonitorReport},
        notifications::{
            AlertConditionKind, AlertObservation, ClaimedNotificationDelivery, GroupAlertPolicy,
            NotificationChannelKind, NotificationChannelsView, NotificationDeliveryRecord,
            ReplaceNotificationChannels, StoredNotificationChannels, TestNotificationCommand,
            normalized_bark_device_key,
        },
    },
    ports::{
        notification::{BarkMessage, NotificationDelivery, SmtpMessage},
        store::{AccountGroupStore, SettingsStore},
    },
};

use super::map_store_error;

#[async_trait]
pub trait NotificationsService: Send + Sync {
    async fn active_alerts(&self) -> Result<Vec<(String, String)>, AdminError>;
    async fn channels(&self) -> Result<NotificationChannelsView, AdminError>;
    async fn replace_channels(
        &self,
        command: ReplaceNotificationChannels,
        context: &MutationContext,
    ) -> Result<NotificationChannelsView, AdminError>;
    async fn policy(
        &self,
        group_id: &gateway_core::routing::AccountGroupId,
    ) -> Result<GroupAlertPolicy, AdminError>;
    async fn replace_policy(
        &self,
        policy: GroupAlertPolicy,
        context: &MutationContext,
    ) -> Result<GroupAlertPolicy, AdminError>;
    async fn test(&self, command: TestNotificationCommand) -> Result<String, AdminError>;
    async fn deliveries(
        &self,
        group_id: Option<&gateway_core::routing::AccountGroupId>,
        limit: u32,
    ) -> Result<Vec<NotificationDeliveryRecord>, AdminError>;
    async fn observe(&self, report: &GroupMonitorReport) -> Result<(), AdminError>;
    async fn dispatch_one(&self) -> Result<bool, AdminError>;
}

pub(crate) struct DefaultNotificationsService {
    settings: Arc<dyn SettingsStore>,
    groups: Arc<dyn AccountGroupStore>,
    delivery: Arc<dyn NotificationDelivery>,
}

impl DefaultNotificationsService {
    pub(crate) fn new(
        settings: Arc<dyn SettingsStore>,
        groups: Arc<dyn AccountGroupStore>,
        delivery: Arc<dyn NotificationDelivery>,
    ) -> Self {
        Self {
            settings,
            groups,
            delivery,
        }
    }

    async fn deliver(&self, claimed: ClaimedNotificationDelivery) -> Result<(), AdminError> {
        let result = self.send(&claimed).await;
        self.groups
            .finish_notification_delivery(
                &claimed.id,
                result.is_ok(),
                result.as_ref().err().map(AdminError::message),
                Utc::now(),
            )
            .await
            .map_err(|error| map_store_error(error, "notification delivery"))?;
        result
    }

    async fn send(&self, claimed: &ClaimedNotificationDelivery) -> Result<(), AdminError> {
        if !claimed.test {
            let id = claimed
                .group_id
                .as_ref()
                .ok_or_else(|| AdminError::invalid("分组已删除"))?;
            let policy = self.policy(id).await?;
            let condition_enabled = match claimed.condition {
                Some(AlertConditionKind::Concurrency) => policy.concurrency.enabled,
                Some(AlertConditionKind::Eta) => policy.eta.enabled,
                Some(AlertConditionKind::QuotaZero) => policy.quota_zero.enabled,
                Some(AlertConditionKind::Availability) => policy.availability.enabled,
                None => false,
            };
            let route_enabled = match claimed.channel {
                NotificationChannelKind::Email => {
                    policy.email_enabled && policy.email_recipients.contains(&claimed.target)
                }
                NotificationChannelKind::Bark => policy.bark_enabled,
            };
            if !policy.enabled || !condition_enabled || !route_enabled {
                return Err(AdminError::invalid("分组通知已关闭"));
            }
        }
        let channels = self
            .settings
            .load_notification_channels()
            .await
            .map_err(|error| map_store_error(error, "notification channels"))?;
        let result = match claimed.channel {
            NotificationChannelKind::Email => {
                if !channels.smtp.view.enabled {
                    return Err(AdminError::invalid("SMTP 通道未启用"));
                }
                let from_email = channels
                    .smtp
                    .view
                    .from_email
                    .clone()
                    .ok_or_else(|| AdminError::invalid("SMTP 发件人未配置"))?;
                self.delivery
                    .send_smtp(SmtpMessage {
                        host: channels.smtp.view.host,
                        port: channels.smtp.view.port,
                        security: channels.smtp.view.security,
                        username: channels.smtp.view.username,
                        password: channels.smtp.password,
                        from_name: channels.smtp.view.from_name,
                        from_email,
                        recipient: claimed.target.clone(),
                        subject: claimed.subject.clone(),
                        html_body: email_body(claimed),
                    })
                    .await
            }
            NotificationChannelKind::Bark => {
                if !channels.bark.view.enabled {
                    return Err(AdminError::invalid("Bark 通道未启用"));
                }
                let key = channels
                    .bark
                    .device_key
                    .ok_or_else(|| AdminError::invalid("Bark Device Key 未配置"))?;
                self.delivery
                    .send_bark(BarkMessage {
                        server_url: channels.bark.view.server_url,
                        device_key: key,
                        title: claimed.subject.clone(),
                        body: claimed.body.clone(),
                        group: claimed
                            .group_id
                            .as_ref()
                            .map_or_else(|| "CPR".to_owned(), |id| format!("CPR/{}", id.as_str())),
                        level: claimed.bark_level,
                        sound: claimed.bark_sound.clone(),
                        volume: claimed.bark_volume,
                        call: claimed.bark_call,
                    })
                    .await
            }
        };
        result.map_err(|error| AdminError::unavailable(error.to_string()))
    }
}

#[async_trait]
impl NotificationsService for DefaultNotificationsService {
    async fn active_alerts(&self) -> Result<Vec<(String, String)>, AdminError> {
        self.groups
            .active_group_alerts()
            .await
            .map_err(|error| map_store_error(error, "group active alerts"))
    }

    async fn channels(&self) -> Result<NotificationChannelsView, AdminError> {
        let stored = self
            .settings
            .load_notification_channels()
            .await
            .map_err(|error| map_store_error(error, "notification channels"))?;
        let last_test = self
            .groups
            .latest_notification_test()
            .await
            .map_err(|error| map_store_error(error, "notification deliveries"))?;
        Ok(NotificationChannelsView {
            smtp: stored.smtp.view,
            bark: stored.bark.view,
            last_test,
            updated_at: stored.updated_at,
        })
    }

    async fn replace_channels(
        &self,
        mut command: ReplaceNotificationChannels,
        context: &MutationContext,
    ) -> Result<NotificationChannelsView, AdminError> {
        let current = self
            .settings
            .load_notification_channels()
            .await
            .map_err(|error| map_store_error(error, "notification channels"))?;
        if command.smtp.password.is_none() {
            command.smtp.view.password_set = current.smtp.view.password_set;
        }
        if command.bark.device_key.is_none() {
            command.bark.view.device_key_set = current.bark.view.device_key_set;
        }
        if let Some(key) = &command.bark.device_key {
            let normalized = normalized_bark_device_key(key.expose_secret()).ok_or_else(|| {
                AdminError::invalid("Bark Device Key 格式不正确，只填写设备密钥，不要填写完整链接")
            })?;
            command.bark.device_key = Some(SecretString::from(normalized.to_owned()));
        }
        validate_channels(&command)?;
        self.settings
            .replace_notification_channels(command, context)
            .await
            .map_err(|error| map_store_error(error, "notification channels"))?;
        self.channels().await
    }

    async fn policy(
        &self,
        group_id: &gateway_core::routing::AccountGroupId,
    ) -> Result<GroupAlertPolicy, AdminError> {
        self.groups
            .load_group_alert_policy(group_id)
            .await
            .map_err(|error| map_store_error(error, "group alert policy"))
    }

    async fn replace_policy(
        &self,
        mut policy: GroupAlertPolicy,
        context: &MutationContext,
    ) -> Result<GroupAlertPolicy, AdminError> {
        normalize_policy(&mut policy)?;
        self.groups
            .replace_group_alert_policy(policy, context)
            .await
            .map_err(|error| map_store_error(error, "group alert policy"))
    }

    async fn test(&self, command: TestNotificationCommand) -> Result<String, AdminError> {
        let channels = self
            .settings
            .load_notification_channels()
            .await
            .map_err(|error| map_store_error(error, "notification channels"))?;
        let id = uuid::Uuid::now_v7().to_string();
        if command.channel == NotificationChannelKind::Email && !valid_email(&command.target) {
            return Err(AdminError::invalid("测试收件人不合法"));
        }
        let policy = match &command.group_id {
            Some(id) => Some(self.policy(id).await?),
            None => None,
        };
        let delivery = ClaimedNotificationDelivery {
            id: id.clone(),
            group_id: command.group_id,
            condition: None,
            channel: command.channel,
            target: if command.channel == NotificationChannelKind::Bark {
                "default".to_owned()
            } else {
                command.target
            },
            subject: "CPR 通知测试".to_owned(),
            body: "通知渠道连接正常。这是一条测试通知。".to_owned(),
            bark_level: policy
                .as_ref()
                .and_then(|p| p.bark_level)
                .unwrap_or(channels.bark.view.level),
            bark_sound: policy
                .as_ref()
                .and_then(|p| p.bark_sound.clone())
                .or(channels.bark.view.sound),
            bark_volume: policy
                .as_ref()
                .and_then(|p| p.bark_volume)
                .unwrap_or(channels.bark.view.volume),
            bark_call: policy
                .as_ref()
                .and_then(|p| p.bark_call)
                .unwrap_or(channels.bark.view.call),
            test: true,
        };
        self.groups
            .enqueue_test_notification(delivery.clone(), Utc::now())
            .await
            .map_err(|error| map_store_error(error, "notification test"))?;
        self.deliver(delivery).await?;
        Ok(id)
    }

    async fn deliveries(
        &self,
        group_id: Option<&gateway_core::routing::AccountGroupId>,
        limit: u32,
    ) -> Result<Vec<NotificationDeliveryRecord>, AdminError> {
        self.groups
            .recent_notification_deliveries(group_id, limit)
            .await
            .map_err(|error| map_store_error(error, "notification deliveries"))
    }

    async fn observe(&self, report: &GroupMonitorReport) -> Result<(), AdminError> {
        if report.refreshing || !report.pending_group_ids.is_empty() {
            return Ok(());
        }
        let channels = self
            .settings
            .load_notification_channels()
            .await
            .map_err(|error| map_store_error(error, "notification channels"))?;
        let mut observations = Vec::new();
        for item in &report.items {
            let policy = self
                .groups
                .load_group_alert_policy(&item.group.id)
                .await
                .map_err(|error| map_store_error(error, "group alert policy"))?;
            observations.extend(group_observations(item, &policy, &channels));
        }
        self.groups
            .apply_group_alert_observations(&observations, report.generated_at)
            .await
            .map_err(|error| map_store_error(error, "group alert observations"))
    }

    async fn dispatch_one(&self) -> Result<bool, AdminError> {
        let Some(claimed) = self
            .groups
            .claim_notification_delivery(Utc::now())
            .await
            .map_err(|error| map_store_error(error, "notification delivery"))?
        else {
            return Ok(false);
        };
        if let Err(error) = self.deliver(claimed).await {
            tracing::warn!(error = %error, "notification delivery failed");
        }
        Ok(true)
    }
}

fn group_observations(
    item: &GroupMonitorItem,
    policy: &GroupAlertPolicy,
    channels: &StoredNotificationChannels,
) -> Vec<AlertObservation> {
    let email_recipients = if policy.enabled && policy.email_enabled && smtp_ready(channels) {
        policy.email_recipients.clone()
    } else {
        Vec::new()
    };
    let bark_enabled = policy.enabled
        && policy.bark_enabled
        && channels.bark.view.enabled
        && channels.bark.view.device_key_set;
    let level = policy.bark_level.unwrap_or(channels.bark.view.level);
    let sound = policy
        .bark_sound
        .clone()
        .or_else(|| channels.bark.view.sound.clone());
    let volume = policy.bark_volume.unwrap_or(channels.bark.view.volume);
    let call = policy.bark_call.unwrap_or(channels.bark.view.call);
    let concurrency = item.used_slots.and_then(|used| {
        (item.total_slots > 0).then_some(used as f64 * 100.0 / item.total_slots as f64)
    });
    let eta = item
        .eta_minutes
        .filter(|value| item.eta_status == "ready" && value.is_finite());
    let quota = item
        .remaining_usd
        .filter(|value| matches!(item.remaining_status, "ready" | "empty") && value.is_finite());
    let rows = [
        (
            AlertConditionKind::Concurrency,
            policy.concurrency.clone(),
            concurrency,
            concurrency.is_some_and(|value| value >= policy.concurrency.threshold),
        ),
        (
            AlertConditionKind::Eta,
            policy.eta.clone(),
            eta,
            eta.is_some_and(|value| value >= 0.0 && value <= policy.eta.threshold),
        ),
        (
            AlertConditionKind::QuotaZero,
            policy.quota_zero.clone(),
            quota,
            quota.is_some_and(|value| value <= 0.0),
        ),
        (
            AlertConditionKind::Availability,
            policy.availability.clone(),
            Some(item.eligible_accounts as f64),
            item.eligible_accounts == 0,
        ),
    ];
    rows.into_iter()
        .filter_map(|(kind, condition, current, active)| {
            let enabled = policy.enabled
                && condition.enabled
                && (!email_recipients.is_empty() || bark_enabled);
            // Missing telemetry is neither a failure nor confirmed recovery.
            if enabled && current.is_none() {
                return None;
            }
            Some(AlertObservation {
                group_id: item.group.id.clone(),
                group_name: item.group.name.clone(),
                kind,
                active: enabled && active,
                current_value: current,
                threshold: condition.threshold,
                confirmation_seconds: condition.confirmation_seconds,
                email_recipients: email_recipients.clone(),
                bark_enabled,
                bark_level: level,
                bark_sound: sound.clone(),
                bark_volume: volume,
                bark_call: call,
                summary: alert_summary(item, kind, current, condition.threshold),
            })
        })
        .collect()
}

fn smtp_ready(channels: &StoredNotificationChannels) -> bool {
    channels.smtp.view.enabled
        && !channels.smtp.view.host.is_empty()
        && channels
            .smtp
            .view
            .from_email
            .as_deref()
            .is_some_and(valid_email)
        && (channels.smtp.view.username.is_none() || channels.smtp.view.password_set)
}

fn alert_summary(
    item: &GroupMonitorItem,
    kind: AlertConditionKind,
    current: Option<f64>,
    threshold: f64,
) -> String {
    let label = match kind {
        AlertConditionKind::Concurrency => "并发达到阈值（%）",
        AlertConditionKind::Eta => "预计可支撑时间不足（分钟）",
        AlertConditionKind::QuotaZero => "剩余额度为 0",
        AlertConditionKind::Availability => "可调度账号为 0",
    };
    format!(
        "分组：{}\n条件：{}\n当前值：{}\n阈值：{:.2}\n并发：{} / {}\n可调度账号：{} / {}\n预计剩余额度（美元）：{}\n预计可支撑（分钟）：{}",
        item.group.name,
        label,
        current.map_or_else(|| "未知".to_owned(), |value| format!("{value:.4}")),
        threshold,
        item.used_slots
            .map_or_else(|| "未知".to_owned(), |value| value.to_string()),
        item.total_slots,
        item.eligible_accounts,
        item.total_accounts,
        item.remaining_usd
            .map_or_else(|| "未知".to_owned(), |value| format!("{value:.2}")),
        item.eta_minutes
            .map_or_else(|| "未知".to_owned(), |value| format!("{value:.1}"))
    )
}

fn email_body(claimed: &ClaimedNotificationDelivery) -> String {
    format!(
        "<div style=\"font-family:Arial,sans-serif;white-space:pre-line\"><h2>{}</h2><p>{}</p></div>",
        html_escape(&claimed.subject),
        html_escape(&claimed.body)
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn validate_channels(command: &ReplaceNotificationChannels) -> Result<(), AdminError> {
    if command.smtp.view.port == 0 || command.bark.view.volume > 10 {
        return Err(AdminError::invalid("通知端口或音量不合法"));
    }
    if command.smtp.view.enabled
        && (command.smtp.view.host.trim().is_empty()
            || command
                .smtp
                .view
                .from_email
                .as_deref()
                .is_none_or(|value| !valid_email(value))
            || (command.smtp.view.username.is_some()
                && command.smtp.password.is_none()
                && !command.smtp.view.password_set))
    {
        return Err(AdminError::invalid("SMTP 配置不完整"));
    }
    if command.bark.view.enabled
        && (url::Url::parse(command.bark.view.server_url.trim())
            .ok()
            .is_none_or(|url| {
                !matches!(url.scheme(), "http" | "https")
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.fragment().is_some()
                    || url.query().is_some()
                    || url.host_str().is_none()
            })
            || (command.bark.device_key.is_none() && !command.bark.view.device_key_set))
    {
        return Err(AdminError::invalid("Bark 配置不完整"));
    }
    Ok(())
}

fn normalize_policy(policy: &mut GroupAlertPolicy) -> Result<(), AdminError> {
    if !(0.0 < policy.concurrency.threshold && policy.concurrency.threshold <= 100.0)
        || !(0.0..=525_600.0).contains(&policy.eta.threshold)
        || policy.bark_volume.is_some_and(|volume| volume > 10)
        || [
            policy.concurrency.confirmation_seconds,
            policy.eta.confirmation_seconds,
            policy.quota_zero.confirmation_seconds,
            policy.availability.confirmation_seconds,
        ]
        .into_iter()
        .any(|value| value > 86_400)
    {
        return Err(AdminError::invalid("分组预警阈值不合法"));
    }
    policy.email_recipients = policy
        .email_recipients
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if policy.email_recipients.len() > 32
        || policy
            .email_recipients
            .iter()
            .any(|value| !valid_email(value))
    {
        return Err(AdminError::invalid("告警收件人不合法"));
    }
    Ok(())
}

fn valid_email(value: &str) -> bool {
    let value = value.trim();
    value.len() <= 320
        && !value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
        && value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        })
}
