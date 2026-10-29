//! 按 Key 串行检查限额，并幂等累计已取得的 USD 费用。

use std::{collections::BTreeMap, time::Duration};

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use gateway_admin::model::{
    MutationContext,
    client_keys::{ClientKeyBudgetPeriod, ResetClientKeyBudget},
};
use gateway_core::{
    engine::budget::{
        ClientBudgetCharge, ClientBudgetError, ClientBudgetLimits, ClientBudgetPort,
        ClientBudgetStatus,
    },
    error::{GatewayError, GatewayErrorKind},
    metering::Decimal,
    policy::ClientApiKeyId,
};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::{StoreError, StoreResult, mutation_audit, postgres_unavailable};

pub(super) async fn reset_client_key_budget(
    pool: &PgPool,
    command: ResetClientKeyBudget,
    context: &MutationContext,
) -> StoreResult<()> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| postgres_unavailable("begin budget reset").with_source(error))?;
    // 与准入和结算使用相同的 Key 行锁，锁定后再确定重置时间。
    let exists =
        sqlx::query_scalar::<_, String>("select id from client_api_keys where id = $1 for update")
            .bind(command.id.as_str())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| postgres_unavailable("lock budget reset key").with_source(error))?;
    if exists.is_none() {
        return Err(StoreError::NotFound {
            entity: "client API key",
            id: command.id.as_str().to_owned(),
        });
    }
    let daily = matches!(
        command.period,
        ClientKeyBudgetPeriod::Daily | ClientKeyBudgetPeriod::All
    );
    let weekly = matches!(
        command.period,
        ClientKeyBudgetPeriod::Weekly | ClientKeyBudgetPeriod::All
    );
    let reset_at = Utc::now();
    // 已完成但延迟落盘的旧费用不回扣；不为从未使用的 Key 开启窗口。
    sqlx::query(
        "update client_key_budget_windows set
         daily_used_usd = case when $2 then 0 else daily_used_usd end,
         daily_start = case when $2 and daily_end > $4 then $4 else daily_start end,
         weekly_used_usd = case when $3 then 0 else weekly_used_usd end,
         weekly_start = case when $3 and weekly_end > $4 then $4 else weekly_start end
         where client_api_key_id = $1",
    )
    .bind(command.id.as_str())
    .bind(daily)
    .bind(weekly)
    .bind(reset_at)
    .execute(&mut *transaction)
    .await
    .map_err(|error| postgres_unavailable("reset client budget").with_source(error))?;
    let mut fields = Vec::new();
    if daily {
        fields.extend(["daily_used_usd".to_owned(), "daily_start".to_owned()]);
    }
    if weekly {
        fields.extend(["weekly_used_usd".to_owned(), "weekly_start".to_owned()]);
    }
    super::admin_security_audit::insert_admin_audit_event(
        &mut transaction,
        mutation_audit(
            context,
            "reset_budget",
            "client_api_key",
            command.id.as_str(),
            fields,
        ),
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|error| postgres_unavailable("commit budget reset").with_source(error))
}

pub struct PgClientBudgetStore {
    pool: PgPool,
}

impl PgClientBudgetStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub(super) async fn admit_with_pending(
        &self,
        key_id: ClientApiKeyId,
        pending: &[ClientBudgetCharge],
    ) -> Result<(), GatewayError> {
        // 同一语句快照同时读取窗口和已落库请求，避免提交与队列移除之间重复计算费用
        // 普通准入只读，不等待后台结算持有的 Key 行锁
        let now = Utc::now();
        let ids = pending
            .iter()
            .map(|charge| charge.request_id.as_str())
            .collect::<Vec<_>>();
        let row = sqlx::query(
            "select date_trunc('day', $3::timestamptz at time zone 'Asia/Shanghai') at time zone 'Asia/Shanghai' as budget_day, k.enabled, k.daily_limit_usd::text, k.weekly_limit_usd::text,
                w.daily_start, w.daily_end, w.weekly_start, w.weekly_end,
                coalesce(w.daily_used_usd, 0)::text as daily_used,
                coalesce(w.weekly_used_usd, 0)::text as weekly_used,
                case when k.daily_limit_usd = 0 and k.weekly_limit_usd = 0 then array[]::text[]
                    else array(select request_id from client_key_charge_events where request_id = any($2))
                end as persisted
            from client_api_keys k left join client_key_budget_windows w on w.client_api_key_id = k.id
            where k.id = $1",
        )
        .bind(key_id.as_str()).bind(ids).bind(now).fetch_optional(&self.pool).await
        .map_err(|source| unavailable().with_source(source))?
        .ok_or_else(|| GatewayError::new(GatewayErrorKind::Unauthorized, "Client API key no longer exists."))?;
        if !row.get::<bool, _>("enabled") {
            return Err(GatewayError::new(
                GatewayErrorKind::PolicyDenied,
                "Client API key is disabled.",
            ));
        }
        let parse = |field: &str| -> Result<Decimal, GatewayError> {
            row.get::<String, _>(field)
                .parse()
                .map_err(|source| unavailable().with_source(source))
        };
        let limits = ClientBudgetLimits {
            daily_usd: parse("daily_limit_usd")?,
            weekly_usd: parse("weekly_limit_usd")?,
        };
        let day = row.get::<DateTime<Utc>, _>("budget_day");
        let window = |start: &str, end: &str, used: &str, days| -> Result<_, GatewayError> {
            let start = row.get::<Option<DateTime<Utc>>, _>(start);
            let end = row.get::<Option<DateTime<Utc>>, _>(end);
            let reopen = end.is_none_or(|end| end <= now);
            if reopen {
                Ok((
                    day.max(start.unwrap_or(day)).max(end.unwrap_or(day)),
                    day + chrono::TimeDelta::days(days),
                    Decimal::ZERO,
                    true,
                ))
            } else {
                Ok((
                    start.ok_or_else(unavailable)?,
                    end.ok_or_else(unavailable)?,
                    parse(used)?,
                    false,
                ))
            }
        };
        let (daily_start, daily_end, mut daily, open_daily) =
            window("daily_start", "daily_end", "daily_used", 1)?;
        let (weekly_start, weekly_end, mut weekly, open_weekly) =
            window("weekly_start", "weekly_end", "weekly_used", 7)?;
        if limits.is_limited() {
            let persisted = row
                .get::<Vec<String>, _>("persisted")
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>();
            for charge in pending {
                if persisted.contains(charge.request_id.as_str()) {
                    continue;
                }
                let completed_at = DateTime::<Utc>::from(charge.completed_at);
                if completed_at >= daily_start && completed_at < daily_end {
                    daily = daily
                        .checked_add(charge.amount_usd)
                        .ok_or_else(unavailable)?;
                }
                if completed_at >= weekly_start && completed_at < weekly_end {
                    weekly = weekly
                        .checked_add(charge.amount_usd)
                        .ok_or_else(unavailable)?;
                }
            }
            let daily_exceeded = limits.daily_usd != Decimal::ZERO && daily >= limits.daily_usd;
            let weekly_exceeded = limits.weekly_usd != Decimal::ZERO && weekly >= limits.weekly_usd;
            if daily_exceeded || weekly_exceeded {
                let (reset, code, message) = if weekly_exceeded {
                    (
                        weekly_end,
                        "key_weekly_budget_exceeded",
                        "Weekly spending limit reached.",
                    )
                } else {
                    (
                        daily_end,
                        "key_daily_budget_exceeded",
                        "Daily spending limit reached.",
                    )
                };
                return Err(GatewayError::new(GatewayErrorKind::RateLimited, message)
                    .with_client_code(code)
                    .with_retry_after((reset - now).to_std().unwrap_or(Duration::from_secs(1))));
            }
        }
        // 首次使用或窗口到期才写入；与重置、结算保持相同锁序
        if open_daily || open_weekly {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|source| unavailable().with_source(source))?;
            sqlx::query("select id from client_api_keys where id = $1 for update")
                .bind(key_id.as_str())
                .fetch_optional(&mut *tx)
                .await
                .map_err(|source| unavailable().with_source(source))?
                .ok_or_else(|| {
                    GatewayError::new(
                        GatewayErrorKind::Unauthorized,
                        "Client API key no longer exists.",
                    )
                })?;
            advance_windows(&mut tx, key_id.as_str(), now, now)
                .await
                .map_err(|source| unavailable().with_source(source))?;
            tx.commit()
                .await
                .map_err(|source| unavailable().with_source(source))?;
        }
        Ok(())
    }

    pub(super) async fn settle_charges(
        &self,
        charges: &[ClientBudgetCharge],
    ) -> Result<(), ClientBudgetError> {
        let Some(first) = charges.first() else {
            return Ok(());
        };
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|source| ClientBudgetError(Some(source.into())))?;
        // 与窗口开启和重置统一先锁 Key，再写窗口和费用
        let key = sqlx::query_scalar::<_, String>(
            "select id from client_api_keys where id = $1 for update",
        )
        .bind(first.key_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|source| ClientBudgetError(Some(source.into())))?;
        let Some(key) = key else { return Ok(()) }; // 删除 Key 时也会删除其费用记录
        settle_in_transaction(&mut tx, &key, charges)
            .await
            .map_err(|source| ClientBudgetError(Some(source.into())))?;
        tx.commit()
            .await
            .map_err(|source| ClientBudgetError(Some(source.into())))
    }
}

async fn settle_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    charges: &[ClientBudgetCharge],
) -> Result<(), sqlx::Error> {
    let ids = charges
        .iter()
        .map(|charge| charge.request_id.as_str())
        .collect::<Vec<_>>();
    let amounts = charges
        .iter()
        .map(|charge| charge.amount_usd.canonical())
        .collect::<Vec<_>>();
    let times = charges
        .iter()
        .map(|charge| DateTime::<Utc>::from(charge.completed_at))
        .collect::<Vec<_>>();
    // 同一 Key 的小批次共用事务，只累计本次真正插入的事件
    let inserted = sqlx::query(
        "insert into client_key_charge_events (request_id, client_api_key_id, amount_usd, completed_at)
            select request_id, $1, amount::numeric, completed_at
            from unnest($2::text[], $3::text[], $4::timestamptz[]) as charges(request_id, amount, completed_at)
            on conflict (request_id) do nothing returning amount_usd::text as amount, completed_at",
    ).bind(key).bind(ids).bind(amounts).bind(times).fetch_all(&mut **tx).await?;
    let amounts = inserted
        .iter()
        .map(|row| row.get::<String, _>("amount"))
        .collect::<Vec<_>>();
    let times = inserted
        .iter()
        .map(|row| row.get::<DateTime<Utc>, _>("completed_at"))
        .collect::<Vec<_>>();
    let Some(latest) = times.iter().max().copied() else {
        return Ok(());
    };
    advance_windows(tx, key, Utc::now(), latest).await?;
    sqlx::query(
        "update client_key_budget_windows w set
        daily_used_usd = daily_used_usd + coalesce((select sum(amount::numeric)
            from unnest($2::text[], $3::timestamptz[]) as charges(amount, completed_at)
            where completed_at >= w.daily_start and completed_at < w.daily_end), 0),
        weekly_used_usd = weekly_used_usd + coalesce((select sum(amount::numeric)
            from unnest($2::text[], $3::timestamptz[]) as charges(amount, completed_at)
            where completed_at >= w.weekly_start and completed_at < w.weekly_end), 0)
        where client_api_key_id = $1",
    )
    .bind(key)
    .bind(amounts)
    .bind(times)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl ClientBudgetPort for PgClientBudgetStore {
    fn admit(&self, key_id: ClientApiKeyId) -> BoxFuture<'_, Result<(), GatewayError>> {
        Box::pin(async move { self.admit_with_pending(key_id, &[]).await })
    }

    fn settle(&self, charge: ClientBudgetCharge) -> BoxFuture<'_, Result<(), ClientBudgetError>> {
        Box::pin(async move { self.settle_charges(std::slice::from_ref(&charge)).await })
    }
}

async fn advance_windows(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    now: DateTime<Utc>,
    used_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query("insert into client_key_budget_windows
        (client_api_key_id, daily_start, daily_end, weekly_start, weekly_end)
        select $1, day, day + interval '24 hours', day, day + interval '168 hours'
        from (select date_trunc('day', $2::timestamptz at time zone 'Asia/Shanghai') at time zone 'Asia/Shanghai' as day) d
        on conflict (client_api_key_id) do update set
            daily_start = case when client_key_budget_windows.daily_end <= $2 and client_key_budget_windows.daily_start <= $3 then greatest(excluded.daily_start, client_key_budget_windows.daily_start) else client_key_budget_windows.daily_start end,
            daily_end = case when client_key_budget_windows.daily_end <= $2 and client_key_budget_windows.daily_start <= $3 then excluded.daily_end else client_key_budget_windows.daily_end end,
            daily_used_usd = case when client_key_budget_windows.daily_end <= $2 and client_key_budget_windows.daily_start <= $3 then 0 else client_key_budget_windows.daily_used_usd end,
            weekly_start = case when client_key_budget_windows.weekly_end <= $2 and client_key_budget_windows.weekly_start <= $3 then greatest(excluded.weekly_start, client_key_budget_windows.weekly_start) else client_key_budget_windows.weekly_start end,
            weekly_end = case when client_key_budget_windows.weekly_end <= $2 and client_key_budget_windows.weekly_start <= $3 then excluded.weekly_end else client_key_budget_windows.weekly_end end,
            weekly_used_usd = case when client_key_budget_windows.weekly_end <= $2 and client_key_budget_windows.weekly_start <= $3 then 0 else client_key_budget_windows.weekly_used_usd end")
        .bind(key).bind(now).bind(used_at).execute(&mut **tx).await?;
    Ok(())
}

pub(super) async fn load_client_key_budgets(
    pool: &PgPool,
    records: &mut [super::ClientApiKeyRecord],
) -> StoreResult<()> {
    if records.is_empty() {
        return Ok(());
    }
    let ids = records
        .iter()
        .map(|record| record.id.as_str())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        "select k.id, k.daily_limit_usd::text, k.weekly_limit_usd::text,
        (case when w.daily_end > w.daily_start and w.daily_end > now() then w.daily_used_usd else 0 end)::text as daily_used,
        (case when w.weekly_end > w.weekly_start and w.weekly_end > now() then w.weekly_used_usd else 0 end)::text as weekly_used,
        case when w.daily_end > w.daily_start and w.daily_end > now() then w.daily_end end as daily_end,
        case when w.weekly_end > w.weekly_start and w.weekly_end > now() then w.weekly_end end as weekly_end
        from client_api_keys k left join client_key_budget_windows w on w.client_api_key_id = k.id
        where k.id = any($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
    .map_err(|error| postgres_unavailable("load client budgets").with_source(error))?;
    let mut budgets = BTreeMap::new();
    for row in rows {
        let parse = |field| -> StoreResult<Decimal> {
            row.get::<String, _>(field)
                .parse()
                .map_err(|error| postgres_unavailable("decode client budget").with_source(error))
        };
        budgets.insert(
            row.get::<String, _>("id"),
            ClientBudgetStatus {
                limits: ClientBudgetLimits {
                    daily_usd: parse("daily_limit_usd")?,
                    weekly_usd: parse("weekly_limit_usd")?,
                },
                daily_used_usd: parse("daily_used")?,
                weekly_used_usd: parse("weekly_used")?,
                daily_resets_at: row
                    .get::<Option<DateTime<Utc>>, _>("daily_end")
                    .map(Into::into),
                weekly_resets_at: row
                    .get::<Option<DateTime<Utc>>, _>("weekly_end")
                    .map(Into::into),
            },
        );
    }
    for record in records {
        record.budget = budgets
            .remove(&record.id)
            .ok_or_else(|| postgres_unavailable("load client budget policy"))?;
    }
    Ok(())
}

pub(super) fn unavailable() -> GatewayError {
    GatewayError::new(
        GatewayErrorKind::ProviderInfrastructureUnavailable,
        "Client budget service is temporarily unavailable.",
    )
    .with_client_code("key_budget_unavailable")
}
