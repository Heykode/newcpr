//! Full-pool monitor facts, read in one bounded, read-only snapshot.

use chrono::{DateTime, Duration, Utc};
use gateway_admin::model::{
    account_groups::AccountGroupRef,
    group_monitor::{
        GroupMonitorFacts, MonitorLifespan, MonitorMember, MonitorQuotaPeer, MonitorUsage,
    },
};

use super::*;

impl PgAccountGroupRepository {
    pub(super) async fn monitor_facts(
        &self,
        now: DateTime<Utc>,
    ) -> AdminStoreResult<GroupMonitorFacts> {
        self.read_monitor_facts(now)
            .await
            .map_err(|error| admin_store_error(ENTITY, error))
    }

    async fn read_monitor_facts(&self, now: DateTime<Utc>) -> StoreResult<GroupMonitorFacts> {
        self.sync_monitor_lifecycles(now).await?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| unavailable("monitor snapshot"))?;
        sqlx::query("set transaction isolation level repeatable read, read only")
            .execute(&mut *tx)
            .await
            .map_err(|_| unavailable("monitor read-only snapshot"))?;
        sqlx::query("set local statement_timeout = '2s'")
            .execute(&mut *tx)
            .await
            .map_err(|_| unavailable("monitor query budget"))?;
        let revision: i64 =
            sqlx::query_scalar("select config_revision from runtime_settings where id = 1")
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| unavailable("monitor config revision"))?;
        let rows = sqlx::query(
            "select id, name, color, enabled from account_groups order by created_at, id",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| unavailable("monitor groups"))?;
        let mut facts = GroupMonitorFacts {
            config_revision: u64::try_from(revision).map_err(|_| invalid("monitor revision"))?,
            ..Default::default()
        };
        for row in rows {
            facts.groups.push(AccountGroupRef {
                id: AccountGroupId::new(
                    row.try_get::<String, _>("id")
                        .map_err(|_| invalid("group ID"))?,
                )
                .map_err(|_| invalid("group ID"))?,
                name: row.try_get("name").map_err(|_| invalid("group name"))?,
                color: AccountGroupColor::parse(
                    row.try_get::<String, _>("color")
                        .map_err(|_| invalid("group color"))?
                        .as_str(),
                )
                .ok_or_else(|| invalid("group color"))?,
                enabled: row
                    .try_get("enabled")
                    .map_err(|_| invalid("group enabled"))?,
            });
        }
        let group_ids = facts
            .groups
            .iter()
            .map(|group| group.id.clone())
            .collect::<Vec<_>>();
        let members = load_member_facts(&mut *tx, &group_ids).await?;
        let ids = group_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let account_ids = members
            .iter()
            .map(|member| member.account_id.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let query = format!(
            "select a.id, a.provider_kind, a.plan_type, a.created_at, coalesce(l.account_created_at, a.created_at) as first_seen_at
             from provider_accounts a left join monitor_account_lifecycles l
               on l.identity_key = {}",
            monitor_lifecycles::IDENTITY_SQL,
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| unavailable("monitor account facts"))?;
        let mut metadata = BTreeMap::new();
        for row in rows {
            facts.quota_peers.push(MonitorQuotaPeer {
                id: row.try_get("id").map_err(|_| invalid("account ID"))?,
                provider: row
                    .try_get("provider_kind")
                    .map_err(|_| invalid("provider"))?,
                plan: row.try_get("plan_type").map_err(|_| invalid("plan"))?,
                created_at: row
                    .try_get("created_at")
                    .map_err(|_| invalid("created at"))?,
            });
            metadata.insert(
                row.try_get::<String, _>("id")
                    .map_err(|_| invalid("account ID"))?,
                (
                    row.try_get::<String, _>("provider_kind")
                        .map_err(|_| invalid("provider"))?,
                    row.try_get::<Option<String>, _>("plan_type")
                        .map_err(|_| invalid("plan"))?,
                    row.try_get::<DateTime<Utc>, _>("first_seen_at")
                        .map_err(|_| invalid("first seen"))?,
                ),
            );
        }
        let rows = sqlx::query(
            "select account_group_id, client_api_key_id from client_api_key_groups
             where account_group_id = any($1::text[]) order by account_group_id, client_api_key_id",
        )
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| unavailable("monitor group client keys"))?;
        for row in rows {
            facts
                .group_client_keys
                .entry(
                    row.try_get("account_group_id")
                        .map_err(|_| invalid("group ID"))?,
                )
                .or_default()
                .push(
                    row.try_get("client_api_key_id")
                        .map_err(|_| invalid("client key ID"))?,
                );
        }
        for member in members {
            let Some((provider, plan, first_seen_at)) = metadata.get(&member.account_id) else {
                // A concurrent deletion must not publish a seemingly complete estimate.
                return Err(unavailable("monitor membership changed"));
            };
            facts.members.push(MonitorMember {
                member,
                provider: provider.clone(),
                plan: plan.clone(),
                first_seen_at: *first_seen_at,
            });
        }
        // Bound the scan by completion time; a long request contributes when its cost is known.
        let predicate = completed_usage_fact_predicate("mr");
        let query = format!(
            "with recent as (
               select mr.provider_account_ref, mr.cost_amount, mr.cost_currency
                 from model_requests mr
                where mr.completed_at > $2 and mr.completed_at <= $3 and {predicate}
                  and mr.provider_account_ref = any($4::text[])
             ), attributed as (
               select 'group' as kind, g.id, r.cost_amount, r.cost_currency
                 from unnest($1::text[]) g(id) join recent r on exists (
                   select 1 from account_group_accounts membership
                    where membership.account_group_id = g.id
                      and membership.provider_account_id = r.provider_account_ref
                 )
               union all
               select 'account', provider_account_ref, cost_amount, cost_currency from recent
                 where provider_account_ref = any($4::text[])
             )
             select kind, id,
                    coalesce(sum(cost_amount) filter (where cost_currency = 'USD'), 0)::float8 as usd,
                    count(*) filter (where cost_amount is null or cost_currency is distinct from 'USD')::bigint as missing
               from attributed group by kind, id"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(&ids)
            .bind(now - Duration::seconds(60))
            .bind(now)
            .bind(&account_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| unavailable("monitor recent usage"))?;
        for row in rows {
            let usage = MonitorUsage {
                usd: row.try_get("usd").map_err(|_| invalid("monitor cost"))?,
                missing_costs: u64::try_from(
                    row.try_get::<i64, _>("missing")
                        .map_err(|_| invalid("monitor coverage"))?,
                )
                .map_err(|_| invalid("monitor coverage"))?,
            };
            let id = row
                .try_get::<String, _>("id")
                .map_err(|_| invalid("monitor usage ID"))?;
            if row
                .try_get::<String, _>("kind")
                .map_err(|_| invalid("monitor usage kind"))?
                == "group"
            {
                facts.group_usage.insert(id, usage);
            } else {
                facts.account_usage.insert(id, usage);
            }
        }
        // Recovered accounts leave the sample set; earlier valid samples fill the latest five.
        let rows = sqlx::query(
            "with lifetimes as (
               select provider_kind, lower(plan_type) as plan,
                      extract(epoch from (dead_at - account_created_at)) / 60 as minutes,
                      row_number() over (partition by provider_kind, lower(plan_type) order by dead_at desc, identity_key) as rank
                 from monitor_account_lifecycles
                where dead_at is not null and dead_at <= $1
                  and plan_type is not null and lower(plan_type) <> 'unknown'
                  and dead_at > account_created_at
             ) select provider_kind, plan, avg(minutes)::float8 as minutes, count(*)::bigint as samples
                 from lifetimes where rank <= 5 group by provider_kind, plan",
        ).bind(now).fetch_all(&mut *tx).await.map_err(|_| unavailable("monitor lifespan samples"))?;
        for row in rows {
            facts.lifespans.push(MonitorLifespan {
                provider: row
                    .try_get("provider_kind")
                    .map_err(|_| invalid("lifespan provider"))?,
                plan: row.try_get("plan").map_err(|_| invalid("lifespan plan"))?,
                minutes: row
                    .try_get("minutes")
                    .map_err(|_| invalid("lifespan minutes"))?,
                samples: u64::try_from(
                    row.try_get::<i64, _>("samples")
                        .map_err(|_| invalid("lifespan samples"))?,
                )
                .map_err(|_| invalid("lifespan samples"))?,
            });
        }
        tx.commit()
            .await
            .map_err(|_| unavailable("finish monitor snapshot"))?;
        Ok(facts)
    }
}
