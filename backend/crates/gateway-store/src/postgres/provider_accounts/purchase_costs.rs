//! Durable purchase metadata; never mutates scheduling, credentials, or billing prices.

use super::*;
use gateway_admin::model::account_purchase::{AccountPurchaseUpdate, AccountPurchaseView};
use std::collections::BTreeMap;

pub(super) async fn update(
    transaction: &mut Transaction<'_, Postgres>,
    account_ids: &[String],
    patch: &AccountPurchaseUpdate,
) -> StoreResult<()> {
    patch
        .validate()
        .map_err(|_| invalid("invalid account purchase cost"))?;
    let bound: i64 = sqlx::query_scalar(
        "select count(*) from provider_accounts a join account_purchase_bindings b
         on b.provider_account_ref=a.id where a.id=any($1::text[])",
    )
    .bind(account_ids)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("validate purchase identity"))?;
    if usize::try_from(bound).ok() != Some(account_ids.len()) {
        return Err(invalid(
            "purchase cost requires a confirmed account identity",
        ));
    }
    sqlx::query(
        "update account_purchase_identities p set monthly_cost_cny=$2::text::numeric,
           cycle_anchor=coalesce($3, p.cycle_anchor, (now() at time zone 'Asia/Shanghai')::date)
         where p.id in (select identity_id from account_purchase_bindings
                        where provider_account_ref=any($1::text[]))",
    )
    .bind(account_ids)
    .bind(patch.amount_cny.as_deref())
    .bind(patch.cycle_start)
    .execute(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("update account purchase cost"))?;
    Ok(())
}

pub(super) async fn load(
    pool: &PgPool,
    account_ids: &[String],
) -> StoreResult<BTreeMap<String, AccountPurchaseView>> {
    if account_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let rows = sqlx::query(
        "select b.provider_account_ref, p.monthly_cost_cny::text as amount_cny,
           p.cycle_anchor, period.period_start, period.period_end,
           coalesce(usage.amount,0)::text as usage_usd,
           case when usage.amount>0 and p.monthly_cost_cny is not null
             then (p.monthly_cost_cny / usage.amount)::text end as breakeven,
           (p.history_complete_from is null or
             period.period_start >= (p.history_complete_from at time zone 'Asia/Shanghai')::date + 1)
               as history_complete, p.history_complete_from
         from account_purchase_bindings b join account_purchase_identities p on p.id=b.identity_id
         left join lateral account_purchase_period(p.cycle_anchor,
           (now() at time zone 'Asia/Shanghai')::date) period on true
         left join lateral (select sum(amount_usd) as amount from account_purchase_daily_usage d
           where d.identity_id=p.id and d.usage_date>=period.period_start
             and d.usage_date<period.period_end) usage on true
         where b.provider_account_ref=any($1::text[])",
    ).bind(account_ids).fetch_all(pool).await
        .map_err(|_| postgres_unavailable("load account purchase costs"))?;
    rows.into_iter()
        .map(|row| {
            Ok((
                get(&row, "provider_account_ref")?,
                AccountPurchaseView {
                    amount_cny: get(&row, "amount_cny")?,
                    cycle_anchor: get(&row, "cycle_anchor")?,
                    period_start: get(&row, "period_start")?,
                    period_end: get(&row, "period_end")?,
                    usage_usd: get(&row, "usage_usd")?,
                    breakeven_cny_per_usd: get(&row, "breakeven")?,
                    history_complete: get::<Option<bool>>(&row, "history_complete")?
                        .unwrap_or(false),
                    history_complete_from: get(&row, "history_complete_from")?,
                },
            ))
        })
        .collect()
}
