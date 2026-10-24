use chrono::{DateTime, TimeDelta, Utc};
use gateway_admin::model::codex_quota_overdraft::{
    CodexQuotaOverdraftClaim, CodexQuotaOverdraftOutcome, CodexQuotaOverdraftState,
    CodexQuotaOverdraftWindow,
};
use gateway_core::{
    account::{CredentialRevision, ProviderAccountId},
    routing::{ConfigRevision, ProviderKind, UpstreamModelId},
};
use sqlx::{PgPool, Row};

use crate::{StoreError, StoreResult, postgres_unavailable};

pub(super) async fn load(
    pool: &PgPool,
    account_ids: &[String],
) -> StoreResult<std::collections::BTreeMap<String, CodexQuotaOverdraftState>> {
    if account_ids.is_empty() {
        return Ok(Default::default());
    }
    let rows = sqlx::query(
        "select s.account_id,s.status,s.quota_window,s.probe_model,s.attempts,s.attempt_limit,s.reason_code,
                s.tested_at,s.retry_at,s.recover_at,s.five_hour_started_at,s.five_hour_recover_at,
                s.seven_day_started_at,s.seven_day_recover_at
         from codex_quota_overdraft_states s
         join runtime_settings r on r.id=1 and r.codex_quota_overdraft_enabled and r.config_revision=s.config_revision
         join provider_accounts a on a.id=s.account_id and a.credential_revision=s.credential_revision
         where s.account_id=any($1::text[])",
    )
    .bind(account_ids)
    .fetch_all(pool)
    .await
    .map_err(|error| postgres_unavailable("load quota overdraft states").with_source(error))?;
    let now = Utc::now();
    let mut result = std::collections::BTreeMap::new();
    for row in rows {
        let account_id = row
            .try_get::<String, _>("account_id")
            .map_err(|_| invalid("invalid account ID"))?;
        let status = row
            .try_get::<String, _>("status")
            .map_err(|_| invalid("invalid status"))?;
        let five_started = row
            .try_get::<Option<DateTime<Utc>>, _>("five_hour_started_at")
            .map_err(|_| invalid("invalid five hour start"))?;
        let five_recover = row
            .try_get::<Option<DateTime<Utc>>, _>("five_hour_recover_at")
            .map_err(|_| invalid("invalid five hour recovery"))?;
        let seven_started = row
            .try_get::<Option<DateTime<Utc>>, _>("seven_day_started_at")
            .map_err(|_| invalid("invalid seven day start"))?;
        let seven_recover = row
            .try_get::<Option<DateTime<Utc>>, _>("seven_day_recover_at")
            .map_err(|_| invalid("invalid seven day recovery"))?;
        let active = |started: Option<DateTime<Utc>>, recover: Option<DateTime<Utc>>| {
            status == "passed"
                && started.is_some_and(|value| value <= now)
                && recover.is_some_and(|value| value > now)
        };
        let windows = vec![
            CodexQuotaOverdraftWindow {
                window_key: "5h".to_owned(),
                active: active(five_started, five_recover),
                started_at: five_started,
                recover_at: five_recover,
                stats: None,
            },
            CodexQuotaOverdraftWindow {
                window_key: "7d".to_owned(),
                active: active(seven_started, seven_recover),
                started_at: seven_started,
                recover_at: seven_recover,
                stats: None,
            },
        ];
        result.insert(
            account_id.clone(),
            CodexQuotaOverdraftState {
                account_id,
                status,
                quota_window: row
                    .try_get("quota_window")
                    .map_err(|_| invalid("invalid quota window"))?,
                probe_model: row
                    .try_get("probe_model")
                    .map_err(|_| invalid("invalid probe model"))?,
                attempts: u32::try_from(
                    row.try_get::<i32, _>("attempts")
                        .map_err(|_| invalid("invalid attempts"))?,
                )
                .map_err(|_| invalid("invalid attempts"))?,
                attempt_limit: u32::try_from(
                    row.try_get::<i32, _>("attempt_limit")
                        .map_err(|_| invalid("invalid attempt limit"))?,
                )
                .map_err(|_| invalid("invalid attempt limit"))?,
                reason_code: row
                    .try_get("reason_code")
                    .map_err(|_| invalid("invalid reason code"))?,
                tested_at: row
                    .try_get("tested_at")
                    .map_err(|_| invalid("invalid tested time"))?,
                retry_at: row
                    .try_get("retry_at")
                    .map_err(|_| invalid("invalid retry time"))?,
                recover_at: row
                    .try_get("recover_at")
                    .map_err(|_| invalid("invalid recovery time"))?,
                windows,
            },
        );
    }
    Ok(result)
}

pub(super) async fn claim(pool: &PgPool) -> StoreResult<Option<CodexQuotaOverdraftClaim>> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| postgres_unavailable("begin quota overdraft claim").with_source(error))?;
    sqlx::query(
        "update codex_quota_overdraft_states s
         set status='recovered', reason_code=null, retry_at=null, owner_token=null, lease_until=null, updated_at=now()
         from provider_accounts a
         where a.id=s.account_id and a.quota_access_state<>'exhausted' and s.status<>'recovered'",
    )
    .execute(&mut *tx)
    .await
    .map_err(|error| postgres_unavailable("recover quota overdraft states").with_source(error))?;

    let settings = sqlx::query(
        "select config_revision,codex_quota_overdraft_enabled,codex_quota_overdraft_probe_model
         from runtime_settings where id=1 for share",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| postgres_unavailable("load quota overdraft settings").with_source(error))?;
    if !settings
        .try_get::<bool, _>("codex_quota_overdraft_enabled")
        .map_err(|_| invalid("invalid quota overdraft enabled setting"))?
    {
        tx.commit().await.map_err(|error| {
            postgres_unavailable("commit disabled quota overdraft claim").with_source(error)
        })?;
        return Ok(None);
    }
    let config_revision = settings
        .try_get::<i64, _>("config_revision")
        .map_err(|_| invalid("invalid quota overdraft config revision"))?;
    let probe_model = settings
        .try_get::<String, _>("codex_quota_overdraft_probe_model")
        .map_err(|_| invalid("invalid quota overdraft probe model"))?;

    let row = sqlx::query(
        "select a.id,a.credential_revision,a.upstream_account_id,a.quota_reset_at,a.quota_access_observed_at,
                s.status,s.cycle_key,s.generation,s.attempts,s.attempt_limit,s.lease_until,
                s.config_revision as state_config_revision,s.credential_revision as state_credential_revision
         from provider_accounts a
         left join codex_quota_overdraft_states s on s.account_id=a.id
         where a.provider_kind='openai' and a.authentication_kind='oauth' and a.enabled
           and a.credential_state='ready' and a.responses_upstream='codex'
           and a.upstream_account_id is not null and a.quota_access_state='exhausted'
           and (s.account_id is null
             or s.credential_revision<>a.credential_revision
             or s.config_revision<>$1
             or s.cycle_key<>coalesce(extract(epoch from a.quota_reset_at)::bigint::text,'unknown')||':'||coalesce(extract(epoch from a.quota_access_observed_at)::bigint::text,'unknown')
             or (s.status='inconclusive' and s.attempts<s.attempt_limit and s.retry_at<=now())
             or (s.status='pending' and s.attempts<s.attempt_limit and s.lease_until<=now()))
           and (s.lease_until is null or s.lease_until<=now())
         order by a.quota_reset_at nulls last,a.id
         for update of a skip locked limit 1",
    )
    .bind(config_revision)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| postgres_unavailable("claim quota overdraft candidate").with_source(error))?;
    let Some(row) = row else {
        tx.commit().await.map_err(|error| {
            postgres_unavailable("commit empty quota overdraft claim").with_source(error)
        })?;
        return Ok(None);
    };

    let account_id = row
        .try_get::<String, _>("id")
        .map_err(|_| invalid("invalid account ID"))?;
    let credential_revision = row
        .try_get::<i64, _>("credential_revision")
        .map_err(|_| invalid("invalid credential revision"))?;
    let subject = row
        .try_get::<Option<String>, _>("upstream_account_id")
        .map_err(|_| invalid("invalid upstream account ID"))?
        .ok_or_else(|| invalid("missing upstream account ID"))?;
    let reset_at = row
        .try_get::<Option<DateTime<Utc>>, _>("quota_reset_at")
        .map_err(|_| invalid("invalid quota reset time"))?;
    let observed_at = row
        .try_get::<Option<DateTime<Utc>>, _>("quota_access_observed_at")
        .map_err(|_| invalid("invalid quota observation time"))?;
    let cycle_key = format!(
        "{}:{}",
        reset_at
            .map(|value| value.timestamp().to_string())
            .unwrap_or_else(|| "unknown".to_owned()),
        observed_at
            .map(|value| value.timestamp().to_string())
            .unwrap_or_else(|| "unknown".to_owned()),
    );
    let previous_cycle = row
        .try_get::<Option<String>, _>("cycle_key")
        .unwrap_or(None);
    let previous_generation = row
        .try_get::<Option<i64>, _>("generation")
        .unwrap_or(None)
        .unwrap_or(0);
    let previous_attempts = row
        .try_get::<Option<i32>, _>("attempts")
        .unwrap_or(None)
        .unwrap_or(0);
    let state_config_revision = row
        .try_get::<Option<i64>, _>("state_config_revision")
        .unwrap_or(None);
    let state_credential_revision = row
        .try_get::<Option<i64>, _>("state_credential_revision")
        .unwrap_or(None);
    let same_cycle = previous_cycle.as_deref() == Some(cycle_key.as_str())
        && state_config_revision == Some(config_revision)
        && state_credential_revision == Some(credential_revision);
    let generation = if same_cycle {
        previous_generation.max(1)
    } else {
        previous_generation.saturating_add(1).max(1)
    };
    let attempts = if same_cycle {
        previous_attempts.saturating_add(1)
    } else {
        1
    };
    let owner_token = uuid::Uuid::now_v7().to_string();
    let started_at = Utc::now();
    let lease_until = started_at + TimeDelta::seconds(30);
    sqlx::query(
        "insert into codex_quota_overdraft_states(
             account_id,credential_revision,account_subject,config_revision,status,quota_window,cycle_key,generation,probe_model,attempts,attempt_limit,started_at,recover_at,five_hour_started_at,five_hour_recover_at,seven_day_started_at,seven_day_recover_at,owner_token,lease_until,updated_at)
         values($1,$2,$3,$4,'pending','multiple',$5,$6,$7,$8,5,$9,$10,$9,$10,$9,$10,$11,$12,now())
         on conflict(account_id) do update set
             credential_revision=excluded.credential_revision,account_subject=excluded.account_subject,config_revision=excluded.config_revision,
             status='pending',quota_window='multiple',cycle_key=excluded.cycle_key,generation=excluded.generation,probe_model=excluded.probe_model,
             attempts=excluded.attempts,attempt_limit=5,reason_code=null,started_at=case when codex_quota_overdraft_states.cycle_key=excluded.cycle_key then codex_quota_overdraft_states.started_at else excluded.started_at end,
             tested_at=null,retry_at=null,recover_at=excluded.recover_at,
             five_hour_started_at=case when codex_quota_overdraft_states.cycle_key=excluded.cycle_key then codex_quota_overdraft_states.five_hour_started_at else excluded.five_hour_started_at end,
             five_hour_recover_at=excluded.five_hour_recover_at,seven_day_started_at=case when codex_quota_overdraft_states.cycle_key=excluded.cycle_key then codex_quota_overdraft_states.seven_day_started_at else excluded.seven_day_started_at end,
             seven_day_recover_at=excluded.seven_day_recover_at,owner_token=excluded.owner_token,lease_until=excluded.lease_until,updated_at=now()",
    )
    .bind(&account_id)
    .bind(credential_revision)
    .bind(&subject)
    .bind(config_revision)
    .bind(&cycle_key)
    .bind(generation)
    .bind(&probe_model)
    .bind(attempts)
    .bind(started_at)
    .bind(reset_at)
    .bind(&owner_token)
    .bind(lease_until)
    .execute(&mut *tx)
    .await
    .map_err(|error| postgres_unavailable("persist quota overdraft claim").with_source(error))?;
    tx.commit()
        .await
        .map_err(|error| postgres_unavailable("commit quota overdraft claim").with_source(error))?;

    Ok(Some(CodexQuotaOverdraftClaim {
        account_id: ProviderAccountId::new(account_id)
            .map_err(|_| invalid("invalid account ID"))?,
        provider_kind: ProviderKind::new("openai").map_err(|_| invalid("invalid provider"))?,
        credential_revision: CredentialRevision::new(
            u64::try_from(credential_revision)
                .map_err(|_| invalid("invalid credential revision"))?,
        )
        .map_err(|_| invalid("invalid credential revision"))?,
        config_revision: ConfigRevision::new(
            u64::try_from(config_revision).map_err(|_| invalid("invalid config revision"))?,
        )
        .map_err(|_| invalid("invalid config revision"))?,
        account_subject: subject,
        cycle_key,
        generation: u64::try_from(generation).map_err(|_| invalid("invalid generation"))?,
        owner_token,
        probe_model: UpstreamModelId::new(probe_model)
            .map_err(|_| invalid("invalid probe model"))?,
        attempts: u32::try_from(attempts).map_err(|_| invalid("invalid attempt count"))?,
    }))
}

pub(super) async fn finish(
    pool: &PgPool,
    claim: &CodexQuotaOverdraftClaim,
    outcome: CodexQuotaOverdraftOutcome,
) -> StoreResult<bool> {
    let (status, reason, tested_at, retry_at, recover_at) = match outcome {
        CodexQuotaOverdraftOutcome::Passed { tested_at } => ("passed", None, tested_at, None, None),
        CodexQuotaOverdraftOutcome::Inconclusive {
            reason_code,
            tested_at,
            retry_at,
        } => {
            if claim.attempts >= 5 {
                ("failed", Some(reason_code), tested_at, None, None)
            } else {
                (
                    "inconclusive",
                    Some(reason_code),
                    tested_at,
                    Some(retry_at),
                    None,
                )
            }
        }
        CodexQuotaOverdraftOutcome::Failed {
            reason_code,
            tested_at,
            recover_at,
        } => ("failed", Some(reason_code), tested_at, None, recover_at),
    };
    let updated = sqlx::query(
        "update codex_quota_overdraft_states s set
             status=$7,reason_code=$8,tested_at=$9,retry_at=$10,recover_at=coalesce($11,recover_at),owner_token=null,lease_until=null,updated_at=now()
         where account_id=$1 and credential_revision=$2 and config_revision=$3 and cycle_key=$4 and generation=$5 and owner_token=$6
           and account_subject=$12
           and exists(select 1 from runtime_settings where id=1 and config_revision=$3 and codex_quota_overdraft_enabled)
           and exists(select 1 from provider_accounts where id=$1 and credential_revision=$2 and upstream_account_id=$12)",
    )
    .bind(claim.account_id.as_str())
    .bind(i64::try_from(claim.credential_revision.get()).map_err(|_| invalid("invalid credential revision"))?)
    .bind(i64::try_from(claim.config_revision.get()).map_err(|_| invalid("invalid config revision"))?)
    .bind(&claim.cycle_key)
    .bind(i64::try_from(claim.generation).map_err(|_| invalid("invalid generation"))?)
    .bind(&claim.owner_token)
    .bind(status)
    .bind(reason.as_deref())
    .bind(tested_at)
    .bind(retry_at)
    .bind(recover_at)
    .bind(&claim.account_subject)
    .execute(pool)
    .await
    .map_err(|error| postgres_unavailable("finish quota overdraft claim").with_source(error))?;
    Ok(updated.rows_affected() == 1)
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidData {
        entity: "codex quota overdraft",
        message: message.to_owned(),
    }
}
