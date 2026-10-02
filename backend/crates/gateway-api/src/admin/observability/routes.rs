//! 固定 `/api/admin` 观测路由与 handler。

use super::*;

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/dashboard/summary", get(dashboard_summary::<S>))
        .route("/api/admin/dashboard/trend", get(dashboard_trend::<S>))
        .route("/api/admin/usage/records", get(usage_records::<S>))
        .route(
            "/api/admin/usage/account-options",
            get(account_options::<S>),
        )
        .route(
            "/api/admin/usage/records/detail",
            get(usage_record_detail::<S>),
        )
        .route(
            "/api/admin/usage/records/summary",
            get(usage_records_summary::<S>),
        )
        .route(
            "/api/admin/usage/insights/overview",
            get(usage_insights_overview::<S>),
        )
        .route(
            "/api/admin/usage/insights/diagnostics",
            get(usage_insights_diagnostics::<S>),
        )
        .route("/api/admin/operations/errors", get(ops_errors::<S>))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountOptionsQuery {
    start_time: Option<String>,
    end_time: Option<String>,
    search: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountOptionView {
    id: String,
    email: Option<String>,
    name: Option<String>,
    custom_name: Option<String>,
    deleted: bool,
}

async fn account_options<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<AccountOptionsQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let range = usage_range(query.start_time.as_deref(), query.end_time.as_deref())
        .map_err(map_wire_error)?;
    let search = query.search.unwrap_or_default();
    if search.len() > 256 || search.chars().any(char::is_control) {
        return Err(map_wire_error(WireValidationError::new("search")));
    }
    let options = state
        .admin_services()
        .observability()
        .account_filter_options(range, search.trim())
        .await
        .map_err(map_service_error)?;
    let items: Vec<_> = options
        .into_iter()
        .map(|option| AccountOptionView {
            id: option.id,
            email: option.email,
            name: option.name,
            custom_name: option.custom_name,
            deleted: option.deleted,
        })
        .collect();
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(items)))
}

pub(crate) async fn dashboard_summary<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<DashboardQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let kind = query.trend_kind().map_err(map_wire_error)?;
    // 概览默认按中国时区当日统计，与单独趋势接口保持同一口径。
    let range = dashboard_today_range_with_timezone(
        query.start_time.as_deref(),
        query.end_time.as_deref(),
        state.deployment_timezone(),
    )
    .map_err(map_wire_error)?;
    let result = state
        .admin_services()
        .observability()
        .dashboard_summary(range, domain_trend_kind(kind))
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(dashboard_view_with_timezone(
            result,
            kind,
            state.deployment_timezone(),
        )),
    ))
}

pub(crate) async fn dashboard_trend<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<DashboardQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let kind = query.trend_kind().map_err(map_wire_error)?;
    let range = dashboard_today_range_with_timezone(
        query.start_time.as_deref(),
        query.end_time.as_deref(),
        state.deployment_timezone(),
    )
    .map_err(map_wire_error)?;
    let result = state
        .admin_services()
        .observability()
        .dashboard_trend(range, domain_trend_kind(kind))
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(trend_view_with_timezone(
            result,
            kind,
            state.deployment_timezone(),
        )),
    ))
}

pub(crate) async fn usage_records<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<UsageQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let command = usage_command(&query).map_err(map_wire_error)?;
    let result = state
        .admin_services()
        .observability()
        .usage_records(command)
        .await
        .map_err(map_service_error)?;
    let data = usage_page_view_with_timezone(result, state.deployment_timezone());
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}

pub(crate) async fn usage_record_detail<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<DetailQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    query.validate().map_err(map_wire_error)?;
    let result = state
        .admin_services()
        .observability()
        .usage_record_detail(query.id.trim())
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(usage_detail_view_with_timezone(
            result,
            state.deployment_timezone(),
        )),
    ))
}

pub(crate) async fn usage_records_summary<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<UsageQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let range = usage_range(query.start_time.as_deref(), query.end_time.as_deref())
        .map_err(map_wire_error)?;
    let filter = usage_filter(&query).map_err(map_wire_error)?;
    let result = state
        .admin_services()
        .observability()
        .usage_summary(range, filter)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(usage_summary_view(result)),
    ))
}

pub(crate) async fn usage_insights_overview<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<UsageQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let range = usage_range(query.start_time.as_deref(), query.end_time.as_deref())
        .map_err(map_wire_error)?;
    let filter = usage_filter(&query).map_err(map_wire_error)?;
    let result = state
        .admin_services()
        .observability()
        .usage_insights(range, filter)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(usage_insights_view(result)),
    ))
}

pub(crate) async fn usage_insights_diagnostics<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<DiagnosticsQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let dimension = query.dimension().map_err(map_wire_error)?;
    let page = query.page(dimension).map_err(map_wire_error)?;
    let range = usage_range(query.start_time.as_deref(), query.end_time.as_deref())
        .map_err(map_wire_error)?;
    let filter = domain::UsageFilter {
        details: query.details.filter().map_err(map_wire_error)?,
        provider_account_ref: non_empty(query.account_id),
        client_api_key_ref: non_empty(query.client_api_key_id),
        request_id: non_empty(query.request_id),
        operation: non_empty(query.route),
        transport: non_empty(query.transport),
        response_id: non_empty(query.response_id),
        upstream_request_id: non_empty(query.upstream_request_id),
        provider_kind: non_empty(query.provider),
        model: non_empty(query.model),
        status_code: parse_status(query.status_code).map_err(map_wire_error)?,
        search: non_empty(query.search),
        ..domain::UsageFilter::default()
    };
    let result = state
        .admin_services()
        .observability()
        .diagnostics(range, filter, domain_diagnostic_dimension(dimension), page)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(diagnostics_view(result, dimension)),
    ))
}

pub(crate) async fn ops_errors<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<OpsQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let command = ops_command(&query).map_err(map_wire_error)?;
    let result = state
        .admin_services()
        .observability()
        .ops_errors(command)
        .await
        .map_err(map_service_error)?;
    let data = ops_page_view_with_timezone(result, state.deployment_timezone());
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}
