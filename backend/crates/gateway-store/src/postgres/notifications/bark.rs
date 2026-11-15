use chrono::{DateTime, Utc};
use gateway_admin::model::notifications::{
    AlertObservation, ClaimedNotificationDelivery, NotificationChannelKind, bark_delivery_key,
};
use sqlx::Row as _;

use crate::{StoreResult, postgres_unavailable};

/// The caller holds the group lock. Only rows belonging to the current incident are read.
pub(super) async fn enqueue_current(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    observation: &AlertObservation,
    activated_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> StoreResult<()> {
    let rows = sqlx::query(
        "select o.* from account_group_alert_incidents i
         join notification_outbox o on o.group_id=i.group_id and o.condition_kind=i.condition_kind
           and o.incident_activated_at=i.activated_at
         where i.group_id=$1 and i.activated_at is not null and o.channel='bark' and not o.test
           and o.status <> 'cancelled'
         for update of o",
    )
    .bind(observation.group_id.as_str())
    .fetch_all(&mut **tx)
    .await
    .map_err(|_| postgres_unavailable("load current Bark incident deliveries"))?;
    let key = observation.bark_rule_key.clone().unwrap_or_else(|| {
        bark_delivery_key(
            "default",
            observation.bark_level,
            observation.bark_sound.as_deref(),
            observation.bark_volume,
            observation.bark_call,
        )
    });
    let strength = observation
        .bark_level
        .delivery_strength(observation.bark_volume, observation.bark_call);
    let mut blocked = false;
    for row in &rows {
        let previous = super::claimed_from_row(row)?;
        let previous_key = previous.bark_rule_key.clone().unwrap_or_else(|| {
            bark_delivery_key(
                "default",
                previous.bark_level,
                previous.bark_sound.as_deref(),
                previous.bark_volume,
                previous.bark_call,
            )
        });
        let status: String = row
            .try_get("status")
            .map_err(|_| postgres_unavailable("decode Bark delivery status"))?;
        if status == "sending"
            || (status == "sent"
                && previous
                    .bark_level
                    .delivery_strength(previous.bark_volume, previous.bark_call)
                    >= strength)
            || (matches!(status.as_str(), "pending" | "failed") && previous_key == key)
        {
            blocked = true;
        }
        // An unsent reminder cannot leak into a different schedule. Cancellation is not
        // successful delivery, so returning to that period may enqueue it again.
        if status == "pending" && previous_key != key {
            sqlx::query(
                "update notification_outbox set status='cancelled',finished_at=$2,
                         error_summary='Bark reminder rule superseded' where id=$1",
            )
            .bind(&previous.id)
            .bind(now)
            .execute(&mut **tx)
            .await
            .map_err(|_| postgres_unavailable("supersede queued Bark reminder"))?;
        }
    }
    if blocked {
        return Ok(());
    }
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
        bark_rule_key: Some(key),
        incident_activated_at: Some(activated_at),
        test: false,
    };
    super::insert_delivery(&mut **tx, &delivery, now).await
}
