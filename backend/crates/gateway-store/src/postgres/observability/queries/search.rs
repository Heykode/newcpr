//! Shared read-only predicates. Event-local facts never borrow a final attempt.

use super::super::*;
use admin_observability::RequestSearchFilter;

pub(crate) async fn account_filter_options(
    pool: &PgPool,
    range: ObservabilityRange,
    search: &str,
) -> StoreResult<Vec<admin_observability::AccountFilterOption>> {
    validate_optional_text(
        (!search.is_empty()).then_some(search),
        MAX_SEARCH_BYTES,
        "account search",
    )?;
    // No credentials or capture bodies are selected, including for deleted accounts.
    let rows = sqlx::query(
        "with candidates as (
           select id, email, name, custom_name, false as deleted, updated_at as seen
           from provider_accounts
           where concat_ws(' ', id, email, name, custom_name) ilike $3 escape '\\'
           union all
           select provider_account_ref, provider_account_email_snapshot,
                  provider_account_name_snapshot, null, true, started_at
           from model_requests
           where started_at >= $1 and started_at < $2 and provider_account_ref is not null
             and concat_ws(' ', provider_account_ref, provider_account_email_snapshot,
                         provider_account_name_snapshot) ilike $3 escape '\\'
           union all
           select provider_account_ref, provider_account_email_snapshot,
                  provider_account_name_snapshot, null, true, created_at
           from ops_events
           where created_at >= $1 and created_at < $2 and provider_account_ref is not null
             and concat_ws(' ', provider_account_ref, provider_account_email_snapshot,
                         provider_account_name_snapshot) ilike $3 escape '\\'
         ), matched as (
           select distinct on (id) id, email, name, custom_name, deleted, seen
           from candidates order by id, deleted, seen desc
         )
         select m.id, coalesce(pa.email,m.email) as email, coalesce(pa.name,m.name) as name,
                coalesce(pa.custom_name,m.custom_name) as custom_name,
                (pa.id is null) as deleted
         from matched m left join provider_accounts pa on pa.id = m.id
         order by (m.id = $4) desc, (pa.id is null), m.seen desc, m.id limit 50",
    )
    .bind(range.start)
    .bind(range.end)
    .bind(format!("%{}", literal_prefix_pattern(search)))
    .bind(search)
    .fetch_all(pool)
    .await
    .map_err(|_| postgres_unavailable("account filter options"))?;
    rows.iter()
        .map(|row| {
            Ok(admin_observability::AccountFilterOption {
                id: get(row, "id")?,
                email: get(row, "email")?,
                name: get(row, "name")?,
                custom_name: get(row, "custom_name")?,
                deleted: get(row, "deleted")?,
            })
        })
        .collect()
}

pub(crate) fn validate_search_details(filter: &RequestSearchFilter) -> StoreResult<()> {
    if filter.account_ids.len() > 50 {
        return Err(invalid("too many account filters"));
    }
    for id in &filter.account_ids {
        validate_text(id, MAX_FILTER_BYTES, "account filter")?;
    }
    for value in [
        &filter.account_search,
        &filter.group_id,
        &filter.requested_model,
        &filter.upstream_model,
        &filter.upstream_mode,
        &filter.client_transport,
        &filter.upstream_transport,
        &filter.client_ip,
        &filter.failure_kind,
        &filter.error_code,
        &filter.error_phase,
        &filter.recovery,
        &filter.error_scope,
        &filter.cache_match,
    ] {
        validate_optional_text(value.as_deref(), MAX_FILTER_BYTES, "request filter")?;
    }
    for code in [filter.client_status_code, filter.upstream_status_code]
        .into_iter()
        .flatten()
    {
        if !(100..=599).contains(&code) {
            return Err(invalid("invalid status code"));
        }
    }
    if filter
        .client_ip
        .as_deref()
        .is_some_and(|ip| ip.parse::<std::net::IpAddr>().is_err())
    {
        return Err(invalid("invalid client IP"));
    }
    for (min, max) in [
        (filter.min_latency_ms, filter.max_latency_ms),
        (filter.min_first_token_ms, filter.max_first_token_ms),
    ] {
        if min.zip(max).is_some_and(|(min, max)| min > max)
            || min.is_some_and(|v| v > i64::MAX as u64)
            || max.is_some_and(|v| v > i64::MAX as u64)
        {
            return Err(invalid("invalid timing range"));
        }
    }
    Ok(())
}

pub(crate) fn push_contains(query: &mut QueryBuilder<Postgres>, columns: &[String], text: &str) {
    let pattern = format!("%{}", literal_prefix_pattern(text));
    query.push(" and (");
    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            query.push(" or ");
        }
        query.push(format!("{column} ilike "));
        query.push_bind(pattern.clone()).push(" escape '\\'");
    }
    query.push(")");
}

fn equality(query: &mut QueryBuilder<Postgres>, column: &str, value: &Option<String>) {
    if let Some(value) = value {
        query
            .push(format!(" and {column} = "))
            .push_bind(value.clone());
    }
}

pub(crate) fn push_request_search(
    query: &mut QueryBuilder<Postgres>,
    filter: &RequestSearchFilter,
    request: &str,
    event: Option<&str>,
) {
    let owner = event.unwrap_or(request);
    let transport = event.map_or_else(
        || format!("{request}.upstream_transport"),
        |_| "null::text".to_owned(),
    );
    let status = event.map_or_else(
        || format!("{request}.upstream_status_code"),
        |e| format!("{e}.status_code"),
    );
    let failure = event.map_or_else(
        || format!("{request}.error_kind"),
        |e| format!("{e}.failure_kind"),
    );
    if !filter.account_ids.is_empty() {
        query
            .push(format!(" and {owner}.provider_account_ref = any("))
            .push_bind(filter.account_ids.clone())
            .push("::text[])");
    }
    if let Some(search) = &filter.account_search {
        push_contains(
            query,
            &[
                format!("{owner}.provider_account_ref"),
                format!("{owner}.provider_account_email_snapshot"),
                format!("{owner}.provider_account_name_snapshot"),
                format!(
                    "(select concat_ws(' ', pa.name, pa.email, pa.custom_name) from provider_accounts pa where pa.id = {owner}.provider_account_ref)"
                ),
            ],
            search,
        );
    }
    if let Some(group) = &filter.group_id {
        query
            .push(" and ")
            .push_bind(group.clone())
            .push(format!(" = any({request}.routing_group_refs)"));
    }
    equality(
        query,
        &format!("{request}.requested_model_id"),
        &filter.requested_model,
    );
    equality(
        query,
        &format!("{owner}.upstream_model_id"),
        &filter.upstream_model,
    );
    equality(
        query,
        &format!("{request}.client_transport"),
        &filter.client_transport,
    );
    if let Some(ip) = &filter.client_ip {
        query
            .push(format!(" and {request}.client_ip = "))
            .push_bind(ip.clone())
            .push("::inet");
    }
    equality(query, &failure, &filter.failure_kind);
    equality(
        query,
        &format!("{owner}.provider_error_code"),
        &filter.error_code,
    );
    match filter.upstream_mode.as_deref() {
        Some("excel") => {
            query.push(format!(" and {transport} = 'excel_http_sse'"));
        }
        Some("codex") => {
            query.push(format!(" and {owner}.provider_kind = 'openai' and {transport} in ('http', 'http_sse', 'websocket')"));
        }
        Some("unknown") => {
            query.push(format!(" and {transport} is null"));
        }
        _ => {}
    }
    match filter.upstream_transport.as_deref() {
        Some("http_sse") => {
            query.push(format!(
                " and {transport} in ('http_sse', 'excel_http_sse')"
            ));
        }
        Some("unknown") => {
            query.push(format!(" and {transport} is null"));
        }
        Some(_) => equality(query, &transport, &filter.upstream_transport),
        None => {}
    }
    if let Some(code) = filter.client_status_code {
        // An event itself has no downstream response status.
        if event.is_some() {
            query.push(" and false");
        } else {
            query
                .push(format!(" and {request}.client_status_code = "))
                .push_bind(i32::from(code));
        }
    }
    if let Some(code) = filter.upstream_status_code {
        query
            .push(format!(" and {status} = "))
            .push_bind(i32::from(code));
    }
    for (column, min, max) in [
        (
            format!("{owner}.latency_ms"),
            filter.min_latency_ms,
            filter.max_latency_ms,
        ),
        (
            event.map_or_else(
                || format!("{request}.first_token_ms"),
                |_| "null::bigint".to_owned(),
            ),
            filter.min_first_token_ms,
            filter.max_first_token_ms,
        ),
    ] {
        if let Some(min) = min {
            query
                .push(format!(" and {column} >= "))
                .push_bind(min as i64);
        }
        if let Some(max) = max {
            query
                .push(format!(" and {column} <= "))
                .push_bind(max as i64);
        }
    }
    if let Some(cache) = &filter.cache_match {
        if event.is_some() {
            query.push(" and false");
        } else {
            match cache.as_str() {
                "hit" => {
                    query.push(format!(" and {request}.cached_tokens > 0"));
                }
                "miss" => {
                    query.push(format!(" and {request}.cached_tokens = 0"));
                }
                "unknown" => {
                    query.push(format!(" and {request}.cached_tokens is null"));
                }
                _ => {}
            }
        }
    }
    let recovered =
        format!("({request}.recovered_at is not null or {request}.outcome = 'succeeded')");
    match filter.recovery.as_deref() {
        Some("recovered") => {
            query.push(format!(" and {recovered}"));
        }
        Some("unrecovered") => {
            query.push(format!(" and not {recovered}"));
        }
        _ => {}
    }
    if let Some(phase) = &filter.error_phase {
        let attempt = event.map_or_else(
            || format!("{request}.attempt_count"),
            |e| format!("{e}.attempt_index"),
        );
        query
            .push(format!(
                " and exists (select 1 from jsonb_array_elements(
             coalesce({request}.diagnostic_trace_json->'events', '[]'::jsonb)) trace
             where trace->>'stage' = 'attempt.failed'
               and trace->>'attemptIndex' = {attempt}::text
               and trace #>> '{{data,diagnostic,stage}}' = "
            ))
            .push_bind(phase.clone())
            .push(")");
    }
}
