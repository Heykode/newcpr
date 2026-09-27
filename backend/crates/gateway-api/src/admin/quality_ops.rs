use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use gateway_admin::model::quality_ops::QualityRuleConfig;
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
        .route("/api/admin/quality-ops/rules", get(rules::<S>))
        .route("/api/admin/quality-ops/save", post(save::<S>))
        .route("/api/admin/quality-ops/delete", post(delete::<S>))
        .route("/api/admin/quality-ops/run", post(enqueue::<S>))
        .route("/api/admin/quality-ops/runs", get(runs::<S>))
        .route("/api/admin/quality-ops/detail", get(detail::<S>))
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
