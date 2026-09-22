use chrono::{DateTime, Duration, Utc};
use gateway_admin::model::{
    MutationContext,
    notifications::{
        AlertConditionKind, AlertConditionPolicy, AlertObservation, BarkChannelView, BarkLevel,
        ClaimedNotificationDelivery, GroupAlertPolicy, NotificationChannelKind,
        NotificationChannelsView, NotificationDeliveryRecord, ReplaceNotificationChannels,
        SmtpChannelView, SmtpSecurity, StoredBarkChannel, StoredNotificationChannels,
        StoredSmtpChannel,
    },
};
use gateway_core::routing::AccountGroupId;
use secrecy::{ExposeSecret as _, SecretString};
use sqlx::{PgPool, Row as _};

use crate::{
    StoreResult, mutation_audit,
    postgres::{append_admin_audit_event_in_transaction, bump_config_revision_in_transaction},
    postgres_unavailable,
};

#[derive(Clone)]
pub struct PgNotificationRepository {
    pool: PgPool,
}

impl PgNotificationRepository {
    pub async fn active_alerts(&self) -> StoreResult<Vec<(String, String)>> {
        sqlx::query_as(
            "select group_id,condition_kind from account_group_alert_incidents
             where activated_at is not null order by group_id,condition_kind",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("load active alerts"))
    }

    pub async fn latest_test(&self) -> StoreResult<Option<NotificationDeliveryRecord>> {
        let row = sqlx::query(
            "select * from notification_outbox where test order by created_at desc limit 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("load latest notification test"))?;
        row.as_ref().map(delivery_record_from_row).transpose()
    }

    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn load_channels(&self) -> StoreResult<StoredNotificationChannels> {
        let row = sqlx::query("select * from notification_channels where id = 1")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| postgres_unavailable("load notification channels"))?;
        channels_from_row(&row)
    }

    pub async fn replace_channels(
        &self,
        command: ReplaceNotificationChannels,
        context: &MutationContext,
    ) -> StoreResult<NotificationChannelsView> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin notification channel update"))?;
        let revision = bump_config_revision_in_transaction(&mut tx).await?;
        let smtp_password = command
            .smtp
            .password
            .as_ref()
            .map(|value| value.expose_secret().to_owned());
        let bark_device_key = command
            .bark
            .device_key
            .as_ref()
            .map(|value| value.expose_secret().to_owned());
        let row = sqlx::query(
            "update notification_channels set
               smtp_enabled=$1,smtp_host=$2,smtp_port=$3,smtp_security=$4,
               smtp_username=$5,smtp_password=coalesce($6,smtp_password),
               smtp_from_name=$7,smtp_from_email=$8,
               bark_enabled=$9,bark_server_url=$10,bark_device_key=coalesce($11,bark_device_key),
               bark_level=$12,bark_sound=$13,bark_volume=$14,bark_call=$15,updated_at=now()
             where id=1 returning *",
        )
        .bind(command.smtp.view.enabled)
        .bind(command.smtp.view.host)
        .bind(i64::from(command.smtp.view.port))
        .bind(command.smtp.view.security.as_str())
        .bind(command.smtp.view.username)
        .bind(smtp_password)
        .bind(command.smtp.view.from_name)
        .bind(command.smtp.view.from_email)
        .bind(command.bark.view.enabled)
        .bind(command.bark.view.server_url)
        .bind(bark_device_key)
        .bind(command.bark.view.level.as_str())
        .bind(command.bark.view.sound)
        .bind(i64::from(command.bark.view.volume))
        .bind(command.bark.view.call)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| postgres_unavailable("update notification channels"))?;
        append_admin_audit_event_in_transaction(
            &mut tx,
            mutation_audit(
                context,
                "notification_channels.replace",
                "notification_channels",
                "1",
                vec!["smtp".to_owned(), "bark".to_owned()],
            ),
            revision,
        )
        .await?;
        tx.commit()
            .await
            .map_err(|_| postgres_unavailable("commit notification channel update"))?;
        let stored = channels_from_row(&row)?;
        Ok(NotificationChannelsView {
            smtp: stored.smtp.view,
            bark: stored.bark.view,
            last_test: None,
            updated_at: stored.updated_at,
        })
    }

    pub async fn load_policy(&self, group_id: &AccountGroupId) -> StoreResult<GroupAlertPolicy> {
        let row = sqlx::query("select * from account_group_alert_policies where group_id=$1")
            .bind(group_id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| postgres_unavailable("load group alert policy"))?;
        row.as_ref().map(policy_from_row).transpose().map(|policy| {
            policy.unwrap_or_else(|| GroupAlertPolicy::defaults(group_id.clone(), Utc::now()))
        })
    }

    pub async fn replace_policy(
        &self,
        policy: GroupAlertPolicy,
        context: &MutationContext,
    ) -> StoreResult<GroupAlertPolicy> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin group alert policy update"))?;
        let revision = bump_config_revision_in_transaction(&mut tx).await?;
        let recipients = serde_json::to_value(&policy.email_recipients)
            .map_err(|_| postgres_unavailable("encode group alert recipients"))?;
        let row = sqlx::query(
            "insert into account_group_alert_policies (
               group_id,enabled,concurrency_enabled,concurrency_threshold,concurrency_confirmation_seconds,
               eta_enabled,eta_threshold_minutes,eta_confirmation_seconds,
               quota_zero_enabled,quota_zero_confirmation_seconds,
               availability_enabled,availability_confirmation_seconds,
               email_enabled,email_recipients_json,bark_enabled,bark_level,bark_sound,bark_volume,bark_call,updated_at)
             values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,now())
             on conflict(group_id) do update set
               enabled=excluded.enabled,concurrency_enabled=excluded.concurrency_enabled,
               concurrency_threshold=excluded.concurrency_threshold,
               concurrency_confirmation_seconds=excluded.concurrency_confirmation_seconds,
               eta_enabled=excluded.eta_enabled,eta_threshold_minutes=excluded.eta_threshold_minutes,
               eta_confirmation_seconds=excluded.eta_confirmation_seconds,
               quota_zero_enabled=excluded.quota_zero_enabled,
               quota_zero_confirmation_seconds=excluded.quota_zero_confirmation_seconds,
               availability_enabled=excluded.availability_enabled,
               availability_confirmation_seconds=excluded.availability_confirmation_seconds,
               email_enabled=excluded.email_enabled,email_recipients_json=excluded.email_recipients_json,
               bark_enabled=excluded.bark_enabled,bark_level=excluded.bark_level,
               bark_sound=excluded.bark_sound,bark_volume=excluded.bark_volume,
               bark_call=excluded.bark_call,updated_at=excluded.updated_at returning *",
        )
        .bind(policy.group_id.as_str())
        .bind(policy.enabled)
        .bind(policy.concurrency.enabled)
        .bind(policy.concurrency.threshold)
        .bind(i64::from(policy.concurrency.confirmation_seconds))
        .bind(policy.eta.enabled)
        .bind(policy.eta.threshold)
        .bind(i64::from(policy.eta.confirmation_seconds))
        .bind(policy.quota_zero.enabled)
        .bind(i64::from(policy.quota_zero.confirmation_seconds))
        .bind(policy.availability.enabled)
        .bind(i64::from(policy.availability.confirmation_seconds))
        .bind(policy.email_enabled)
        .bind(recipients)
        .bind(policy.bark_enabled)
        .bind(policy.bark_level.map(BarkLevel::as_str))
        .bind(policy.bark_sound)
        .bind(policy.bark_volume.map(i64::from))
        .bind(policy.bark_call)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| postgres_unavailable("replace group alert policy"))?;
        append_admin_audit_event_in_transaction(
            &mut tx,
            mutation_audit(
                context,
                "account_group_alert_policy.replace",
                "account_group_alert_policies",
                policy.group_id.as_str(),
                vec!["conditions".to_owned(), "routing".to_owned()],
            ),
            revision,
        )
        .await?;
        tx.commit()
            .await
            .map_err(|_| postgres_unavailable("commit group alert policy update"))?;
        policy_from_row(&row)
    }

    pub async fn apply_observations(
        &self,
        observations: &[AlertObservation],
        now: DateTime<Utc>,
    ) -> StoreResult<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin group alert observation"))?;
        let mut ordered: Vec<_> = observations.iter().collect();
        ordered.sort_by(|a, b| (a.group_id.as_str(), a.kind).cmp(&(b.group_id.as_str(), b.kind)));
        for observation in ordered {
            // Materialize the key before locking: SELECT FOR UPDATE cannot lock a missing row.
            sqlx::query(
                "insert into account_group_alert_incidents(group_id,condition_kind,updated_at)
                 values($1,$2,$3) on conflict do nothing",
            )
            .bind(observation.group_id.as_str())
            .bind(observation.kind.as_str())
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| postgres_unavailable("initialize group alert incident"))?;
            let row = sqlx::query(
                "select first_active_at,activated_at,recovery_started_at,updated_at
                 from account_group_alert_incidents where group_id=$1 and condition_kind=$2 for update",
            )
            .bind(observation.group_id.as_str())
            .bind(observation.kind.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| postgres_unavailable("lock group alert incident"))?;
            if row.as_ref().is_some_and(|row| {
                row.try_get::<DateTime<Utc>, _>("updated_at")
                    .is_ok_and(|updated| updated > now)
            }) {
                continue;
            }
            let interrupted = row.as_ref().is_some_and(|row| {
                row.try_get::<DateTime<Utc>, _>("updated_at")
                    .is_ok_and(|updated| now.signed_duration_since(updated).num_seconds() > 30)
            });
            let first_active = row
                .as_ref()
                .and_then(|row| {
                    row.try_get::<Option<DateTime<Utc>>, _>("first_active_at")
                        .ok()
                })
                .flatten();
            let activated = row
                .as_ref()
                .and_then(|row| row.try_get::<Option<DateTime<Utc>>, _>("activated_at").ok())
                .flatten();
            let recovery = row
                .as_ref()
                .and_then(|row| {
                    row.try_get::<Option<DateTime<Utc>>, _>("recovery_started_at")
                        .ok()
                })
                .flatten();
            if observation.active {
                let first = if interrupted && activated.is_none() {
                    now
                } else {
                    first_active.unwrap_or(now)
                };
                let should_activate = activated.is_none()
                    && now.signed_duration_since(first)
                        >= Duration::seconds(i64::from(observation.confirmation_seconds));
                sqlx::query(
                    "insert into account_group_alert_incidents(group_id,condition_kind,first_active_at,activated_at,recovery_started_at,updated_at)
                     values($1,$2,$3,$4,null,$5)
                     on conflict(group_id,condition_kind) do update set
                       first_active_at=excluded.first_active_at,
                       activated_at=coalesce(account_group_alert_incidents.activated_at,excluded.activated_at),
                       recovery_started_at=null,updated_at=excluded.updated_at",
                )
                .bind(observation.group_id.as_str())
                .bind(observation.kind.as_str())
                .bind(first)
                .bind(should_activate.then_some(now))
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|_| postgres_unavailable("save group alert incident"))?;
                if should_activate {
                    enqueue_observation(&mut tx, observation, now).await?;
                }
            } else if first_active.is_some() || activated.is_some() {
                let recovery_started = if interrupted {
                    now
                } else {
                    recovery.unwrap_or(now)
                };
                if activated.is_none()
                    || now.signed_duration_since(recovery_started) >= Duration::seconds(30)
                {
                    sqlx::query(
                        "update account_group_alert_incidents
                         set first_active_at=null,activated_at=null,recovery_started_at=null,updated_at=$3
                         where group_id=$1 and condition_kind=$2",
                    )
                    .bind(observation.group_id.as_str())
                    .bind(observation.kind.as_str())
                    .bind(now)
                    .execute(&mut *tx)
                    .await
                    .map_err(|_| postgres_unavailable("clear group alert incident"))?;
                } else {
                    sqlx::query(
                        "update account_group_alert_incidents set recovery_started_at=$3,updated_at=$4
                         where group_id=$1 and condition_kind=$2",
                    )
                    .bind(observation.group_id.as_str())
                    .bind(observation.kind.as_str())
                    .bind(recovery_started)
                    .bind(now)
                    .execute(&mut *tx)
                    .await
                    .map_err(|_| postgres_unavailable("recover group alert incident"))?;
                }
            } else {
                sqlx::query(
                    "update account_group_alert_incidents set updated_at=$3
                     where group_id=$1 and condition_kind=$2",
                )
                .bind(observation.group_id.as_str())
                .bind(observation.kind.as_str())
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|_| postgres_unavailable("record inactive alert sample"))?;
            }
        }
        tx.commit()
            .await
            .map_err(|_| postgres_unavailable("commit group alert observation"))
    }

    pub async fn claim_delivery(
        &self,
        now: DateTime<Utc>,
    ) -> StoreResult<Option<ClaimedNotificationDelivery>> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin notification claim"))?;
        sqlx::query(
            "update notification_outbox o set status='failed',finished_at=$1,
               error_summary='alert recovered or test interrupted',claim_started_at=null
             where (status='pending' or (status='sending' and claim_started_at<=$2))
               and (test or not exists (
                 select 1 from account_group_alert_incidents i
                 where i.group_id=o.group_id and i.condition_kind=o.condition_kind
                   and i.activated_at=o.created_at))",
        )
        .bind(now)
        .bind(now - Duration::minutes(5))
        .execute(&mut *tx)
        .await
        .map_err(|_| postgres_unavailable("expire inactive notifications"))?;
        let row = sqlx::query(
            "select * from notification_outbox
             where (status='pending' and next_attempt_at<=$1)
                or (status='sending' and claim_started_at<=$2)
             order by next_attempt_at,created_at for update skip locked limit 1",
        )
        .bind(now)
        .bind(now - Duration::minutes(5))
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| postgres_unavailable("claim notification delivery"))?;
        let Some(row) = row else {
            tx.commit()
                .await
                .map_err(|_| postgres_unavailable("finish empty notification claim"))?;
            return Ok(None);
        };
        let id: String = row
            .try_get("id")
            .map_err(|_| postgres_unavailable("decode delivery"))?;
        sqlx::query(
            "update notification_outbox set status='sending',attempts=attempts+1,claim_started_at=$2 where id=$1",
        )
        .bind(&id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|_| postgres_unavailable("mark notification sending"))?;
        tx.commit()
            .await
            .map_err(|_| postgres_unavailable("commit notification claim"))?;
        Ok(Some(claimed_from_row(&row)?))
    }

    pub async fn finish_delivery(
        &self,
        id: &str,
        delivered: bool,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> StoreResult<()> {
        sqlx::query(
            "update notification_outbox set
               status=case when $2 then 'sent' when test or attempts>=5 then 'failed' else 'pending' end,
               next_attempt_at=case when $2 or test or attempts>=5 then next_attempt_at else $3 end,
               claim_started_at=null,error_summary=$4,
               finished_at=case when $2 or test or attempts>=5 then $5 else null end
             where id=$1",
        )
        .bind(id)
        .bind(delivered)
        .bind(now + Duration::minutes(1))
        .bind(error.map(|value| value.chars().take(512).collect::<String>()))
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("finish notification delivery"))?;
        Ok(())
    }

    pub async fn enqueue_test(
        &self,
        delivery: ClaimedNotificationDelivery,
        now: DateTime<Utc>,
    ) -> StoreResult<String> {
        insert_delivery(&self.pool, &delivery, now).await?;
        Ok(delivery.id)
    }

    pub async fn recent_deliveries(
        &self,
        group_id: Option<&AccountGroupId>,
        limit: u32,
    ) -> StoreResult<Vec<NotificationDeliveryRecord>> {
        let rows = sqlx::query(
            "select * from notification_outbox
             where ($1::text is null or group_id=$1)
             order by created_at desc limit $2",
        )
        .bind(group_id.map(AccountGroupId::as_str))
        .bind(i64::from(limit.min(100)))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("list notification deliveries"))?;
        rows.iter().map(delivery_record_from_row).collect()
    }
}

async fn enqueue_observation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    observation: &AlertObservation,
    now: DateTime<Utc>,
) -> StoreResult<()> {
    for recipient in &observation.email_recipients {
        let delivery = ClaimedNotificationDelivery {
            id: uuid::Uuid::now_v7().to_string(),
            group_id: Some(observation.group_id.clone()),
            condition: Some(observation.kind),
            channel: NotificationChannelKind::Email,
            target: recipient.clone(),
            subject: format!("[CPR] 分组预警：{}", observation.group_name),
            body: observation.summary.clone(),
            bark_level: observation.bark_level,
            bark_sound: observation.bark_sound.clone(),
            bark_volume: observation.bark_volume,
            bark_call: observation.bark_call,
            test: false,
        };
        insert_delivery(&mut **tx, &delivery, now).await?;
    }
    if observation.bark_enabled {
        let delivery = ClaimedNotificationDelivery {
            id: uuid::Uuid::now_v7().to_string(),
            group_id: Some(observation.group_id.clone()),
            condition: Some(observation.kind),
            channel: NotificationChannelKind::Bark,
            target: "default".to_owned(),
            subject: format!("CPR 分组预警：{}", observation.group_name),
            body: observation.summary.clone(),
            bark_level: observation.bark_level,
            bark_sound: observation.bark_sound.clone(),
            bark_volume: observation.bark_volume,
            bark_call: observation.bark_call,
            test: false,
        };
        insert_delivery(&mut **tx, &delivery, now).await?;
    }
    Ok(())
}

async fn insert_delivery<'e, E>(
    executor: E,
    delivery: &ClaimedNotificationDelivery,
    now: DateTime<Utc>,
) -> StoreResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        "insert into notification_outbox(
           id,group_id,condition_kind,channel,target,subject,body,bark_level,bark_sound,
           bark_volume,bark_call,test,status,attempts,next_attempt_at,created_at,claim_started_at)
         values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,
           case when $12 then 'sending' else 'pending' end,
           case when $12 then 1 else 0 end,$13,$13,case when $12 then $13 else null end)",
    )
    .bind(&delivery.id)
    .bind(delivery.group_id.as_ref().map(AccountGroupId::as_str))
    .bind(delivery.condition.map(|kind| kind.as_str()))
    .bind(delivery.channel.as_str())
    .bind(&delivery.target)
    .bind(&delivery.subject)
    .bind(&delivery.body)
    .bind(delivery.bark_level.as_str())
    .bind(&delivery.bark_sound)
    .bind(i64::from(delivery.bark_volume))
    .bind(delivery.bark_call)
    .bind(delivery.test)
    .bind(now)
    .execute(executor)
    .await
    .map_err(|_| postgres_unavailable("enqueue notification delivery"))?;
    Ok(())
}

fn channels_from_row(row: &sqlx::postgres::PgRow) -> StoreResult<StoredNotificationChannels> {
    let smtp_password = row
        .try_get::<Option<String>, _>("smtp_password")
        .map_err(|_| postgres_unavailable("decode smtp secret"))?;
    let bark_key = row
        .try_get::<Option<String>, _>("bark_device_key")
        .map_err(|_| postgres_unavailable("decode bark secret"))?;
    let smtp_port = row
        .try_get::<i64, _>("smtp_port")
        .ok()
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| postgres_unavailable("decode smtp port"))?;
    let bark_volume = row
        .try_get::<i64, _>("bark_volume")
        .ok()
        .and_then(|value| u8::try_from(value).ok())
        .ok_or_else(|| postgres_unavailable("decode bark volume"))?;
    Ok(StoredNotificationChannels {
        smtp: StoredSmtpChannel {
            view: SmtpChannelView {
                enabled: row
                    .try_get("smtp_enabled")
                    .map_err(|_| postgres_unavailable("decode smtp"))?,
                host: row
                    .try_get("smtp_host")
                    .map_err(|_| postgres_unavailable("decode smtp"))?,
                port: smtp_port,
                security: SmtpSecurity::parse(
                    &row.try_get::<String, _>("smtp_security")
                        .map_err(|_| postgres_unavailable("decode smtp"))?,
                )
                .ok_or_else(|| postgres_unavailable("decode smtp security"))?,
                username: row
                    .try_get("smtp_username")
                    .map_err(|_| postgres_unavailable("decode smtp"))?,
                password_set: smtp_password.is_some(),
                from_name: row
                    .try_get("smtp_from_name")
                    .map_err(|_| postgres_unavailable("decode smtp"))?,
                from_email: row
                    .try_get("smtp_from_email")
                    .map_err(|_| postgres_unavailable("decode smtp"))?,
            },
            password: smtp_password.map(SecretString::from),
        },
        bark: StoredBarkChannel {
            view: BarkChannelView {
                enabled: row
                    .try_get("bark_enabled")
                    .map_err(|_| postgres_unavailable("decode bark"))?,
                server_url: row
                    .try_get("bark_server_url")
                    .map_err(|_| postgres_unavailable("decode bark"))?,
                device_key_set: bark_key.is_some(),
                level: BarkLevel::parse(
                    &row.try_get::<String, _>("bark_level")
                        .map_err(|_| postgres_unavailable("decode bark"))?,
                )
                .ok_or_else(|| postgres_unavailable("decode bark level"))?,
                sound: row
                    .try_get("bark_sound")
                    .map_err(|_| postgres_unavailable("decode bark"))?,
                volume: bark_volume,
                call: row
                    .try_get("bark_call")
                    .map_err(|_| postgres_unavailable("decode bark"))?,
            },
            device_key: bark_key.map(SecretString::from),
        },
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| postgres_unavailable("decode notification channels"))?,
    })
}

fn policy_from_row(row: &sqlx::postgres::PgRow) -> StoreResult<GroupAlertPolicy> {
    let group_id = AccountGroupId::new(
        row.try_get::<String, _>("group_id")
            .map_err(|_| postgres_unavailable("decode alert policy"))?,
    )
    .map_err(|_| postgres_unavailable("decode alert policy group"))?;
    let condition =
        |enabled: &str, threshold: &str, confirmation: &str| -> StoreResult<AlertConditionPolicy> {
            Ok(AlertConditionPolicy {
                enabled: row
                    .try_get(enabled)
                    .map_err(|_| postgres_unavailable("decode alert condition"))?,
                threshold: row
                    .try_get(threshold)
                    .map_err(|_| postgres_unavailable("decode alert condition"))?,
                confirmation_seconds: row
                    .try_get::<i64, _>(confirmation)
                    .ok()
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| postgres_unavailable("decode alert confirmation"))?,
            })
        };
    let recipients = row
        .try_get::<serde_json::Value, _>("email_recipients_json")
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| postgres_unavailable("decode alert recipients"))?;
    Ok(GroupAlertPolicy {
        group_id,
        enabled: row
            .try_get("enabled")
            .map_err(|_| postgres_unavailable("decode alert policy"))?,
        concurrency: condition(
            "concurrency_enabled",
            "concurrency_threshold",
            "concurrency_confirmation_seconds",
        )?,
        eta: condition(
            "eta_enabled",
            "eta_threshold_minutes",
            "eta_confirmation_seconds",
        )?,
        quota_zero: AlertConditionPolicy {
            enabled: row
                .try_get("quota_zero_enabled")
                .map_err(|_| postgres_unavailable("decode alert condition"))?,
            threshold: 0.0,
            confirmation_seconds: row
                .try_get::<i64, _>("quota_zero_confirmation_seconds")
                .ok()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| postgres_unavailable("decode alert confirmation"))?,
        },
        availability: AlertConditionPolicy {
            enabled: row
                .try_get("availability_enabled")
                .map_err(|_| postgres_unavailable("decode alert condition"))?,
            threshold: 0.0,
            confirmation_seconds: row
                .try_get::<i64, _>("availability_confirmation_seconds")
                .ok()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| postgres_unavailable("decode alert confirmation"))?,
        },
        email_enabled: row
            .try_get("email_enabled")
            .map_err(|_| postgres_unavailable("decode alert routing"))?,
        email_recipients: recipients,
        bark_enabled: row
            .try_get("bark_enabled")
            .map_err(|_| postgres_unavailable("decode alert routing"))?,
        bark_level: row
            .try_get::<Option<String>, _>("bark_level")
            .ok()
            .flatten()
            .as_deref()
            .and_then(BarkLevel::parse),
        bark_sound: row
            .try_get("bark_sound")
            .map_err(|_| postgres_unavailable("decode bark policy"))?,
        bark_volume: row
            .try_get::<Option<i64>, _>("bark_volume")
            .ok()
            .flatten()
            .and_then(|value| u8::try_from(value).ok()),
        bark_call: row
            .try_get("bark_call")
            .map_err(|_| postgres_unavailable("decode bark policy"))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| postgres_unavailable("decode alert policy"))?,
    })
}

fn claimed_from_row(row: &sqlx::postgres::PgRow) -> StoreResult<ClaimedNotificationDelivery> {
    let group_id = row
        .try_get::<Option<String>, _>("group_id")
        .ok()
        .flatten()
        .map(AccountGroupId::new)
        .transpose()
        .map_err(|_| postgres_unavailable("decode delivery group"))?;
    let channel = match row
        .try_get::<String, _>("channel")
        .map_err(|_| postgres_unavailable("decode delivery"))?
        .as_str()
    {
        "email" => NotificationChannelKind::Email,
        "bark" => NotificationChannelKind::Bark,
        _ => return Err(postgres_unavailable("decode delivery channel")),
    };
    Ok(ClaimedNotificationDelivery {
        id: row
            .try_get("id")
            .map_err(|_| postgres_unavailable("decode delivery"))?,
        group_id,
        condition: row
            .try_get::<Option<String>, _>("condition_kind")
            .ok()
            .flatten()
            .as_deref()
            .and_then(AlertConditionKind::parse),
        channel,
        target: row
            .try_get("target")
            .map_err(|_| postgres_unavailable("decode delivery"))?,
        subject: row
            .try_get("subject")
            .map_err(|_| postgres_unavailable("decode delivery"))?,
        body: row
            .try_get("body")
            .map_err(|_| postgres_unavailable("decode delivery"))?,
        bark_level: BarkLevel::parse(
            &row.try_get::<String, _>("bark_level")
                .map_err(|_| postgres_unavailable("decode delivery"))?,
        )
        .ok_or_else(|| postgres_unavailable("decode delivery level"))?,
        bark_sound: row
            .try_get("bark_sound")
            .map_err(|_| postgres_unavailable("decode delivery"))?,
        bark_volume: row
            .try_get::<i64, _>("bark_volume")
            .ok()
            .and_then(|value| u8::try_from(value).ok())
            .ok_or_else(|| postgres_unavailable("decode delivery volume"))?,
        bark_call: row
            .try_get("bark_call")
            .map_err(|_| postgres_unavailable("decode delivery"))?,
        test: row
            .try_get("test")
            .map_err(|_| postgres_unavailable("decode delivery"))?,
    })
}

fn delivery_record_from_row(
    row: &sqlx::postgres::PgRow,
) -> StoreResult<NotificationDeliveryRecord> {
    Ok(NotificationDeliveryRecord {
        id: row
            .try_get("id")
            .map_err(|_| postgres_unavailable("decode delivery record"))?,
        group_id: row
            .try_get("group_id")
            .map_err(|_| postgres_unavailable("decode delivery record"))?,
        channel: row
            .try_get("channel")
            .map_err(|_| postgres_unavailable("decode delivery record"))?,
        target: row
            .try_get("target")
            .map_err(|_| postgres_unavailable("decode delivery record"))?,
        status: row
            .try_get("status")
            .map_err(|_| postgres_unavailable("decode delivery record"))?,
        test: row
            .try_get("test")
            .map_err(|_| postgres_unavailable("decode delivery record"))?,
        attempts: row
            .try_get::<i64, _>("attempts")
            .ok()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| postgres_unavailable("decode delivery attempts"))?,
        error: row
            .try_get("error_summary")
            .map_err(|_| postgres_unavailable("decode delivery record"))?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| postgres_unavailable("decode delivery record"))?,
        finished_at: row
            .try_get("finished_at")
            .map_err(|_| postgres_unavailable("decode delivery record"))?,
    })
}
