use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use gateway_admin::model::notifications::AlertObservation;
use sqlx::PgPool;

use crate::{StoreResult, postgres_unavailable};

const RECOVERY_SECONDS: i64 = 30;
// Sampling runs every ten seconds. A missed tick cannot prove continuous health.
const MAX_SAMPLE_GAP_SECONDS: i64 = 15;

#[derive(sqlx::FromRow)]
struct Incident {
    condition_kind: String,
    first_active_at: Option<DateTime<Utc>>,
    activated_at: Option<DateTime<Utc>>,
    recovery_started_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}

pub(super) async fn apply_observations(
    pool: &PgPool,
    observations: &[AlertObservation],
    now: DateTime<Utc>,
) -> StoreResult<()> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for observation in observations {
        groups
            .entry(observation.group_id.as_str())
            .or_default()
            .push(observation);
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| postgres_unavailable("begin group alert observation"))?;
    let mut transitions = Vec::new();
    for (group_id, mut observations) in groups {
        // Serialize the entire group, including its first-ever condition rows.
        let exists = sqlx::query("select id from account_groups where id=$1 for update")
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| postgres_unavailable("lock group alert episode"))?;
        if exists.is_none() {
            continue;
        }
        observations.sort_by_key(|observation| observation.kind);
        let incidents = sqlx::query_as::<_, Incident>(
            "select condition_kind,first_active_at,activated_at,recovery_started_at,updated_at
             from account_group_alert_incidents where group_id=$1 order by condition_kind for update",
        )
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| postgres_unavailable("load group alert episode"))?;
        if incidents.iter().any(|incident| incident.updated_at > now) {
            continue;
        }
        let interrupted = incidents.iter().any(|incident| {
            now.signed_duration_since(incident.updated_at).num_seconds() > MAX_SAMPLE_GAP_SECONDS
        });
        let all_healthy = observations.iter().all(|observation| {
            !observation.enabled || (observation.current_value.is_some() && !observation.active)
        }) && incidents.iter().all(|incident| {
            observations
                .iter()
                .any(|observation| observation.kind.as_str() == incident.condition_kind)
        });
        let active: Vec<_> = incidents
            .iter()
            .filter(|incident| incident.activated_at.is_some())
            .collect();
        let recovery_started = if !active.is_empty() && all_healthy {
            Some(
                if interrupted
                    || active
                        .iter()
                        .any(|incident| incident.recovery_started_at.is_none())
                {
                    now
                } else {
                    active
                        .iter()
                        .filter_map(|incident| incident.recovery_started_at)
                        .max()
                        .unwrap_or(now)
                },
            )
        } else {
            None
        };
        let recovered = recovery_started.is_some_and(|started| {
            now.signed_duration_since(started) >= Duration::seconds(RECOVERY_SECONDS)
        });
        if recovered {
            transitions.push((group_id, "all", "recovered"));
        }
        // Unknown or any remaining abnormal condition cancels recovery for the whole group.
        sqlx::query(
            "update account_group_alert_incidents set recovery_started_at=$2
             where group_id=$1",
        )
        .bind(group_id)
        .bind(recovery_started)
        .execute(&mut *tx)
        .await
        .map_err(|_| postgres_unavailable("update group alert recovery"))?;
        let mut confirmed = Vec::new();
        for observation in observations {
            let previous = incidents
                .iter()
                .find(|incident| incident.condition_kind == observation.kind.as_str());
            let mut activated = previous.and_then(|incident| incident.activated_at);
            let mut first = previous.and_then(|incident| incident.first_active_at);
            let is_active =
                observation.enabled && observation.current_value.is_some() && observation.active;
            if recovered {
                first = None;
                activated = None;
            } else if is_active {
                if interrupted && activated.is_none() {
                    first = None;
                }
                let started = *first.get_or_insert(now);
                if activated.is_none()
                    && now.signed_duration_since(started)
                        >= Duration::seconds(i64::from(observation.confirmation_seconds))
                {
                    activated = Some(now);
                    confirmed.push(observation);
                    transitions.push((group_id, observation.kind.as_str(), "activated"));
                }
            } else if activated.is_none() {
                first = None;
            }
            sqlx::query(
                "insert into account_group_alert_incidents
                   (group_id,condition_kind,first_active_at,activated_at,recovery_started_at,updated_at)
                 values($1,$2,$3,$4,$5,$6)
                 on conflict(group_id,condition_kind) do update set
                   first_active_at=excluded.first_active_at,activated_at=excluded.activated_at,
                   recovery_started_at=excluded.recovery_started_at,updated_at=excluded.updated_at",
            )
            .bind(group_id)
            .bind(observation.kind.as_str())
            .bind(first)
            .bind(activated)
            .bind(if recovered { None } else { recovery_started })
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| postgres_unavailable("save group alert incident"))?;
        }
        for observation in confirmed {
            super::enqueue_observation(&mut tx, observation, now).await?;
        }
    }
    tx.commit()
        .await
        .map_err(|_| postgres_unavailable("commit group alert observation"))?;
    // Record committed transitions, without recipient, credentials or message contents.
    for (group_id, condition, state) in transitions {
        tracing::info!(group_id, condition, state, "group alert episode transition");
    }
    Ok(())
}

pub(super) async fn target_enqueued(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    group_id: &str,
    channel: &str,
    target: &str,
) -> StoreResult<bool> {
    sqlx::query_scalar(
        "select exists (
           select 1 from notification_outbox o
           join account_group_alert_incidents i
             on i.group_id=o.group_id and i.condition_kind=o.condition_kind
               and i.activated_at=o.created_at
           where o.group_id=$1 and o.channel=$2 and o.target=$3 and not o.test)",
    )
    .bind(group_id)
    .bind(channel)
    .bind(target)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| postgres_unavailable("check group alert target"))
}
