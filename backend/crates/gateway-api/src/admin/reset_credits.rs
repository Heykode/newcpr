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
use serde::Deserialize;
use uuid::Uuid;

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/accounts/reset-credits/cache", post(cache::<S>))
        .route(
            "/api/admin/accounts/reset-credits/refresh",
            post(refresh::<S>),
        )
        .route(
            "/api/admin/accounts/reset-credits/preview",
            post(preview::<S>),
        )
        .route(
            "/api/admin/accounts/reset-credits/batches",
            get(batches::<S>),
        )
        .route(
            "/api/admin/accounts/reset-credits/confirm",
            post(confirm::<S>),
        )
        .route("/api/admin/accounts/reset-credits/retry", post(retry::<S>))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selection {
    account_ids: Vec<String>,
    reset_type: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Confirm {
    id: Uuid,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Retry {
    id: Uuid,
    account_id: String,
}

async fn cache<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(command): AdminJson<Selection>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .reset_credits()
        .inventories(command.account_ids)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}
async fn refresh<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(command): AdminJson<Selection>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .reset_credits()
        .refresh(command.account_ids, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}
async fn preview<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(command): AdminJson<Selection>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .reset_credits()
        .preview(
            command.account_ids,
            command.reset_type,
            &auth.context().mutation_context(),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}
async fn batches<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .reset_credits()
        .batches()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}
async fn confirm<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(command): AdminJson<Confirm>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .reset_credits()
        .confirm(command.id, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::ACCEPTED,
        AdminEnvelope::ok(data),
    ))
}
async fn retry<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(command): AdminJson<Retry>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .reset_credits()
        .retry(
            command.id,
            &command.account_id,
            &auth.context().mutation_context(),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::ACCEPTED,
        AdminEnvelope::ok(()),
    ))
}
