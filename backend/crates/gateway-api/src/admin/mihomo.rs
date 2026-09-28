//! Authenticated control plane; user requests never enter this module.

use super::{AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminResponse, AdminSessionState};
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use gateway_admin::model::mihomo::MihomoCommand;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NodeCheckRequest {
    node: String,
    #[serde(default)]
    quality: bool,
}

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/admin/proxies/mihomo",
            get(status::<S>).post(submit::<S>),
        )
        .route("/api/admin/proxies/mihomo/check", post(check::<S>))
}

fn unavailable() -> AdminError {
    AdminError::bad_request("受管代理服务未配置")
}

async fn status<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let services = state.admin_services();
    let status = services
        .mihomo()
        .ok_or_else(unavailable)?
        .status()
        .await
        .map_err(super::wire::map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(status),
    ))
}

async fn submit<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(command): AdminJson<MihomoCommand>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let services = state.admin_services();
    let result = services
        .mihomo()
        .ok_or_else(unavailable)?
        .submit(command, &auth.context().mutation_context())
        .await
        .map_err(super::wire::map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::ACCEPTED,
        AdminEnvelope::ok(result),
    ))
}

async fn check<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<NodeCheckRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let services = state.admin_services();
    let result = services
        .mihomo()
        .ok_or_else(unavailable)?
        .test_node(
            &request.node,
            request.quality,
            &auth.context().mutation_context(),
        )
        .await
        .map_err(super::wire::map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}
