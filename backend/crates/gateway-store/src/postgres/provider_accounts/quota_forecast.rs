//! 预测只读采样：先聚合数值、再读取有界的 Provider 文档，不扫描模型排行维度。

use gateway_admin::model::quota_forecast_sampling::{QuotaForecastHistory, QuotaForecastUsage};

use super::*;

pub(super) async fn load_learning_usage(
    pool: &PgPool,
    budget: &super::super::ObservabilityQueryBudget,
    window: &AccountUsageWindowQuery,
) -> AdminStoreResult<QuotaForecastUsage> {
    validate_admin_account_ids(std::slice::from_ref(&window.account_id))
        .map_err(|error| admin_store_error(ENTITY, error))?;
    let range = ObservabilityRange::new(window.range.start, window.range.end)
        .map_err(|error| admin_store_error(ENTITY, error))?;
    if range.end - range.start > TimeDelta::days(32) {
        return Err(AdminStoreError::new(
            AdminStoreErrorKind::Invalid,
            ENTITY,
            "learning range exceeds supported quota windows",
        ));
    }
    let query = format!(
        "select count(*)::bigint as request_count,
                count(*) filter (where coalesce(cost_source in ('calculated', 'provider_reported')
                  and cost_currency = 'USD' and cost_amount is not null, false))::bigint as known_cost_count,
                count(*) filter (where not coalesce(cost_source in ('calculated', 'provider_reported')
                  and cost_currency = 'USD' and cost_amount is not null, false))::bigint as unavailable_cost_count,
                coalesce(sum(cost_amount) filter (where cost_source in ('calculated', 'provider_reported')
                  and cost_currency = 'USD'), 0)::float8 as usd
           from model_requests mr
          where mr.provider_account_ref = $1
            and mr.started_at >= $2 and mr.started_at < $3
            and mr.completed_at <= $3 and {}",
        completed_usage_fact_predicate("mr"),
    );
    let row = budget
        .run(
            pool,
            "load monitor learning usage",
            |mut connection| async move {
                sqlx::query(sqlx::AssertSqlSafe(query))
                    .bind(&window.account_id)
                    .bind(range.start)
                    .bind(range.end)
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(|error| {
                        postgres_unavailable("load monitor learning usage").with_source(error)
                    })
            },
        )
        .await
        .map_err(|error| admin_store_error(ENTITY, error))?;
    Ok(QuotaForecastUsage {
        request_count: window_usage_count(&row, "request_count")?,
        known_cost_count: window_usage_count(&row, "known_cost_count")?,
        unavailable_cost_count: window_usage_count(&row, "unavailable_cost_count")?,
        usd: window_usage_value(&row, "usd")?,
        ..QuotaForecastUsage::default()
    })
}

pub(super) async fn load_history(
    pool: &PgPool,
    budget: &super::super::ObservabilityQueryBudget,
    window: &AccountUsageWindowQuery,
) -> AdminStoreResult<QuotaForecastHistory> {
    validate_admin_account_ids(std::slice::from_ref(&window.account_id))
        .map_err(|error| admin_store_error(ENTITY, error))?;
    let range = ObservabilityRange::new(window.range.start, window.range.end)
        .map_err(|error| admin_store_error(ENTITY, error))?;
    if window.models.is_some() {
        return Err(AdminStoreError::new(
            AdminStoreErrorKind::Invalid,
            ENTITY,
            "quota forecast requires an account-wide window",
        ));
    }
    if range.end - range.start > TimeDelta::days(32) {
        return Err(AdminStoreError::new(
            AdminStoreErrorKind::Invalid,
            ENTITY,
            "forecast range exceeds supported quota windows",
        ));
    }
    budget
        .run(
            pool,
            "load quota forecast history",
            |mut connection| async move {
                let mut snapshot =
                    super::super::observability::analytics::AnalyticsSnapshot::new_for_accounts(
                        &mut connection,
                    )
                    .await?;
                let history = snapshot.forecast_history(&window.account_id, range).await?;
                snapshot.finish().await?;
                Ok(history)
            },
        )
        .await
        .map_err(|error| admin_store_error(ENTITY, error))
}
