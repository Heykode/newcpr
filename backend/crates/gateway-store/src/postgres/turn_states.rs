//! PostgreSQL owner for opaque Provider turn-state values.

use std::time::SystemTime;

use chrono::{DateTime, Utc};
use gateway_core::{
    account::{CredentialRevision, ProviderAccountId},
    provider_ports::{
        OpaqueTurnState, ProviderStoreError, ProviderStoreErrorKind, ProviderTurnStateAnomaly,
        ProviderTurnStateCandidate, ProviderTurnStatePort, ProviderTurnStatePromotion,
        ProviderTurnStateRecord, ProviderTurnStateRefreshStatus, ProviderTurnStateSlot,
        ProviderTurnStateValue,
    },
    routing::UpstreamModelId,
};
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct PgProviderTurnStateRepository {
    pool: PgPool,
}

impl PgProviderTurnStateRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct TurnStateRow {
    provider_account_id: String,
    upstream_model: String,
    normal_length: i16,
    active_state: Option<String>,
    active_issued_at: Option<DateTime<Utc>>,
    active_expires_at: Option<DateTime<Utc>>,
    standby_state: Option<String>,
    standby_issued_at: Option<DateTime<Utc>>,
    standby_expires_at: Option<DateTime<Utc>>,
    state_version: i64,
    refresh_status: String,
    last_observed_length: Option<i16>,
}

impl ProviderTurnStatePort for PgProviderTurnStateRepository {
    fn promote_standby(
        &self,
        promotion: ProviderTurnStatePromotion,
    ) -> futures::future::BoxFuture<'_, Result<Option<ProviderTurnStateRecord>, ProviderStoreError>>
    {
        Box::pin(async move {
            validate_normal_length(promotion.normal_length)?;
            let deadline = promotion
                .observed_at
                .checked_add(promotion.minimum_remaining)
                .ok_or_else(|| invalid("turn state promotion deadline"))?;
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(|_| unavailable("begin provider turn state promotion"))?;
            ensure_row(
                &mut transaction,
                &promotion.account_id,
                &promotion.upstream_model,
                promotion.expected_revision,
                promotion.normal_length,
                promotion.observed_at,
            )
            .await?;
            let current = load_locked(
                &mut transaction,
                &promotion.account_id,
                &promotion.upstream_model,
            )
            .await?;
            if u64::try_from(current.state_version).ok() != Some(promotion.expected_active_version)
                || current
                    .active_expires_at
                    .is_some_and(|expires| SystemTime::from(expires) > deadline)
                || !current
                    .standby_issued_at
                    .is_some_and(|issued| SystemTime::from(issued) <= promotion.observed_at)
                || !current
                    .standby_expires_at
                    .is_some_and(|expires| SystemTime::from(expires) > deadline)
            {
                return Ok(None);
            }
            sqlx::query(
                "update provider_turn_states
                 set active_state = standby_state, active_issued_at = standby_issued_at,
                     active_expires_at = standby_expires_at,
                     standby_state = null, standby_issued_at = null, standby_expires_at = null,
                     state_version = state_version + 1, refresh_status = 'refreshing',
                     updated_at = $3
                 where provider_account_id = $1 and upstream_model = $2",
            )
            .bind(promotion.account_id.as_str())
            .bind(promotion.upstream_model.as_str())
            .bind(DateTime::<Utc>::from(promotion.observed_at))
            .execute(&mut *transaction)
            .await
            .map_err(|_| unavailable("promote provider turn state standby"))?;
            let row = load_locked(
                &mut transaction,
                &promotion.account_id,
                &promotion.upstream_model,
            )
            .await?;
            transaction
                .commit()
                .await
                .map_err(|_| unavailable("commit provider turn state promotion"))?;
            decode_row(row).map(Some)
        })
    }

    fn cancel_refresh<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
        upstream_model: &'a UpstreamModelId,
        expected_revision: CredentialRevision,
    ) -> futures::future::BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            // Cancellation must work after opt-out, but must not touch a newer credential's task.
            sqlx::query(
                "update provider_turn_states set refresh_status = 'failed', updated_at = now()
                 where provider_account_id = $1 and upstream_model = $2
                   and credential_revision = $3 and refresh_status = 'refreshing'",
            )
            .bind(account_id.as_str())
            .bind(upstream_model.as_str())
            .bind(revision_value(expected_revision)?)
            .execute(&self.pool)
            .await
            .map_err(|_| unavailable("cancel provider turn state refresh"))?;
            Ok(())
        })
    }

    fn read<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
        upstream_model: &'a UpstreamModelId,
        expected_revision: CredentialRevision,
    ) -> futures::future::BoxFuture<'a, Result<Option<ProviderTurnStateRecord>, ProviderStoreError>>
    {
        Box::pin(async move {
            let row = sqlx::query_as::<_, TurnStateRow>(
                "select provider_account_id, upstream_model, normal_length,
                        active_state, active_issued_at, active_expires_at,
                        standby_state, standby_issued_at, standby_expires_at,
                        state_version, refresh_status, last_observed_length
                 from provider_turn_states s
                 where provider_account_id = $1 and upstream_model = $2
                   and s.credential_revision = $3
                   and exists (select 1 from provider_accounts a
                     where a.id = s.provider_account_id and a.enabled
                       and a.turn_state_injection_enabled
                       and a.credential_state = 'ready'
                       and (a.access_token_expires_at is null or a.access_token_expires_at > now())
                       and a.quota_access_state <> 'exhausted'
                       and a.turn_state_binding_revision = s.credential_revision)
                   and exists (select 1 from runtime_settings r
                     where r.turn_state_injection_enabled
                       and s.upstream_model = any(r.turn_state_models))",
            )
            .bind(account_id.as_str())
            .bind(upstream_model.as_str())
            .bind(revision_value(expected_revision)?)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| unavailable("read provider turn state"))?;
            row.map(decode_row).transpose()
        })
    }

    fn put_candidate(
        &self,
        mut candidate: ProviderTurnStateCandidate,
    ) -> futures::future::BoxFuture<'_, Result<ProviderTurnStateRecord, ProviderStoreError>> {
        Box::pin(async move {
            validate_candidate(&candidate)?;
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(|_| unavailable("begin provider turn state update"))?;
            ensure_row(
                &mut transaction,
                &candidate.account_id,
                &candidate.upstream_model,
                candidate.expected_revision,
                candidate.normal_length,
                candidate.observed_at,
            )
            .await?;
            let current = load_locked(
                &mut transaction,
                &candidate.account_id,
                &candidate.upstream_model,
            )
            .await?;
            if candidate
                .expected_active_version
                .is_some_and(|version| u64::try_from(current.state_version).ok() != Some(version))
            {
                return decode_row(current);
            }
            // A captured standby echoed by a probe/response keeps its original lifetime.
            if current.standby_state.as_deref()
                == Some(candidate.value.state().expose_to_provider())
                && let (Some(issued), Some(expires)) =
                    (current.standby_issued_at, current.standby_expires_at)
            {
                candidate.value = ProviderTurnStateValue::new(
                    candidate.value.state().clone(),
                    issued.into(),
                    expires.into(),
                );
                if !candidate.value.is_valid_at(candidate.observed_at) {
                    return decode_row(current);
                }
            }
            let candidate_value = candidate.value.state().expose_to_provider();
            let (same_value, replace) = match candidate.slot {
                ProviderTurnStateSlot::Active => (
                    current.active_state.as_deref() == Some(candidate_value),
                    current.active_issued_at.is_none_or(|issued| {
                        candidate.value.issued_at() >= SystemTime::from(issued)
                    }),
                ),
                ProviderTurnStateSlot::Standby => (
                    current.active_state.as_deref() == Some(candidate_value)
                        || current.standby_state.as_deref() == Some(candidate_value),
                    current.standby_issued_at.is_none_or(|issued| {
                        candidate.value.issued_at() >= SystemTime::from(issued)
                    }),
                ),
            };
            if !same_value && replace {
                update_candidate(&mut transaction, &candidate).await?;
            } else {
                touch_success(&mut transaction, &candidate).await?;
            }
            let row = load_locked(
                &mut transaction,
                &candidate.account_id,
                &candidate.upstream_model,
            )
            .await?;
            transaction
                .commit()
                .await
                .map_err(|_| unavailable("commit provider turn state update"))?;
            decode_row(row)
        })
    }

    fn record_anomaly(
        &self,
        anomaly: ProviderTurnStateAnomaly,
    ) -> futures::future::BoxFuture<'_, Result<ProviderTurnStateRecord, ProviderStoreError>> {
        Box::pin(async move {
            validate_normal_length(anomaly.normal_length)?;
            if anomaly
                .observed_length
                .is_some_and(|length| !(1..=4096).contains(&length))
            {
                return Err(invalid("turn state observed length"));
            }
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(|_| unavailable("begin provider turn state anomaly"))?;
            ensure_row(
                &mut transaction,
                &anomaly.account_id,
                &anomaly.upstream_model,
                anomaly.expected_revision,
                anomaly.normal_length,
                anomaly.observed_at,
            )
            .await?;
            let current = load_locked(
                &mut transaction,
                &anomaly.account_id,
                &anomaly.upstream_model,
            )
            .await?;
            if u64::try_from(current.state_version).ok() != Some(anomaly.expected_active_version) {
                return decode_row(current);
            }
            let promote = anomaly.promote_standby
                && current
                    .standby_issued_at
                    .is_some_and(|issued| SystemTime::from(issued) <= anomaly.observed_at)
                && current
                    .standby_expires_at
                    .is_some_and(|expires| SystemTime::from(expires) > anomaly.observed_at);
            sqlx::query(
                "update provider_turn_states
                 set active_state = case when $4 then standby_state when $6 then null else active_state end,
                     active_issued_at = case when $4 then standby_issued_at when $6 then null else active_issued_at end,
                     active_expires_at = case when $4 then standby_expires_at when $6 then null else active_expires_at end,
                     standby_state = case when $4 then null else standby_state end,
                     standby_issued_at = case when $4 then null else standby_issued_at end,
                     standby_expires_at = case when $4 then null else standby_expires_at end,
                     state_version = state_version + case when $4 or ($6 and active_state is not null) then 1 else 0 end,
                     refresh_status = 'refreshing', last_observed_length = $3,
                     last_probe_at = $5, updated_at = $5
                 where provider_account_id = $1 and upstream_model = $2",
            )
            .bind(anomaly.account_id.as_str())
            .bind(anomaly.upstream_model.as_str())
            .bind(anomaly.observed_length.map(|value| i16::try_from(value).unwrap_or(i16::MAX)))
            .bind(promote)
            .bind(DateTime::<Utc>::from(anomaly.observed_at))
            .bind(anomaly.promote_standby)
            .execute(&mut *transaction)
            .await
            .map_err(|_| unavailable("record provider turn state anomaly"))?;
            let row = load_locked(
                &mut transaction,
                &anomaly.account_id,
                &anomaly.upstream_model,
            )
            .await?;
            transaction
                .commit()
                .await
                .map_err(|_| unavailable("commit provider turn state anomaly"))?;
            decode_row(row)
        })
    }

    fn mark_refresh_status<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
        upstream_model: &'a UpstreamModelId,
        expected_revision: CredentialRevision,
        normal_length: u16,
        status: ProviderTurnStateRefreshStatus,
        observed_at: SystemTime,
    ) -> futures::future::BoxFuture<'a, Result<ProviderTurnStateRecord, ProviderStoreError>> {
        Box::pin(async move {
            validate_normal_length(normal_length)?;
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(|_| unavailable("begin provider turn state refresh"))?;
            ensure_row(
                &mut transaction,
                account_id,
                upstream_model,
                expected_revision,
                normal_length,
                observed_at,
            )
            .await?;
            let observed_at = DateTime::<Utc>::from(observed_at);
            sqlx::query(
                "insert into provider_turn_states
                   (provider_account_id, upstream_model, normal_length, refresh_status,
                    last_probe_at, updated_at)
                 values ($1, $2, $3, $4, $5, $5)
                 on conflict (provider_account_id, upstream_model) do update set
                   normal_length = excluded.normal_length,
                   active_state = case
                     when provider_turn_states.normal_length <> excluded.normal_length then null
                     else provider_turn_states.active_state end,
                   active_issued_at = case
                     when provider_turn_states.normal_length <> excluded.normal_length then null
                     else provider_turn_states.active_issued_at end,
                   active_expires_at = case
                     when provider_turn_states.normal_length <> excluded.normal_length then null
                     else provider_turn_states.active_expires_at end,
                   standby_state = case
                     when provider_turn_states.normal_length <> excluded.normal_length then null
                     else provider_turn_states.standby_state end,
                   standby_issued_at = case
                     when provider_turn_states.normal_length <> excluded.normal_length then null
                     else provider_turn_states.standby_issued_at end,
                   standby_expires_at = case
                     when provider_turn_states.normal_length <> excluded.normal_length then null
                     else provider_turn_states.standby_expires_at end,
                   state_version = provider_turn_states.state_version + case
                     when provider_turn_states.normal_length <> excluded.normal_length then 1
                     else 0 end,
                   last_observed_length = case
                     when provider_turn_states.normal_length <> excluded.normal_length then null
                     else provider_turn_states.last_observed_length end,
                   refresh_status = excluded.refresh_status,
                   last_probe_at = excluded.last_probe_at,
                   updated_at = excluded.updated_at",
            )
            .bind(account_id.as_str())
            .bind(upstream_model.as_str())
            .bind(i16::try_from(normal_length).map_err(|_| invalid("turn state length"))?)
            .bind(status.as_str())
            .bind(observed_at)
            .execute(&mut *transaction)
            .await
            .map_err(|_| unavailable("mark provider turn state refresh"))?;
            let row = load_locked(&mut transaction, account_id, upstream_model).await?;
            transaction
                .commit()
                .await
                .map_err(|_| unavailable("commit provider turn state refresh"))?;
            decode_row(row)
        })
    }
}

async fn ensure_row(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: &ProviderAccountId,
    upstream_model: &UpstreamModelId,
    expected_revision: CredentialRevision,
    normal_length: u16,
    observed_at: SystemTime,
) -> Result<(), ProviderStoreError> {
    // Match the control-plane lock order: runtime policy, account, then state.
    let allowed = sqlx::query_scalar::<_, bool>(
        "select turn_state_injection_enabled and $1 = any(turn_state_models)
         from runtime_settings for share",
    )
    .bind(upstream_model.as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| unavailable("lock turn state policy"))?
    .unwrap_or(false);
    let owner = sqlx::query_scalar::<_, bool>(
        "select enabled and turn_state_injection_enabled and turn_state_binding_revision = $2
           and credential_state = 'ready'
           and (access_token_expires_at is null or access_token_expires_at > now())
           and quota_access_state <> 'exhausted'
         from provider_accounts where id = $1 for share",
    )
    .bind(account_id.as_str())
    .bind(revision_value(expected_revision)?)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| unavailable("lock turn state owner"))?
    .unwrap_or(false);
    if !allowed || !owner {
        return Err(unavailable("turn state policy or owner changed"));
    }
    sqlx::query(
        "insert into provider_turn_states
           (provider_account_id, upstream_model, normal_length, last_probe_at, updated_at, credential_revision)
         values ($1, $2, $3, $4, $4, $5)
         on conflict (provider_account_id, upstream_model) do update set
           credential_revision = excluded.credential_revision,
           normal_length = excluded.normal_length,
           active_state = null, active_issued_at = null, active_expires_at = null,
           standby_state = null, standby_issued_at = null, standby_expires_at = null,
           state_version = provider_turn_states.state_version + 1,
           refresh_status = 'missing', last_observed_length = null,
           last_probe_at = excluded.last_probe_at, updated_at = excluded.updated_at
         where provider_turn_states.normal_length <> excluded.normal_length
            or provider_turn_states.credential_revision <> excluded.credential_revision",
    )
    .bind(account_id.as_str())
    .bind(upstream_model.as_str())
    .bind(i16::try_from(normal_length).map_err(|_| invalid("turn state length"))?)
    .bind(DateTime::<Utc>::from(observed_at))
    .bind(revision_value(expected_revision)?)
    .execute(&mut **transaction)
    .await
    .map_err(|_| unavailable("ensure provider turn state"))?;
    Ok(())
}

fn revision_value(revision: CredentialRevision) -> Result<i64, ProviderStoreError> {
    i64::try_from(revision.get()).map_err(|_| invalid("turn state credential revision"))
}

async fn load_locked(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: &ProviderAccountId,
    upstream_model: &UpstreamModelId,
) -> Result<TurnStateRow, ProviderStoreError> {
    sqlx::query_as::<_, TurnStateRow>(
        "select provider_account_id, upstream_model, normal_length,
                active_state, active_issued_at, active_expires_at,
                standby_state, standby_issued_at, standby_expires_at,
                state_version, refresh_status, last_observed_length
         from provider_turn_states
         where provider_account_id = $1 and upstream_model = $2 for update",
    )
    .bind(account_id.as_str())
    .bind(upstream_model.as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| unavailable("lock provider turn state"))?
    .ok_or_else(|| unavailable("provider turn state disappeared"))
}

async fn update_candidate(
    transaction: &mut Transaction<'_, Postgres>,
    candidate: &ProviderTurnStateCandidate,
) -> Result<(), ProviderStoreError> {
    let (active, standby) = match candidate.slot {
        ProviderTurnStateSlot::Active => (true, false),
        ProviderTurnStateSlot::Standby => (false, true),
    };
    sqlx::query(
        "update provider_turn_states
         set normal_length = $3,
             active_state = case when $4 then $6 else active_state end,
             active_issued_at = case when $4 then $7 else active_issued_at end,
             active_expires_at = case when $4 then $8 else active_expires_at end,
             standby_state = case
               when $4 and active_state is not null and active_state <> $6
                    and active_expires_at > $9 then active_state
               when $5 then $6 else standby_state end,
             standby_issued_at = case
               when $4 and active_state is not null and active_state <> $6
                    and active_expires_at > $9 then active_issued_at
               when $5 then $7 else standby_issued_at end,
             standby_expires_at = case
               when $4 and active_state is not null and active_state <> $6
                    and active_expires_at > $9 then active_expires_at
               when $5 then $8 else standby_expires_at end,
             state_version = state_version + case when $4 then 1 else 0 end,
             refresh_status = case when $4 then 'refreshing' else 'ready' end, last_observed_length = $3,
             last_probe_at = $9, last_success_at = $9, updated_at = $9
         where provider_account_id = $1 and upstream_model = $2",
    )
    .bind(candidate.account_id.as_str())
    .bind(candidate.upstream_model.as_str())
    .bind(i16::try_from(candidate.normal_length).map_err(|_| invalid("turn state length"))?)
    .bind(active)
    .bind(standby)
    .bind(candidate.value.state().expose_to_provider())
    .bind(DateTime::<Utc>::from(candidate.value.issued_at()))
    .bind(DateTime::<Utc>::from(candidate.value.expires_at()))
    .bind(DateTime::<Utc>::from(candidate.observed_at))
    .execute(&mut **transaction)
    .await
    .map_err(|_| unavailable("update provider turn state candidate"))?;
    Ok(())
}

async fn touch_success(
    transaction: &mut Transaction<'_, Postgres>,
    candidate: &ProviderTurnStateCandidate,
) -> Result<(), ProviderStoreError> {
    sqlx::query(
        "update provider_turn_states
         set last_observed_length = $3,
             last_probe_at = $4, last_success_at = $4, updated_at = $4
         where provider_account_id = $1 and upstream_model = $2",
    )
    .bind(candidate.account_id.as_str())
    .bind(candidate.upstream_model.as_str())
    .bind(i16::try_from(candidate.normal_length).map_err(|_| invalid("turn state length"))?)
    .bind(DateTime::<Utc>::from(candidate.observed_at))
    .execute(&mut **transaction)
    .await
    .map_err(|_| unavailable("touch provider turn state candidate"))?;
    Ok(())
}

fn decode_row(row: TurnStateRow) -> Result<ProviderTurnStateRecord, ProviderStoreError> {
    let active = decode_value(
        row.active_state,
        row.active_issued_at,
        row.active_expires_at,
    )?;
    let standby = decode_value(
        row.standby_state,
        row.standby_issued_at,
        row.standby_expires_at,
    )?;
    Ok(ProviderTurnStateRecord::new(
        ProviderAccountId::new(row.provider_account_id)
            .map_err(|_| invalid("turn state account"))?,
        UpstreamModelId::new(row.upstream_model).map_err(|_| invalid("turn state model"))?,
        u16::try_from(row.normal_length).map_err(|_| invalid("turn state length"))?,
        active,
        standby,
        u64::try_from(row.state_version).map_err(|_| invalid("turn state version"))?,
        ProviderTurnStateRefreshStatus::parse(&row.refresh_status)
            .ok_or_else(|| invalid("turn state refresh status"))?,
        row.last_observed_length
            .map(u16::try_from)
            .transpose()
            .map_err(|_| invalid("turn state observed length"))?,
    ))
}

fn decode_value(
    state: Option<String>,
    issued_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
) -> Result<Option<ProviderTurnStateValue>, ProviderStoreError> {
    match (state, issued_at, expires_at) {
        (None, None, None) => Ok(None),
        (Some(state), Some(issued_at), Some(expires_at)) => Ok(Some(ProviderTurnStateValue::new(
            OpaqueTurnState::new(state),
            issued_at.into(),
            expires_at.into(),
        ))),
        _ => Err(invalid("incomplete provider turn state")),
    }
}

fn validate_normal_length(value: u16) -> Result<(), ProviderStoreError> {
    if matches!(value, 292 | 332) {
        Ok(())
    } else {
        Err(invalid("turn state length"))
    }
}

fn validate_candidate(candidate: &ProviderTurnStateCandidate) -> Result<(), ProviderStoreError> {
    validate_normal_length(candidate.normal_length)?;
    if candidate.value.state().expose_to_provider().len() != usize::from(candidate.normal_length)
        || candidate.value.issued_at() >= candidate.value.expires_at()
        || !candidate.value.is_valid_at(candidate.observed_at)
    {
        return Err(invalid("turn state candidate"));
    }
    Ok(())
}

fn unavailable(operation: &'static str) -> ProviderStoreError {
    ProviderStoreError::new(ProviderStoreErrorKind::Unavailable, operation)
}

fn invalid(operation: &'static str) -> ProviderStoreError {
    ProviderStoreError::new(ProviderStoreErrorKind::InvalidData, operation)
}
