//! Admin-authenticated relogin library. No secret-bearing response DTOs.

use super::wire::map_admin_service_error;
use super::{AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminResponse, AdminSessionState};
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use gateway_admin::model::relogin::{ReloginSettings, ReloginTarget};
use gateway_admin::model::relogin_templates::{ReloginTemplateConfig, ReloginTemplateSelection};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportRequest {
    text: String,
    #[serde(default)]
    replace_existing: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BatchRequest {
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AccountQueueRequest {
    entry_id: String,
    revision: u64,
    target: ReloginTarget,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PushRequest {
    custom_name: Option<String>,
    ids: Vec<String>,
    revisions: std::collections::BTreeMap<String, u64>,
    template: Option<ReloginTemplateSelection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TemplateSaveRequest {
    selection: Option<ReloginTemplateSelection>,
    config: ReloginTemplateConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AutomaticRequest {
    ids: Vec<String>,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceRequest {
    id: String,
    workspace_id: Option<String>,
}

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/relogin", get(list::<S>))
        .route("/api/admin/relogin/import", post(import::<S>))
        .route("/api/admin/relogin/queue", post(queue::<S>))
        .route("/api/admin/relogin/push", post(push::<S>))
        .route("/api/admin/relogin/delete", post(delete::<S>))
        .route("/api/admin/relogin/automatic", post(automatic::<S>))
        .route("/api/admin/relogin/workspace", post(workspace::<S>))
        .route("/api/admin/relogin/settings", post(settings::<S>))
        .route("/api/admin/relogin/templates", get(templates::<S>))
        .route(
            "/api/admin/relogin/templates/save",
            post(save_template::<S>),
        )
        .route(
            "/api/admin/relogin/templates/delete",
            post(delete_template::<S>),
        )
        .route(
            "/api/admin/relogin/accounts/query",
            post(account_actions::<S>),
        )
        .route(
            "/api/admin/relogin/accounts/queue",
            post(queue_account::<S>),
        )
}

async fn account_actions<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<BatchRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .relogin()
        .account_actions(&request.ids)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn queue_account<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<AccountQueueRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .relogin()
        .queue_account(
            &request.entry_id,
            request.revision,
            &request.target,
            &auth.context().mutation_context(),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}

async fn list<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .relogin()
        .list()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn import<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<ImportRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let count = state
        .admin_services()
        .relogin()
        .import(&request.text, request.replace_existing)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(serde_json::json!({"imported": count})),
    ))
}

async fn queue<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<BatchRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .relogin()
        .queue(&request.ids)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn push<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<PushRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .relogin()
        .push_with_template(
            &request.ids,
            &request.revisions,
            request.template,
            request.custom_name,
            &auth.context().mutation_context(),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn templates<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .account_templates()
        .templates()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn save_template<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<TemplateSaveRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .account_templates()
        .save_template(request.selection, request.config)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn delete_template<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<ReloginTemplateSelection>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .account_templates()
        .delete_template(request)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}

async fn delete<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<BatchRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .relogin()
        .delete(&request.ids)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}

async fn automatic<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<AutomaticRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .relogin()
        .automatic(&request.ids, request.enabled)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}

async fn workspace<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<WorkspaceRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .relogin()
        .workspace(&request.id, request.workspace_id)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}

async fn settings<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<ReloginSettings>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .relogin()
        .configure(request)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}
