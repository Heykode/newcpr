use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use gateway_admin::model::quality_ops::{
    QualityGroupFilter, QualityRuleConfig, QualityTemplateTarget,
};
use serde::Deserialize;

use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminQuery, AdminResponse, AdminSessionState,
    wire::map_admin_service_error,
};

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/quality-ops/groups", get(groups::<S>))
        .route("/api/admin/quality-ops/groups/save", post(save_group::<S>))
        .route(
            "/api/admin/quality-ops/groups/delete",
            post(delete_group::<S>),
        )
        .route("/api/admin/quality-ops/rules", get(rules::<S>))
        .route("/api/admin/quality-ops/save", post(save::<S>))
        .route("/api/admin/quality-ops/delete", post(delete::<S>))
        .route("/api/admin/quality-ops/run", post(enqueue::<S>))
        .route("/api/admin/quality-ops/runs", get(runs::<S>))
        .route("/api/admin/quality-ops/detail", get(detail::<S>))
        .route("/api/admin/quality-ops/templates", get(templates::<S>))
        .route(
            "/api/admin/quality-ops/templates/save",
            post(save_template::<S>),
        )
        .route(
            "/api/admin/quality-ops/templates/delete",
            post(delete_template::<S>),
        )
        .route(
            "/api/admin/quality-ops/templates/apply",
            post(apply_template::<S>),
        )
        .route("/api/admin/quality-ops/monitoring", post(monitoring::<S>))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveGroup {
    id: Option<String>,
    revision: Option<i64>,
    name: String,
    filter: QualityGroupFilter,
    config: QualityRuleConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteGroup {
    id: String,
    revision: i64,
    delete_rules: bool,
}

async fn groups<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .quality_ops()
        .group_rules()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn save_group<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(body): AdminJson<SaveGroup>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .quality_ops()
        .save_group_rule(
            body.id.as_deref(),
            body.revision,
            body.name,
            body.filter,
            body.config,
            &auth.context().mutation_context(),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn delete_group<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(body): AdminJson<DeleteGroup>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .quality_ops()
        .delete_group_rule(
            &body.id,
            body.revision,
            body.delete_rules,
            &auth.context().mutation_context(),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveTemplate {
    id: Option<String>,
    revision: Option<i64>,
    name: String,
    config: QualityRuleConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyTemplate {
    id: String,
    revision: i64,
    targets: Vec<QualityTemplateTarget>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MonitoringQuery {
    account_ids: Vec<String>,
}

async fn templates<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .quality_ops()
        .templates()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn save_template<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(body): AdminJson<SaveTemplate>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .quality_ops()
        .save_template(
            body.id.as_deref(),
            body.revision,
            body.name,
            body.config,
            &auth.context().mutation_context(),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn delete_template<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(body): AdminJson<RuleCommand>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .quality_ops()
        .delete_template(&body.id, body.revision, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}

async fn apply_template<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(body): AdminJson<ApplyTemplate>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .quality_ops()
        .apply_template(
            &body.id,
            body.revision,
            body.targets,
            &auth.context().mutation_context(),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn monitoring<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(body): AdminJson<MonitoringQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .quality_ops()
        .monitoring(&body.account_ids)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveRule {
    id: Option<String>,
    revision: Option<i64>,
    config: QualityRuleConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleCommand {
    id: String,
    revision: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdQuery {
    id: String,
}

async fn rules<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .quality_ops()
        .rules()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn save<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(body): AdminJson<SaveRule>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .quality_ops()
        .save(
            body.id.as_deref(),
            body.revision,
            body.config,
            &auth.context().mutation_context(),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn delete<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(body): AdminJson<RuleCommand>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .quality_ops()
        .delete(&body.id, body.revision, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}

async fn enqueue<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(body): AdminJson<RuleCommand>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .quality_ops()
        .enqueue(&body.id, body.revision, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::ACCEPTED,
        AdminEnvelope::ok(()),
    ))
}

async fn runs<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<IdQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .quality_ops()
        .runs(&query.id)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn detail<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<IdQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .quality_ops()
        .detail(&query.id)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}
