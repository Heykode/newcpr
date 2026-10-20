//! Restore-only global usage. Live and restored data never share request identities.
use super::super::*;

pub(crate) fn backup_usage_export_sql() -> String {
    let fact = completed_usage_fact_predicate("mr");
    let business = business_request_predicate("mr");
    format!(
        "COPY (
         WITH selected AS MATERIALIZED (
           SELECT mr.started_at,mr.outcome,mr.client_status_code,mr.input_tokens,
             mr.output_tokens,mr.cached_tokens,mr.cache_write_tokens,mr.reasoning_tokens,
             mr.total_tokens,mr.cost_currency,mr.cost_amount, ({fact}) IS TRUE AS fact
           FROM model_requests mr
           WHERE mr.recovered_at IS NULL AND ({business}) AND mr.outcome <> 'running'
         ), counters AS (
           SELECT date_bin(interval '1 hour', started_at, timestamptz 'epoch') AS bucket_start,
             jsonb_build_object(
               'request_count',count(*),
               'success_count',count(*) FILTER (WHERE outcome='succeeded'),
               'failure_count',count(*) FILTER (WHERE outcome='failed'),
               'cancelled_count',count(*) FILTER (WHERE outcome='cancelled'),
               'incomplete_count',count(*) FILTER (WHERE outcome='incomplete'),
               'caller_error_count',count(*) FILTER (WHERE client_status_code BETWEEN 400 AND 499),
               'input_tokens',coalesce(sum(input_tokens) FILTER (WHERE fact),0),
               'output_tokens',coalesce(sum(output_tokens) FILTER (WHERE fact),0),
               'cached_tokens',coalesce(sum(cached_tokens) FILTER (WHERE fact),0),
               'cache_write_tokens',coalesce(sum(cache_write_tokens) FILTER (WHERE fact),0),
               'reasoning_tokens',coalesce(sum(reasoning_tokens) FILTER (WHERE fact),0),
               'total_tokens',coalesce(sum(total_tokens) FILTER (WHERE fact),0)
             ) AS counts
           FROM selected GROUP BY bucket_start
         ), amounts AS (
           SELECT date_bin(interval '1 hour', started_at, timestamptz 'epoch') AS bucket_start,
             cost_currency, sum(cost_amount)::text AS amount
           FROM selected WHERE fact AND cost_currency IS NOT NULL AND cost_amount IS NOT NULL
           GROUP BY bucket_start,cost_currency
         ), live_costs AS (
           SELECT bucket_start,jsonb_object_agg(cost_currency,amount) AS costs
           FROM amounts GROUP BY bucket_start
         ), hours AS (
           SELECT bucket_start,counts,coalesce(costs,'{{}}'::jsonb) AS costs
           FROM counters LEFT JOIN live_costs USING(bucket_start)
           UNION ALL SELECT bucket_start,counts,costs FROM backup_usage_hours
         ), combined_counts AS (
           SELECT bucket_start,c.key,sum(c.value::numeric) AS value
           FROM hours CROSS JOIN LATERAL jsonb_each_text(counts) c GROUP BY bucket_start,c.key
         ), combined_costs AS (
           SELECT bucket_start,c.key,sum(c.value::numeric)::text AS value
           FROM hours CROSS JOIN LATERAL jsonb_each_text(costs) c GROUP BY bucket_start,c.key
         ), totals AS (
           SELECT bucket_start,jsonb_object_agg(key,value) AS counts
           FROM combined_counts GROUP BY bucket_start
         ), costs AS (
           SELECT bucket_start,jsonb_object_agg(key,value) AS costs
           FROM combined_costs GROUP BY bucket_start
         )
         SELECT bucket_start,counts,coalesce(costs,'{{}}'::jsonb)
         FROM totals LEFT JOIN costs USING(bucket_start) ORDER BY bucket_start
         ) TO STDOUT"
    )
}

pub(super) async fn restored_metrics(
    connection: &mut PgConnection,
    range: ObservabilityRange,
    filter: &UsageRecordFilter,
) -> StoreResult<RequestMetrics> {
    if *filter != UsageRecordFilter::default() {
        return Ok(RequestMetrics::default());
    }
    let value: serde_json::Value = sqlx::query_scalar(
        "select coalesce(jsonb_object_agg(key,value),'{}'::jsonb) from (
           select c.key,sum(c.value::numeric) as value
           from backup_usage_hours cross join lateral jsonb_each_text(counts) c
           where bucket_start >= $1 and bucket_start < $2 group by c.key
         ) totals",
    )
    .bind(range.start)
    .bind(range.end)
    .fetch_one(connection)
    .await
    .map_err(|error| postgres_unavailable("load restored usage").with_source(error))?;
    decode_metrics(&value)
}

fn decode_metrics(value: &serde_json::Value) -> StoreResult<RequestMetrics> {
    let mut result = RequestMetrics::default();
    macro_rules! read {
        ($($field:ident),+ $(,)?) => {$(
            result.$field = match value.get(stringify!($field)) {
                None => 0,
                Some(value) => value.as_u64().ok_or_else(|| invalid("invalid restored usage counter"))?,
            };
        )+};
    }
    read!(
        request_count,
        success_count,
        failure_count,
        cancelled_count,
        incomplete_count,
        caller_error_count,
        input_tokens,
        output_tokens,
        cached_tokens,
        cache_write_tokens,
        reasoning_tokens,
        total_tokens
    );
    Ok(result)
}

pub(super) fn add_metrics(to: &mut RequestMetrics, from: &RequestMetrics) -> StoreResult<()> {
    macro_rules! add {
        ($($field:ident),+ $(,)?) => {$(
            to.$field = to.$field.checked_add(from.$field)
                .ok_or_else(|| invalid("restored usage counter overflow"))?;
        )+};
    }
    add!(
        request_count,
        success_count,
        failure_count,
        cancelled_count,
        incomplete_count,
        caller_error_count,
        input_tokens,
        output_tokens,
        cached_tokens,
        cache_write_tokens,
        reasoning_tokens,
        total_tokens
    );
    Ok(())
}

pub(super) async fn add_series(
    connection: &mut PgConnection,
    range: ObservabilityRange,
    filter: &UsageRecordFilter,
    granularity: ObservationGranularity,
    points: &mut BTreeMap<DateTime<Utc>, RequestMetricPoint>,
) -> StoreResult<()> {
    if *filter != UsageRecordFilter::default() {
        return Ok(());
    }
    let rows = sqlx::query(
        "select bucket_start,counts from backup_usage_hours
         where bucket_start >= $1 and bucket_start < $2 order by bucket_start",
    )
    .bind(range.start)
    .bind(range.end)
    .fetch_all(connection)
    .await
    .map_err(|error| postgres_unavailable("load restored usage trend").with_source(error))?;
    for row in rows {
        let bucket: DateTime<Utc> = get(&row, "bucket_start")?;
        let seconds = granularity.seconds();
        let bucket = DateTime::from_timestamp(bucket.timestamp().div_euclid(seconds) * seconds, 0)
            .ok_or_else(|| invalid("restored usage time overflow"))?;
        let point = points.entry(bucket).or_insert_with(|| RequestMetricPoint {
            bucket_start: bucket,
            granularity,
            metrics: RequestMetrics::default(),
            cost_coverage: CostCoverage::default(),
            costs: Vec::new(),
        });
        add_metrics(&mut point.metrics, &decode_metrics(&get(&row, "counts")?)?)?;
    }
    Ok(())
}
