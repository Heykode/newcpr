//! Reversible monitor observations of existing CPR credential facts, never account mutations.

use chrono::{DateTime, Utc};

use super::*;

pub(super) const IDENTITY_SQL: &str = "case when nullif(a.upstream_user_id, '') is not null
    then jsonb_build_array('upstream', a.provider_kind, a.upstream_user_id, coalesce(a.upstream_account_id, ''))::text
    else jsonb_build_array('local', a.provider_kind, a.id)::text end";

impl PgAccountGroupRepository {
    pub(super) async fn sync_monitor_lifecycles(&self, now: DateTime<Utc>) -> StoreResult<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| unavailable("monitor lifecycle"))?;
        sqlx::query("set local statement_timeout = '2s'")
            .execute(&mut *tx)
            .await
            .map_err(|_| unavailable("monitor lifecycle budget"))?;
        sqlx::query(
            "select pg_advisory_xact_lock(hashtextextended('monitor_account_lifecycles', 0))",
        )
        .execute(&mut *tx)
        .await
        .map_err(|_| unavailable("monitor lifecycle lock"))?;
        // Existing principal fields are reused, independently of device-row creation.
        // A normal deletion is not a failure, and
        // import/cleared error text alone cannot prove that revoked credentials work.
        let predicate = completed_usage_fact_predicate("mr");
        let query = format!(
            "with current as (
               select a.*, {IDENTITY_SQL} as identity_key from provider_accounts a
             ), signals as (
               select a.*, l.account_created_at as original_created_at, l.plan_type as original_plan,
                      l.failure_started_at as previous_failure, l.failure_reason as previous_reason,
                      l.dead_at as previous_death,
                      case when a.credential_observed_at <= $1 and a.credential_observed_at > a.created_at then
                        case
                          when a.credential_state = 'banned' and a.last_error_reason = 'account_banned' then 'account_banned'
                          when a.credential_state = 'invalid' and a.last_error_reason = 'credential_invalid' then 'credential_invalid'
                          when a.credential_state = 'expired' and a.last_error_reason = 'credential_expired' then 'credential_expired'
                        end
                      end as reason,
                      (l.failure_started_at is not null and a.credential_state = 'ready' and
                       (a.access_token_expires_at is null or a.access_token_expires_at > $1) and
                       exists (
                         select 1 from model_requests mr
                          where mr.provider_account_ref = a.id and mr.outcome = 'succeeded'
                            and mr.started_at > greatest(coalesce(l.failure_started_at, a.created_at), a.credential_observed_at)
                            and mr.completed_at <= $1 and {predicate}
                       )) as recovered
                 from current a left join monitor_account_lifecycles l using (identity_key)
                where a.created_at <= $1
             ), events as (
               select *,
                      case when reason is not null then
                        case when previous_reason = reason and previous_failure is not null
                             then previous_failure else credential_observed_at end
                        when recovered then null else previous_failure end as failure_at
                 from signals
             )
             insert into monitor_account_lifecycles
               (identity_key, provider_kind, plan_type, account_created_at,
                failure_started_at, failure_reason, dead_at, sampled_at)
             select identity_key, provider_kind,
                    case when previous_death is not null and not recovered and reason is null
                         then original_plan else coalesce(nullif(trim(plan_type), ''), original_plan) end,
                    least(created_at, coalesce(original_created_at, created_at)),
                    failure_at,
                    case when reason is not null then reason when recovered then null else previous_reason end,
                    case when reason is not null then
                           case when reason = 'account_banned' or $1 - failure_at >= interval '2 minutes'
                                then failure_at else null end
                         when recovered then null else previous_death end,
                    $1
               from events
             on conflict (identity_key) do update set
               provider_kind = excluded.provider_kind, plan_type = excluded.plan_type,
               account_created_at = excluded.account_created_at,
               failure_started_at = excluded.failure_started_at, failure_reason = excluded.failure_reason,
               dead_at = excluded.dead_at, sampled_at = excluded.sampled_at
             where excluded.sampled_at > monitor_account_lifecycles.sampled_at"
        );
        sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| unavailable("update monitor lifecycle"))?;
        tx.commit()
            .await
            .map_err(|_| unavailable("publish monitor lifecycle"))
    }
}
