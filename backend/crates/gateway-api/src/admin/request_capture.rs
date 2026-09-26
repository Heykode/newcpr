use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminQuery, AdminResponse, AdminSessionState,
    wire::map_admin_service_error,
};
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::StreamExt;
use gateway_admin::model::request_capture::{CreateCaptureTask, RequestCaptureConfig};
use serde::Deserialize;

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/admin/request-captures",
            get(status::<S>).post(create::<S>),
        )
        .route("/api/admin/request-captures/config", post(configure::<S>))
        .route("/api/admin/request-captures/stop", post(stop::<S>))
        .route("/api/admin/request-captures/export", get(export_task::<S>))
        .route("/api/admin/request-captures/delete", post(delete::<S>))
        .route("/api/admin/request-captures/records", get(read::<S>))
        .route(
            "/api/admin/request-captures/records/export",
            get(export::<S>),
        )
}

async fn status<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .request_capture()
        .status()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}
async fn configure<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(config): AdminJson<RequestCaptureConfig>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .request_capture()
        .configure(config, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}
async fn create<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(task): AdminJson<CreateCaptureTask>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let task = state
        .admin_services()
        .request_capture()
        .create(task, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::CREATED,
        AdminEnvelope::ok(task),
    ))
}
async fn stop<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(CaptureId { id }): AdminJson<CaptureId>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .request_capture()
        .stop(&id, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}
async fn delete<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(CaptureId { id }): AdminJson<CaptureId>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .request_capture()
        .delete(&id, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureId {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    id: String,
    #[serde(default)]
    offset: u64,
}
async fn read<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<PageQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let page = state
        .admin_services()
        .request_capture()
        .read(&query.id, query.offset, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(page)))
}
async fn export<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(CaptureId { id }): AdminQuery<CaptureId>,
) -> Result<Response, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let stream = state
        .admin_services()
        .request_capture()
        .export(&id, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(export_response(stream))
}

async fn export_task<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(CaptureId { id }): AdminQuery<CaptureId>,
) -> Result<Response, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let stream = state
        .admin_services()
        .request_capture()
        .export_task(&id, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(export_response(stream))
}

fn export_response(stream: gateway_admin::ports::request_capture::CaptureExport) -> Response {
    let stream =
        stream.map(|item| item.map_err(|_| std::io::Error::other("capture export interrupted")));
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/x-ndjson".parse().unwrap(),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        "attachment; filename=\"error-capture.jsonl\""
            .parse()
            .unwrap(),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
