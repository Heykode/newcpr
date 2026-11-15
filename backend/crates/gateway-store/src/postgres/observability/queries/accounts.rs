//! Provider 账号用量查询族

use super::super::*;

pub(crate) async fn provider_account_usage(
    snapshot: &mut analytics::AnalyticsSnapshot<'_>,
    query: ProviderAccountUsageQuery,
) -> StoreResult<Vec<ProviderAccountUsageObservation>> {
    if let Some(account_ids) = &query.account_ids {
        validate_account_ids(account_ids)?;
    }
    if query.limit == 0 || usize::from(query.limit) > MAX_ACCOUNT_IDS {
        return Err(invalid("account usage limit must be between 1 and 200"));
    }

    // 账号卡片先确定有限候选，避免为少量账号聚合整个历史范围
    let mut statement = QueryBuilder::<Postgres>::new("select pa.* from provider_accounts pa");
    if let Some(account_ids) = &query.account_ids {
        statement
            .push(" where pa.id=any(")
            .push_bind(account_ids.clone())
            .push("::text[])");
    } else {
        statement.push(" join lateral (select mr.started_at from model_requests mr where mr.provider_account_ref=pa.id and mr.started_at >= ").push_bind(query.range.start)
            .push(" and mr.started_at < ").push_bind(query.range.end);
        push_completed_usage_fact_filter(&mut statement, "mr");
        statement.push(" order by mr.started_at desc,mr.id desc limit 1) latest on true order by latest.started_at desc,pa.name,pa.id limit ").push_bind(i64::from(query.limit));
    }
    let rows = statement
        .build()
        .fetch_all(&mut *snapshot.tx)
        .await
        .map_err(|error| postgres_unavailable("select usage accounts").with_source(error))?;
    let account_ids = rows
        .iter()
        .map(|row| get(row, "id"))
        .collect::<StoreResult<Vec<String>>>()?;
    let mut models_by_account = HashMap::<String, Vec<ProviderAccountModelUsageObservation>>::new();
    let mut totals = BTreeMap::<String, analytics::MetricState>::new();
    for ((account, model), state) in snapshot
        .account_usage_states(query.range, &account_ids)
        .await?
    {
        totals.entry(account.clone()).or_default().merge(&state)?;
        if !model.is_empty() {
            models_by_account.entry(account).or_default().push(
                ProviderAccountModelUsageObservation {
                    model,
                    success_count: state.count("completed_request_count"),
                    request_count: state.count("completed_request_count"),
                    health: state.request_health(),
                    input_tokens: state.optional_count("input_tokens"),
                    output_tokens: state.optional_count("output_tokens"),
                    cached_tokens: state.optional_count("cached_tokens"),
                    cache_write_tokens: state.optional_count("cache_write_tokens"),
                    reasoning_tokens: state.optional_count("reasoning_tokens"),
                    image_input_tokens: state.optional_count("image_input_tokens"),
                    image_output_tokens: state.optional_count("image_output_tokens"),
                    total_tokens: state.optional_count("total_tokens"),
                    image_request_count: state.count("image_request_count"),
                    image_request_failed_count: state.count("image_request_failed_count"),
                    cost_coverage: state.coverage(),
                    costs: state.costs()?,
                    last_used_at: state.last_used_at(),
                },
            );
        }
    }
    let mut request_buckets = if let Some(duration) = query.request_bucket_duration {
        provider_account_request_buckets(&mut snapshot.tx, query.range, &account_ids, duration)
            .await?
    } else {
        HashMap::new()
    };
    let mut observations = Vec::with_capacity(rows.len());
    for row in rows {
        let account_id: String = get(&row, "id")?;
        let state = totals.remove(&account_id).unwrap_or_default();
        let mut models = models_by_account.remove(&account_id).unwrap_or_default();
        models.sort_by(|a, b| {
            b.request_count
                .cmp(&a.request_count)
                .then(a.model.cmp(&b.model))
        });
        observations.push(ProviderAccountUsageObservation {
            provider_kind: get(&row, "provider_kind")?,
            authentication_kind: get(&row, "authentication_kind")?,
            name: get(&row, "name")?,
            email: get(&row, "email")?,
            plan_type: get(&row, "plan_type")?,
            request_count: state.count("completed_request_count"),
            success_count: state.count("completed_request_count"),
            input_tokens: state.optional_count("input_tokens"),
            output_tokens: state.optional_count("output_tokens"),
            cached_tokens: state.optional_count("cached_tokens"),
            cache_write_tokens: state.optional_count("cache_write_tokens"),
            reasoning_tokens: state.optional_count("reasoning_tokens"),
            image_input_tokens: state.optional_count("image_input_tokens"),
            image_output_tokens: state.optional_count("image_output_tokens"),
            total_tokens: state.optional_count("total_tokens"),
            image_request_count: state.count("image_request_count"),
            image_request_failed_count: state.count("image_request_failed_count"),
            cost_coverage: state.coverage(),
            costs: state.costs()?,
            last_used_at: state.last_used_at(),
            request_buckets: request_buckets.remove(&account_id).unwrap_or_default(),
            models,
            account_id,
        });
    }
    observations.sort_by(|a, b| {
        b.last_used_at
            .cmp(&a.last_used_at)
            .then(a.name.cmp(&b.name))
            .then(a.account_id.cmp(&b.account_id))
    });
    observations.truncate(usize::from(query.limit));
    Ok(observations)
}

pub(crate) async fn provider_account_request_buckets(
    connection: &mut PgConnection,
    range: ObservabilityRange,
    account_ids: &[String],
    bucket_duration: TimeDelta,
) -> StoreResult<HashMap<String, Vec<ProviderAccountRequestBucket>>> {
    // Business account health includes terminal failures, but not quality retests.
    // Five-minute buckets keep the account page responsive while preserving a short
    // recent-health projection for the UI.
    let bucket_seconds = bucket_duration.num_seconds();
    if bucket_seconds <= 0 {
        return Err(invalid("account request timeline bucket must be positive"));
    }
    let business = business_request_predicate("mr");
    let statement = format!("select provider_account_ref,
                floor(extract(epoch from (started_at - $1)) / {bucket_seconds})::bigint as bucket_index,
                count(*)::bigint as request_count,
                count(*) filter (where outcome = 'succeeded')::bigint as success_count,
                count(*) filter (where outcome = 'failed')::bigint
                  as error_count,
                count(*) filter (where outcome in ('cancelled', 'incomplete'))::bigint
                  as non_completion_count
         from model_requests mr
         where mr.provider_account_ref = any($2::text[])
           and mr.started_at >= $1 and mr.started_at < $3
           and mr.outcome <> 'running'
           and mr.recovered_at is null
           and ({business})
         group by mr.provider_account_ref, bucket_index
         order by mr.provider_account_ref, bucket_index");
    let rows = sqlx::query(sqlx::AssertSqlSafe(statement))
        .bind(range.start)
        .bind(account_ids)
        .bind(range.end)
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| {
            postgres_unavailable("load provider account request timeline").with_source(error)
        })?;

    let mut observed = HashMap::<String, BTreeMap<u64, (u64, u64, u64, u64)>>::new();
    for row in rows {
        observed
            .entry(get(&row, "provider_account_ref")?)
            .or_default()
            .insert(
                unsigned(&row, "bucket_index")?,
                (
                    unsigned(&row, "request_count")?,
                    unsigned(&row, "success_count")?,
                    unsigned(&row, "error_count")?,
                    unsigned(&row, "non_completion_count")?,
                ),
            );
    }

    let step = bucket_duration;
    let mut timelines = HashMap::with_capacity(account_ids.len());
    for account_id in account_ids {
        let mut account_observed = observed.remove(account_id).unwrap_or_default();
        let mut bucket_start = range.start;
        let mut bucket_index = 0_u64;
        let mut buckets = Vec::new();
        while bucket_start < range.end {
            let (request_count, success_count, error_count, non_completion_count) =
                account_observed.remove(&bucket_index).unwrap_or_default();
            buckets.push(ProviderAccountRequestBucket {
                bucket_start,
                request_count,
                success_count,
                error_count,
                non_completion_count,
            });
            bucket_start = bucket_start
                .checked_add_signed(step)
                .ok_or_else(|| invalid("account request timeline exceeds supported timestamps"))?;
            bucket_index = bucket_index
                .checked_add(1)
                .ok_or_else(|| invalid("account request timeline exceeds supported buckets"))?;
        }
        timelines.insert(account_id.clone(), buckets);
    }
    Ok(timelines)
}
