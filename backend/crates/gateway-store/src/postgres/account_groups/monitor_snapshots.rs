//! Only the latest complete sample is published; business rows remain untouched.

use chrono::{DateTime, Utc};
use gateway_admin::model::{
    account_groups::AccountGroupRef,
    group_monitor::{GroupMonitorItem, GroupMonitorReport},
};
use sqlx::postgres::PgRow;

use super::*;

impl PgAccountGroupRepository {
    pub(super) async fn save_monitor_snapshot(
        &self,
        report: &GroupMonitorReport,
        config_revision: u64,
    ) -> StoreResult<()> {
        let revision = signed(config_revision)?;
        if revision == 0 {
            return Err(invalid("monitor revision"));
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| unavailable("monitor publication"))?;
        sqlx::query("set local statement_timeout = '2s'")
            .execute(&mut *tx)
            .await
            .map_err(|_| unavailable("monitor publication budget"))?;
        for item in &report.items {
            // Deleted groups are skipped. A changed config revision makes this
            // sample unreadable until the next cycle, without locking business rows.
            sqlx::query(
                "insert into account_group_monitor_snapshots (
                   group_id, config_revision, sampled_at, total_accounts, eligible_accounts,
                   estimated_accounts, used_slots, total_slots, remaining_usd, remaining_status,
                   expected_expiry_usd, expiry_status, consume_usd_per_minute,
                   quota_consume_usd_per_minute, eta_minutes, eta_status, low_sample, earliest_reset_at
                 ) select id, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18
                   from account_groups where id = $1
                 on conflict (group_id) do update set
                   config_revision = excluded.config_revision, sampled_at = excluded.sampled_at,
                   total_accounts = excluded.total_accounts, eligible_accounts = excluded.eligible_accounts,
                   estimated_accounts = excluded.estimated_accounts, used_slots = excluded.used_slots,
                   total_slots = excluded.total_slots, remaining_usd = excluded.remaining_usd,
                   remaining_status = excluded.remaining_status, expected_expiry_usd = excluded.expected_expiry_usd,
                   expiry_status = excluded.expiry_status, consume_usd_per_minute = excluded.consume_usd_per_minute,
                   quota_consume_usd_per_minute = excluded.quota_consume_usd_per_minute,
                   eta_minutes = excluded.eta_minutes, eta_status = excluded.eta_status,
                   low_sample = excluded.low_sample, earliest_reset_at = excluded.earliest_reset_at
                 where excluded.sampled_at > account_group_monitor_snapshots.sampled_at",
            )
            .bind(item.group.id.as_str()).bind(revision).bind(report.generated_at)
            .bind(signed(item.total_accounts)?).bind(signed(item.eligible_accounts)?)
            .bind(signed(item.estimated_accounts)?).bind(item.used_slots.map(signed).transpose()?)
            .bind(signed(item.total_slots)?).bind(item.remaining_usd).bind(item.remaining_status)
            .bind(item.expected_expiry_usd).bind(item.expiry_status).bind(item.consume_usd_per_minute)
            .bind(item.quota_consume_usd_per_minute).bind(item.eta_minutes).bind(item.eta_status)
            .bind(item.low_sample).bind(item.earliest_reset_at)
            .execute(&mut *tx).await.map_err(|_| unavailable("save monitor snapshot"))?;
        }
        tx.commit()
            .await
            .map_err(|_| unavailable("publish monitor snapshot"))
    }

    pub(super) async fn read_monitor_snapshot(
        &self,
        group_ids: &[AccountGroupId],
    ) -> StoreResult<Option<GroupMonitorReport>> {
        if group_ids.is_empty() || group_ids.len() > 3 {
            return Err(invalid("monitor group count"));
        }
        let ids = group_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let rows = sqlx::query(
            "select g.id, g.name, g.color, g.enabled, s.*
               from account_groups g cross join runtime_settings r
               left join account_group_monitor_snapshots s
                 on s.group_id = g.id and s.config_revision = r.config_revision
              where r.id = 1 and g.id = any($1::text[]) order by g.created_at, g.id",
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| unavailable("read monitor snapshot"))?;
        if rows.len() != ids.len() {
            return Err(invalid("monitor groups changed"));
        }
        let mut report: Option<GroupMonitorReport> = None;
        for row in &rows {
            let Some(sampled_at) = row
                .try_get::<Option<DateTime<Utc>>, _>("sampled_at")
                .map_err(|_| invalid("monitor sample time"))?
            else {
                return Ok(None);
            };
            let result = report.get_or_insert_with(|| GroupMonitorReport {
                generated_at: sampled_at,
                items: Vec::new(),
            });
            result.generated_at = result.generated_at.min(sampled_at);
            result.items.push(decode(row)?);
        }
        Ok(report)
    }
}

fn signed(value: u64) -> StoreResult<i64> {
    i64::try_from(value).map_err(|_| invalid("monitor counter"))
}

fn counter(row: &PgRow, name: &str) -> StoreResult<u64> {
    row.try_get::<i64, _>(name)
        .ok()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| invalid("monitor counter"))
}

fn status(row: &PgRow, name: &str) -> StoreResult<&'static str> {
    match row
        .try_get::<String, _>(name)
        .map_err(|_| invalid("monitor status"))?
        .as_str()
    {
        "ready" => Ok("ready"),
        "partial" => Ok("partial"),
        "learning" => Ok("learning"),
        "unknown" => Ok("unknown"),
        "disabled" => Ok("disabled"),
        "idle" => Ok("idle"),
        "empty" => Ok("empty"),
        _ => Err(invalid("monitor status")),
    }
}

fn decode(row: &PgRow) -> StoreResult<GroupMonitorItem> {
    Ok(GroupMonitorItem {
        group: AccountGroupRef {
            id: AccountGroupId::new(
                row.try_get::<String, _>("id")
                    .map_err(|_| invalid("monitor group"))?,
            )
            .map_err(|_| invalid("monitor group"))?,
            name: row
                .try_get("name")
                .map_err(|_| invalid("monitor group name"))?,
            color: AccountGroupColor::parse(
                &row.try_get::<String, _>("color")
                    .map_err(|_| invalid("monitor color"))?,
            )
            .ok_or_else(|| invalid("monitor color"))?,
            enabled: row
                .try_get("enabled")
                .map_err(|_| invalid("monitor enabled"))?,
        },
        total_accounts: counter(row, "total_accounts")?,
        eligible_accounts: counter(row, "eligible_accounts")?,
        estimated_accounts: counter(row, "estimated_accounts")?,
        used_slots: row
            .try_get::<Option<i64>, _>("used_slots")
            .map_err(|_| invalid("monitor slots"))?
            .map(|value| u64::try_from(value).map_err(|_| invalid("monitor slots")))
            .transpose()?,
        total_slots: counter(row, "total_slots")?,
        remaining_usd: row
            .try_get("remaining_usd")
            .map_err(|_| invalid("monitor remaining"))?,
        remaining_status: status(row, "remaining_status")?,
        expected_expiry_usd: row
            .try_get("expected_expiry_usd")
            .map_err(|_| invalid("monitor expiry"))?,
        expiry_status: status(row, "expiry_status")?,
        consume_usd_per_minute: row
            .try_get("consume_usd_per_minute")
            .map_err(|_| invalid("monitor rate"))?,
        quota_consume_usd_per_minute: row
            .try_get("quota_consume_usd_per_minute")
            .map_err(|_| invalid("monitor quota rate"))?,
        eta_minutes: row
            .try_get("eta_minutes")
            .map_err(|_| invalid("monitor eta"))?,
        eta_status: status(row, "eta_status")?,
        low_sample: row
            .try_get("low_sample")
            .map_err(|_| invalid("monitor samples"))?,
        earliest_reset_at: row
            .try_get("earliest_reset_at")
            .map_err(|_| invalid("monitor reset"))?,
    })
}
