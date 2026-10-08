use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminResponse, AdminSessionState,
    wire::map_admin_service_error,
};
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use gateway_admin::model::log_cleanup::{CleanupCommand, CleanupPreview};
use serde::Deserialize;

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/admin/log-cleanup",
            get(state::<S>).post(configure::<S>),
        )
        .route("/api/admin/log-cleanup/usage", get(usage::<S>))
        .route("/api/admin/log-cleanup/preview", post(preview::<S>))
        .route("/api/admin/log-cleanup/start", post(start::<S>))
        .route("/api/admin/log-cleanup/cancel", post(cancel::<S>))
        .route(
            "/api/admin/log-cleanup/captures/start",
            post(start_capture_clear::<S>),
        )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureClear {
    confirmed: bool,
}

async fn start_capture_clear<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(command): AdminJson<CaptureClear>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let job = state
        .admin_services()
        .log_cleanup()
        .start_capture_clear(command.confirmed, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::ACCEPTED,
        AdminEnvelope::ok(job),
    ))
}
async fn state<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .log_cleanup()
        .state()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}
async fn usage<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .log_cleanup()
        .footprint()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}
async fn configure<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(command): AdminJson<CleanupCommand>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .log_cleanup()
        .configure(command, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}
async fn preview<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .log_cleanup()
        .preview()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}
async fn start<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(preview): AdminJson<CleanupPreview>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .log_cleanup()
        .start(preview, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::ACCEPTED,
        AdminEnvelope::ok(data),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cancel {
    id: String,
}
async fn cancel<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(command): AdminJson<Cancel>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .log_cleanup()
        .cancel(&command.id, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}
