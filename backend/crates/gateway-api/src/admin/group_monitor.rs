//! Strict, authenticated read-only projection for the account overview.

use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use chrono::{DateTime, Utc};
use gateway_admin::model::{auth::AdminPrincipal, group_monitor::GroupMonitorItem};
use gateway_core::routing::AccountGroupId;
use serde::{Deserialize, Serialize};

use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminQuery, AdminResponse, AdminSessionState,
    wire::map_admin_service_error,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MonitorQuery {
    group_ids: String,
    #[serde(default)]
    refresh_forecasts: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MonitorView {
    viewer_scope: String,
    generated_at: DateTime<Utc>,
    refreshing: bool,
    pending_group_ids: Vec<String>,
    rate_window_seconds: u32,
    items: Vec<MonitorItemView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MonitorItemView {
    id: String,
    name: String,
    color: String,
    enabled: bool,
    total_accounts: u64,
    eligible_accounts: u64,
    estimated_accounts: u64,
    used_slots: Option<u64>,
    total_slots: u64,
    remaining_usd: Option<f64>,
    remaining_status: &'static str,
    expected_expiry_usd: Option<f64>,
    expiry_status: &'static str,
    consume_usd_per_minute: Option<f64>,
    quota_consume_usd_per_minute: Option<f64>,
    eta_minutes: Option<f64>,
    eta_status: &'static str,
    low_sample: bool,
    earliest_reset_at: Option<DateTime<Utc>>,
    active_alerts: Vec<String>,
}

impl From<GroupMonitorItem> for MonitorItemView {
    fn from(item: GroupMonitorItem) -> Self {
        Self {
            id: item.group.id.to_string(),
            name: item.group.name,
            color: item.group.color.as_str().to_owned(),
            enabled: item.group.enabled,
            total_accounts: item.total_accounts,
            eligible_accounts: item.eligible_accounts,
            estimated_accounts: item.estimated_accounts,
            used_slots: item.used_slots,
            total_slots: item.total_slots,
            remaining_usd: item.remaining_usd,
            remaining_status: item.remaining_status,
            expected_expiry_usd: item.expected_expiry_usd,
            expiry_status: item.expiry_status,
            consume_usd_per_minute: item.consume_usd_per_minute,
            quota_consume_usd_per_minute: item.quota_consume_usd_per_minute,
            eta_minutes: item.eta_minutes,
            eta_status: item.eta_status,
            low_sample: item.low_sample,
            earliest_reset_at: item.earliest_reset_at,
            active_alerts: Vec::new(),
        }
    }
}

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new().route("/api/admin/account-groups/monitor", get(read::<S>))
}

async fn read<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<MonitorQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    if query.group_ids.len() > 256 {
        return Err(AdminError::bad_request("分组查询不合法"));
    }
    let ids = query
        .group_ids
        .split(',')
        .map(|value| AccountGroupId::new(value.to_owned()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AdminError::bad_request("分组 ID 不合法"))?;
    let report = state
        .admin_services()
        .group_monitor()
        .read(ids, query.refresh_forecasts)
        .await
        .map_err(map_admin_service_error)?;
    let viewer_scope = match &auth.context().principal {
        AdminPrincipal::Session { admin_user_id } => format!("admin:{admin_user_id}"),
        AdminPrincipal::ApiKey => "admin-api-key".to_owned(),
    };
    // Notification storage must not make the monitor unavailable.
    let services = state.admin_services();
    let active = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        services.notifications().active_alerts(),
    )
    .await
    .ok()
    .and_then(Result::ok);
    let items = report
        .items
        .into_iter()
        .map(|item| {
            let mut view = MonitorItemView::from(item);
            if let Some(active) = &active {
                view.active_alerts = active
                    .iter()
                    .filter(|(id, _)| id == &view.id)
                    .map(|(_, kind)| kind.clone())
                    .collect();
            }
            view
        })
        .collect();
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(MonitorView {
            viewer_scope,
            generated_at: report.generated_at,
            refreshing: report.refreshing,
            pending_group_ids: report
                .pending_group_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
            rate_window_seconds: 60,
            items,
        }),
    ))
}
