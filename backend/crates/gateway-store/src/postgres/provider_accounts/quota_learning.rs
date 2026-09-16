use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Utc};
use gateway_admin::{
    model::quota_learning::{QuotaLearningEstimate, QuotaLearningObservation, QuotaLearningSource},
    ports::store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
};
use sqlx::{PgPool, Postgres, Row, Transaction};

const MIN_COST_DELTA_USD: f64 = 5.0;
const MIN_PERCENT_DELTA: f64 = 3.0;
const RESET_JITTER_SECONDS: i64 = 2;
const RETAINED_SAMPLES: i64 = 3;

pub(super) async fn record(
    pool: &PgPool,
    observations: &[QuotaLearningObservation],
) -> AdminStoreResult<Vec<QuotaLearningEstimate>> {
    if observations.is_empty() {
        return Ok(Vec::new());
    }
    let mut transaction = pool.begin().await.map_err(unavailable)?;
    sqlx::query("set local statement_timeout = '2s'")
        .execute(&mut *transaction)
        .await
        .map_err(unavailable)?;
    sqlx::query("set local lock_timeout = '1s'")
        .execute(&mut *transaction)
        .await
        .map_err(unavailable)?;
    let result = record_in_transaction(&mut transaction, observations).await;
    match result {
        Ok(result) => {
            transaction.commit().await.map_err(unavailable)?;
            Ok(result)
        }
        Err(error) => {
            transaction.rollback().await.map_err(unavailable)?;
            Err(error)
        }
    }
}

async fn record_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    observations: &[QuotaLearningObservation],
) -> AdminStoreResult<Vec<QuotaLearningEstimate>> {
    let observations = observations
        .iter()
        .filter(|observation| valid_observation(observation))
        .collect::<Vec<_>>();
    // Serialize first observations and rolling sample updates, including across
    // processes. Sorted keys keep multi-account/window batches deadlock-free.
    let mut locks = BTreeSet::new();
    for observation in &observations {
        locks.insert(format!("account:{}", observation.account_id));
        locks.insert(format!(
            "plan:{}",
            serde_json::json!([
                observation.provider_kind,
                observation.plan_type.trim().to_ascii_lowercase(),
                observation.window_key,
                observation.window_minutes,
            ])
        ));
    }
    for key in locks {
        sqlx::query(
            "select pg_advisory_xact_lock(
                hashtextextended(current_schema() || ':cpr.quota-learning:' || $1, 0))",
        )
        .bind(key)
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?;
    }
    let observations = invalidate_changed_accounts(transaction, &observations).await?;
    let mut estimates = Vec::with_capacity(observations.len());
    for observation in observations {
        let observed_cost_usd = observation.observed_cost_usd;
        let existing = sqlx::query(
            "select provider_kind, plan_type, baseline_cost_usd,
                    baseline_percent, baseline_reset_at,
                    bound_usd, bound_at, last_observed_at, last_percent
               from quota_learning_accounts
              where provider_account_id = $1 and window_key = $2 and window_minutes = $3
              for update",
        )
        .bind(&observation.account_id)
        .bind(&observation.window_key)
        .bind(observation.window_minutes)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(unavailable)?;

        let mut baseline_cost = observed_cost_usd;
        let mut baseline_percent = observation.used_percent;
        let mut baseline_reset = Some(observation.reset_at);
        let mut bound_usd = None;
        let mut bound_at = None;
        let mut stale = false;
        if let Some(row) = existing {
            let provider: String = row.try_get("provider_kind").map_err(invalid)?;
            let plan: String = row.try_get("plan_type").map_err(invalid)?;
            baseline_cost = row.try_get("baseline_cost_usd").map_err(invalid)?;
            baseline_percent = row.try_get("baseline_percent").map_err(invalid)?;
            baseline_reset = row.try_get("baseline_reset_at").map_err(invalid)?;
            bound_usd = row.try_get("bound_usd").map_err(invalid)?;
            bound_at = row.try_get("bound_at").map_err(invalid)?;
            let last_observed: DateTime<Utc> = row.try_get("last_observed_at").map_err(invalid)?;
            let last_percent: f64 = row.try_get("last_percent").map_err(invalid)?;
            stale = observation.observed_at.timestamp_micros() <= last_observed.timestamp_micros();
            if provider != observation.provider_kind
                || !plan.eq_ignore_ascii_case(observation.plan_type.trim())
            {
                // An out-of-order snapshot must not borrow or overwrite a
                // binding from the account's newer Plan.
                bound_usd = None;
                bound_at = None;
                stale = true;
            }
            let rebuild_baseline = baseline_reset
                .map(|value| {
                    (value - observation.reset_at).abs() > Duration::seconds(RESET_JITTER_SECONDS)
                })
                .unwrap_or(false)
                || baseline_cost.is_none()
                || observed_cost_usd
                    .zip(baseline_cost)
                    .is_some_and(|(current, baseline)| current < baseline)
                || observation.used_percent < last_percent - 1.0;
            if !stale && rebuild_baseline {
                baseline_cost = observed_cost_usd;
                baseline_percent = observation.used_percent;
                baseline_reset = Some(observation.reset_at);
            }
        }

        if bound_usd.is_none()
            && !stale
            && let Some((observed_cost, baseline_cost)) = observed_cost_usd.zip(baseline_cost)
        {
            let cost_delta = observed_cost - baseline_cost;
            let percent_delta = observation.used_percent - baseline_percent;
            if cost_delta >= MIN_COST_DELTA_USD && percent_delta >= MIN_PERCENT_DELTA {
                let estimate = cost_delta / (percent_delta / 100.0);
                if estimate.is_finite() && estimate > 0.0 {
                    bound_usd = Some(estimate);
                }
            }
        }

        let newly_bound = bound_usd.is_some() && bound_at.is_none();
        if newly_bound {
            bound_at = Some(Utc::now());
        }
        if !stale {
            sqlx::query(
                "insert into quota_learning_accounts (
                provider_account_id, provider_kind, plan_type, window_key,
                window_minutes, baseline_cost_usd, baseline_percent,
                baseline_reset_at, bound_usd, bound_at, last_percent,
                last_reset_at, last_observed_at, updated_at
             ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, now())
             on conflict (provider_account_id, window_key, window_minutes) do update set
                provider_kind = excluded.provider_kind,
                plan_type = excluded.plan_type,
                window_minutes = excluded.window_minutes,
                baseline_cost_usd = excluded.baseline_cost_usd,
                baseline_percent = excluded.baseline_percent,
                baseline_reset_at = excluded.baseline_reset_at,
                bound_usd = excluded.bound_usd,
                bound_at = excluded.bound_at,
                last_percent = excluded.last_percent,
                last_reset_at = excluded.last_reset_at,
                last_observed_at = excluded.last_observed_at,
                updated_at = now()",
            )
            .bind(&observation.account_id)
            .bind(&observation.provider_kind)
            .bind(observation.plan_type.trim())
            .bind(&observation.window_key)
            .bind(observation.window_minutes)
            .bind(baseline_cost)
            .bind(baseline_percent)
            .bind(baseline_reset)
            .bind(bound_usd)
            .bind(bound_at)
            .bind(observation.used_percent)
            .bind(observation.reset_at)
            .bind(observation.observed_at)
            .execute(&mut **transaction)
            .await
            .map_err(unavailable)?;
        }

        if newly_bound {
            let bound_usd = bound_usd.expect("newly_bound implies a bound value");
            sqlx::query(
                "insert into quota_learning_plan_samples (
                    provider_kind, plan_type, window_key, window_minutes,
                    bound_usd, source_account_id, bound_at
                 ) values ($1, $2, $3, $4, $5, $6, $7)
                 on conflict (provider_kind, source_account_id, window_key, window_minutes)
                 do update set
                    plan_type = excluded.plan_type,
                    window_minutes = excluded.window_minutes,
                    bound_usd = excluded.bound_usd,
                    bound_at = excluded.bound_at",
            )
            .bind(&observation.provider_kind)
            .bind(observation.plan_type.trim())
            .bind(&observation.window_key)
            .bind(observation.window_minutes)
            .bind(bound_usd)
            .bind(&observation.account_id)
            .bind(bound_at)
            .execute(&mut **transaction)
            .await
            .map_err(unavailable)?;
            sqlx::query(
                "delete from quota_learning_plan_samples where id in (
                    select id from quota_learning_plan_samples
                     where provider_kind = $1 and lower(plan_type) = lower($2)
                       and window_key = $3 and window_minutes = $4
                     order by bound_at desc, id desc offset $5
                 )",
            )
            .bind(&observation.provider_kind)
            .bind(observation.plan_type.trim())
            .bind(&observation.window_key)
            .bind(observation.window_minutes)
            .bind(RETAINED_SAMPLES)
            .execute(&mut **transaction)
            .await
            .map_err(unavailable)?;
        }

        let (plan_average, sample_count) = load_plan_average(transaction, observation).await?;
        let (effective_limit_usd, source) = match bound_usd.or(plan_average) {
            Some(limit) if bound_usd.is_some() => (Some(limit), QuotaLearningSource::Personal),
            Some(limit) => (Some(limit), QuotaLearningSource::PlanAverage),
            None => (None, QuotaLearningSource::Learning),
        };
        estimates.push(QuotaLearningEstimate {
            account_id: observation.account_id.clone(),
            window_key: observation.window_key.clone(),
            window_minutes: observation.window_minutes,
            effective_limit_usd,
            source,
            sample_count,
        });
    }
    Ok(estimates)
}

async fn invalidate_changed_accounts<'a>(
    transaction: &mut Transaction<'_, Postgres>,
    observations: &[&'a QuotaLearningObservation],
) -> AdminStoreResult<Vec<&'a QuotaLearningObservation>> {
    let accounts = observations
        .iter()
        .map(|observation| &observation.account_id)
        .collect::<BTreeSet<_>>();
    let now = Utc::now();
    let mut accepted = Vec::with_capacity(observations.len());
    for account_id in accounts {
        let rows = sqlx::query(
            "select provider_kind, plan_type, window_key, window_minutes,
                    last_percent, last_reset_at, last_observed_at
               from quota_learning_accounts where provider_account_id = $1 for update",
        )
        .bind(account_id)
        .fetch_all(&mut **transaction)
        .await
        .map_err(unavailable)?;
        let latest = rows
            .iter()
            .map(|row| {
                Ok::<_, sqlx::Error>((
                    row.try_get::<DateTime<Utc>, _>("last_observed_at")?,
                    row.try_get::<String, _>("provider_kind")?,
                    row.try_get::<String, _>("plan_type")?,
                ))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(invalid)?
            .into_iter()
            .max_by_key(|(observed_at, _, _)| *observed_at);
        // A late snapshot can contain a window absent from the new Plan. Fence
        // the whole account, not just the row for that particular window.
        let current = observations
            .iter()
            .copied()
            .filter(|observation| &observation.account_id == account_id)
            .filter(|observation| {
                latest.as_ref().is_none_or(|(observed_at, provider, plan)| {
                    let timestamp = observation.observed_at.timestamp_micros();
                    timestamp > observed_at.timestamp_micros()
                        || (timestamp == observed_at.timestamp_micros()
                            && provider == &observation.provider_kind
                            && plan.eq_ignore_ascii_case(observation.plan_type.trim()))
                })
            })
            .collect::<Vec<_>>();
        let mut reset = false;
        for row in rows {
            let provider: String = row.try_get("provider_kind").map_err(invalid)?;
            let plan: String = row.try_get("plan_type").map_err(invalid)?;
            let key: String = row.try_get("window_key").map_err(invalid)?;
            let minutes: i32 = row.try_get("window_minutes").map_err(invalid)?;
            let last_percent: f64 = row.try_get("last_percent").map_err(invalid)?;
            let last_reset: Option<DateTime<Utc>> =
                row.try_get("last_reset_at").map_err(invalid)?;
            let last_observed: DateTime<Utc> = row.try_get("last_observed_at").map_err(invalid)?;
            for observation in current.iter().filter(|observation| {
                observation.observed_at.timestamp_micros() > last_observed.timestamp_micros()
            }) {
                let changed_plan = provider != observation.provider_kind
                    || !plan.eq_ignore_ascii_case(observation.plan_type.trim());
                // As in Tools, an early long-window reset invalidates all of
                // this account's bindings. Ordinary expiry keeps learned limits.
                let early_reset = key == observation.window_key
                    && minutes == observation.window_minutes
                    && minutes >= 7 * 24 * 60
                    && last_reset.is_some_and(|previous| {
                        now < previous - Duration::seconds(RESET_JITTER_SECONDS)
                            && (observation.reset_at
                                > previous + Duration::seconds(RESET_JITTER_SECONDS)
                                || (last_percent > 0.0 && observation.used_percent == 0.0))
                    });
                reset |= changed_plan || early_reset;
            }
        }
        if reset {
            sqlx::query("delete from quota_learning_accounts where provider_account_id = $1")
                .bind(account_id)
                .execute(&mut **transaction)
                .await
                .map_err(unavailable)?;
        }
        accepted.extend(current);
    }
    Ok(accepted)
}

fn valid_observation(observation: &QuotaLearningObservation) -> bool {
    !observation.account_id.is_empty()
        && !observation.provider_kind.is_empty()
        && !observation.plan_type.trim().is_empty()
        && !observation.plan_type.trim().eq_ignore_ascii_case("unknown")
        && !observation.window_key.is_empty()
        && observation.window_minutes > 0
        && observation.used_percent.is_finite()
        && (0.0..=100.0).contains(&observation.used_percent)
        && observation.reset_at > DateTime::<Utc>::UNIX_EPOCH
        && observation.observed_at > DateTime::<Utc>::UNIX_EPOCH
        && observation.observed_at <= Utc::now()
        && observation.observed_at < observation.reset_at
        && observation
            .observed_cost_usd
            .is_none_or(|value| value.is_finite() && value >= 0.0)
}

async fn load_plan_average(
    transaction: &mut Transaction<'_, Postgres>,
    observation: &QuotaLearningObservation,
) -> AdminStoreResult<(Option<f64>, u32)> {
    let samples = sqlx::query(
        "select bound_usd
           from quota_learning_plan_samples
          where provider_kind = $1
            and lower(plan_type) = lower($2)
            and window_key = $3 and window_minutes = $4
          order by bound_at desc, id desc
          limit $5",
    )
    .bind(&observation.provider_kind)
    .bind(observation.plan_type.trim())
    .bind(&observation.window_key)
    .bind(observation.window_minutes)
    .bind(RETAINED_SAMPLES)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let mut total = 0.0;
    for row in &samples {
        total += row.try_get::<f64, _>("bound_usd").map_err(invalid)?;
    }
    let average = (!samples.is_empty()).then_some(total / samples.len() as f64);
    Ok((average, samples.len() as u32))
}

fn unavailable(error: sqlx::Error) -> AdminStoreError {
    let _ = error;
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "quota learning",
        "quota learning persistence unavailable",
    )
}

fn invalid(error: sqlx::Error) -> AdminStoreError {
    let _ = error;
    AdminStoreError::new(
        AdminStoreErrorKind::Invalid,
        "quota learning",
        "invalid quota learning state",
    )
}
