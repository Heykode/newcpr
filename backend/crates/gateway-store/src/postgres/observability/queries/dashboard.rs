//! Billing detail stream and shared observation granularity. Aggregates use analytics.
use super::super::*;
use futures::TryStreamExt;

pub(crate) fn calculated_usage_billing_facts(
    mut connection: ObservabilityConnection,
    range: ObservabilityRange,
    filter: UsageRecordFilter,
) -> futures::stream::BoxStream<'static, StoreResult<CalculatedUsageBillingFact>> {
    Box::pin(async_stream::try_stream! {
        filter.validate()?;
        let granularity = granularity_for(range);
        let mut query = QueryBuilder::<Postgres>::new("select date_bin(");
        query.push_bind(granularity.sql_interval());
        query.push(
            "::interval, mr.started_at, timestamptz '1970-01-01 00:00:00+00') as bucket_start,
                    mr.provider_kind, mr.upstream_model_id, mr.service_tier,
                    mr.input_tokens, mr.output_tokens, mr.cached_tokens, mr.cache_write_tokens,
                    mr.cost_currency, mr.cost_amount::text as amount
             from model_requests mr where mr.started_at >= ",
        );
        query.push_bind(range.start);
        query.push(" and mr.started_at < ");
        query.push_bind(range.end);
        query.push(
            " and mr.cost_source = 'calculated'
               and mr.cost_amount is not null and mr.cost_currency is not null
               and nullif(mr.provider_kind, '') is not null
               and nullif(mr.upstream_model_id, '') is not null",
        );
        push_completed_usage_fact_filter(&mut query, "mr");
        push_usage_filter(&mut query, &filter, "mr");
        // 费用逐条累加与行顺序无关，避免全量物化和不必要的数据库排序。
        let mut rows = query.build().fetch(&mut *connection);
        while let Some(row) = rows.try_next().await
            .map_err(|error| postgres_unavailable("load calculated usage billing facts").with_source(error))?
        {
            yield calculated_usage_billing_fact_from_row(&row)?;
        }
    })
}

pub(crate) fn granularity_for(range: ObservabilityRange) -> ObservationGranularity {
    let seconds = range.end.signed_duration_since(range.start).num_seconds();
    if seconds <= 2 * 24 * 60 * 60 {
        ObservationGranularity::FifteenMinutes
    } else if seconds <= 31 * 24 * 60 * 60 {
        ObservationGranularity::Hour
    } else {
        ObservationGranularity::Day
    }
}

pub(crate) fn coverage_from_row(row: &sqlx::postgres::PgRow) -> StoreResult<CostCoverage> {
    Ok(CostCoverage {
        provider_reported_count: unsigned(row, "provider_reported_count")?,
        calculated_count: unsigned(row, "calculated_count")?,
        unavailable_count: unsigned(row, "unavailable_count")?,
    })
}
