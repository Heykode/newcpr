//! Event-sweep concurrency adapted from official CPR #537.

use super::super::*;

pub(crate) async fn request_concurrency(
    connection: &mut PgConnection,
    range: ObservabilityRange,
    filter: &UsageRecordFilter,
) -> StoreResult<Vec<admin_observability::UsageConcurrencyPoint>> {
    filter.validate()?;
    let granularity = granularity_for(range);
    let seconds = granularity.seconds();
    let observed_end = range.end.min(Utc::now());
    let mut bucket =
        DateTime::from_timestamp(range.start.timestamp().div_euclid(seconds) * seconds, 0)
            .ok_or_else(|| invalid("invalid concurrency bucket"))?;
    let mut boundaries = Vec::new();
    while bucket < range.end {
        boundaries.push(bucket);
        bucket = bucket
            .checked_add_signed(TimeDelta::seconds(seconds))
            .ok_or_else(|| invalid("invalid next concurrency bucket"))?;
    }
    // 按生命周期分开筛选，使终态请求复用 completed_at 索引，运行中请求沿用独立索引
    let mut query = QueryBuilder::<Postgres>::new(
        "with buckets as materialized (select greatest(bucket_start,",
    );
    query
        .push_bind(range.start)
        .push("::timestamptz) as effective_start,least(coalesce(lead(bucket_start) over(order by bucket_start),")
        .push_bind(range.end)
        .push("::timestamptz),")
        .push_bind(range.end)
        .push("::timestamptz) as bucket_end,bucket_start from unnest(")
        .push_bind(boundaries)
        .push("::timestamptz[]) input(bucket_start)), matching as (select mr.started_at,mr.completed_at from model_requests mr where mr.outcome <> 'running' and mr.completed_at > ")
        .push_bind(range.start)
        .push(" and mr.started_at < ")
        .push_bind(range.end);
    push_usage_filter(&mut query, filter, "mr");
    push_business_request_filter(&mut query, "mr");
    query
        .push(" union all select mr.started_at,")
        .push_bind(observed_end)
        .push("::timestamptz as completed_at from model_requests mr where mr.outcome = 'running' and mr.started_at < ")
        .push_bind(observed_end)
        .push(" and ")
        .push_bind(observed_end)
        .push("::timestamptz > ")
        .push_bind(range.start);
    push_usage_filter(&mut query, filter, "mr");
    push_business_request_filter(&mut query, "mr");
    // 每个请求只产生两个事件，桶边界只切分时间轴，避免请求与所有桶逐一连接
    // 同时刻先合并增减，再计算下一段并发，遵循左闭右开的执行区间
    query
        .push("), events as (select e.occurred_at,e.delta,null::timestamptz as bucket_start from matching m cross join lateral (values (greatest(m.started_at,")
        .push_bind(range.start)
        .push("::timestamptz),1::bigint),(least(m.completed_at,")
        .push_bind(range.end)
        .push("::timestamptz),-1::bigint)) e(occurred_at,delta) union all select effective_start,0::bigint,bucket_start from buckets union all select ")
        .push_bind(range.end)
        .push("::timestamptz,0::bigint,null::timestamptz),
            event_points as (
                select occurred_at,sum(delta)::bigint as delta,max(bucket_start) as bucket_start
                from events group by occurred_at
            ), timeline as (
                select occurred_at,sum(delta) over timeline_order::bigint as concurrency,
                    max(bucket_start) over timeline_order as bucket_start,
                    lead(occurred_at) over(order by occurred_at) as next_occurred_at
                from event_points
                window timeline_order as (order by occurred_at rows between unbounded preceding and current row)
            )
            select b.bucket_start,
                coalesce(sum(t.concurrency*extract(epoch from t.next_occurred_at-t.occurred_at))
                    /nullif(extract(epoch from b.bucket_end-b.effective_start),0),0)::float8 as average_concurrency,
                coalesce(max(t.concurrency),0)::bigint as peak_concurrency
            from buckets b left join timeline t on t.bucket_start=b.bucket_start and t.next_occurred_at>t.occurred_at
            group by b.bucket_start,b.effective_start,b.bucket_end order by b.bucket_start");
    let rows = query.build().fetch_all(connection).await.map_err(|error| {
        postgres_unavailable("aggregate request concurrency").with_source(error)
    })?;
    rows.iter()
        .map(|row| {
            Ok(admin_observability::UsageConcurrencyPoint {
                bucket_start: get(row, "bucket_start")?,
                average_concurrency: get(row, "average_concurrency")?,
                peak_concurrency: unsigned(row, "peak_concurrency")?,
            })
        })
        .collect()
}
