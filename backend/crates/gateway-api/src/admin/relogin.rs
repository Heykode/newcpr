//! Admin-authenticated relogin library; sensitive reads are explicit and no-store.

use super::wire::map_admin_service_error;
use super::{AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminResponse, AdminSessionState};
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use gateway_admin::model::relogin::{
    ReloginPushSelection, ReloginSettingsUpdate, ReloginTarget, ReloginWorkspaceMode,
};
use gateway_admin::model::relogin_templates::{ReloginTemplateConfig, ReloginTemplateSelection};
use gateway_core::account::{CredentialSelection, CredentialTarget};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HistoryRequest {
    id: String,
    before: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TotpRequest {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportRequest {
    text: String,
    #[serde(default)]
    replace_existing: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnrollRequest {
    text: String,
    #[serde(default)]
    replace_existing: bool,
    #[serde(default)]
    credential_target: CredentialTarget,
    settings: super::accounts::AccountImportSettingsRequest,
    outbound_proxy_id: Option<String>,
    quality_template: Option<gateway_admin::model::quality_ops::QualityTemplateSelection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportRequest {
    ids: Vec<String>,
    format: gateway_admin::model::relogin::ReloginExportFormat,
    confirm: String,
    #[serde(default)]
    credential_target: CredentialSelection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BatchRequest {
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueueRequest {
    ids: Vec<String>,
    #[serde(default)]
    workspace_mode: ReloginWorkspaceMode,
    #[serde(default)]
    credential_target: CredentialSelection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AccountQueueRequest {
    entry_id: String,
    revision: u64,
    target: ReloginTarget,
    additional_target: Option<ReloginTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PushRequest {
    #[serde(default)]
    credential_target: CredentialSelection,
    new_account_excel: Option<gateway_admin::model::relogin_templates::ExcelImportSettings>,
    custom_name: Option<String>,
    ids: Vec<String>,
    revisions: std::collections::BTreeMap<String, u64>,
    template: Option<ReloginTemplateSelection>,
    #[serde(default)]
    selections: std::collections::BTreeMap<String, ReloginPushSelection>,
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
    #[serde(default)]
    credential_target: CredentialTarget,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceRequest {
    id: String,
    workspace_id: Option<String>,
    #[serde(default)]
    credential_target: CredentialTarget,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceResumeRequest {
    id: String,
    revision: u64,
    workspace_id: String,
    #[serde(default)]
    credential_target: CredentialTarget,
}

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/relogin", get(list::<S>))
        .route("/api/admin/relogin/history", post(history::<S>))
        .route("/api/admin/relogin/totp", post(totp::<S>))
        .route("/api/admin/relogin/import", post(import::<S>))
        .route("/api/admin/relogin/enroll", post(enroll::<S>))
        .route("/api/admin/relogin/export", post(export::<S>))
        .route("/api/admin/relogin/queue", post(queue::<S>))
        .route("/api/admin/relogin/push", post(push::<S>))
        .route("/api/admin/relogin/delete", post(delete::<S>))
        .route("/api/admin/relogin/automatic", post(automatic::<S>))
        .route("/api/admin/relogin/workspace", post(workspace::<S>))
        .route(
            "/api/admin/relogin/workspace/resume",
            post(resume_workspace::<S>),
        )
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

async fn history<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<HistoryRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .relogin()
        .history(&request.id, request.before.as_deref())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(result),
    ))
}

async fn totp<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<TotpRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let result = state
        .admin_services()
        .relogin()
        .totp(&request.id, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    let mut response =
        AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(result)).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

async fn enroll<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<EnrollRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let settings = request
        .settings
        .into_settings()
        .map_err(|_| AdminError::invalid_request(StatusCode::BAD_REQUEST, "账号导入设置不合法"))?;
    let enrollment = gateway_admin::model::relogin::ReloginEnrollment::new(
        settings,
        request.outbound_proxy_id,
        auth.context().mutation_context(),
    )
    .map_err(map_admin_service_error)?
    .with_quality_template(request.quality_template);
    let selected = (request.credential_target != CredentialTarget::Codex)
        .then(|| {
            state
                .admin_services()
                .relogin()
                .credential_channel(request.credential_target)
        })
        .transpose()
        .map_err(map_admin_service_error)?;
    let service = selected
        .as_deref()
        .unwrap_or(state.admin_services().relogin());
    let ids = service
        .enroll(&request.text, request.replace_existing, enrollment)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::ACCEPTED,
        AdminEnvelope::ok(serde_json::json!({"ids": ids})),
    ))
}

async fn export<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<ExportRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    if request.confirm != "export_sensitive_relogin" {
        return Err(AdminError::invalid_request(
            StatusCode::BAD_REQUEST,
            "请先确认导出敏感资料",
        ));
    }
    let mut data = gateway_admin::model::relogin::ReloginExport {
        files: Vec::new(),
        skipped: Vec::new(),
    };
    let targets = if request.format == gateway_admin::model::relogin::ReloginExportFormat::TwoFa {
        CredentialSelection::Codex.targets()
    } else {
        request.credential_target.targets()
    };
    for &target in targets {
        let selected = (target != CredentialTarget::Codex)
            .then(|| state.admin_services().relogin().credential_channel(target))
            .transpose()
            .map_err(map_admin_service_error)?;
        let service = selected
            .as_deref()
            .unwrap_or(state.admin_services().relogin());
        match service
            .export(
                &request.ids,
                request.format,
                &auth.context().mutation_context(),
            )
            .await
        {
            Ok(exported) => {
                data.files.extend(exported.files);
                data.skipped.extend(exported.skipped);
            }
            Err(error) if !data.files.is_empty() => {
                data.skipped.extend(request.ids.iter().map(|id| {
                    gateway_admin::model::relogin::ReloginExportSkipped {
                        id: id.clone(),
                        credential_target: target,
                        message: error.message().to_owned(),
                    }
                }));
            }
            Err(error) => return Err(map_admin_service_error(error)),
        }
    }
    let mut response = AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
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
    let mut targets = vec![request.target];
    targets.extend(request.additional_target);
    state
        .admin_services()
        .relogin()
        .queue_account_channels(
            &request.entry_id,
            request.revision,
            &targets,
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
    AdminJson(request): AdminJson<QueueRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let mut result = Vec::new();
    for &target in request.credential_target.targets() {
        let selected = (target != CredentialTarget::Codex)
            .then(|| state.admin_services().relogin().credential_channel(target))
            .transpose()
            .map_err(map_admin_service_error)?;
        let service = selected
            .as_deref()
            .unwrap_or(state.admin_services().relogin());
        let mut items = match service
            .queue_with_workspace(&request.ids, request.workspace_mode)
            .await
        {
            Ok(items) => items,
            Err(error) if !result.is_empty() => request
                .ids
                .iter()
                .map(|id| gateway_admin::ReloginBatchResult {
                    id: id.clone(),
                    credential_target: target,
                    success: false,
                    message: error.message().to_owned(),
                })
                .collect(),
            Err(error) => return Err(map_admin_service_error(error)),
        };
        for item in &mut items {
            item.credential_target = target;
        }
        result.extend(items);
    }
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
        .push_channels(
            &request.ids,
            &request.revisions,
            gateway_admin::model::relogin_templates::ReloginNewAccountOptions {
                template: request.template,
                custom_name: request.custom_name,
                excel: request.new_account_excel,
            },
            &request.selections,
            request.credential_target,
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
    let selected = (request.credential_target != CredentialTarget::Codex)
        .then(|| {
            state
                .admin_services()
                .relogin()
                .credential_channel(request.credential_target)
        })
        .transpose()
        .map_err(map_admin_service_error)?;
    selected
        .as_deref()
        .unwrap_or(state.admin_services().relogin())
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
    let selected = (request.credential_target != CredentialTarget::Codex)
        .then(|| {
            state
                .admin_services()
                .relogin()
                .credential_channel(request.credential_target)
        })
        .transpose()
        .map_err(map_admin_service_error)?;
    selected
        .as_deref()
        .unwrap_or(state.admin_services().relogin())
        .workspace(&request.id, request.workspace_id)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}

async fn resume_workspace<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<WorkspaceResumeRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let selected = (request.credential_target != CredentialTarget::Codex)
        .then(|| {
            state
                .admin_services()
                .relogin()
                .credential_channel(request.credential_target)
        })
        .transpose()
        .map_err(map_admin_service_error)?;
    selected
        .as_deref()
        .unwrap_or(state.admin_services().relogin())
        .resume_workspace(&request.id, request.revision, &request.workspace_id)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}

async fn settings<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<ReloginSettingsUpdate>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    state
        .admin_services()
        .relogin()
        .configure_update(request)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(())))
}
