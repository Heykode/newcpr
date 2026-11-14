//! Explicit Excel credential endpoints; legacy credential endpoints retain their Codex contract.

use super::*;

pub(super) fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/accounts/excel/import", post(import::<S>))
        .route("/api/admin/accounts/excel/oauth/start", post(start::<S>))
        .route(
            "/api/admin/accounts/excel/oauth/complete",
            post(complete::<S>),
        )
        .route("/api/admin/accounts/excel/refresh", post(refresh::<S>))
        .route("/api/admin/accounts/excel/models", get(models::<S>))
        .route("/api/admin/accounts/excel/export", get(export::<S>))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExcelImportRequest {
    data: Value,
    account_id: Option<String>,
    expected_revision: Option<u64>,
    outbound_proxy_id: Option<String>,
    settings: Option<AccountImportSettingsRequest>,
}

async fn import<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<ExcelImportRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let target = match request.account_id {
        Some(id) => {
            require_account_id(&id, "accountId").map_err(map_wire_error)?;
            Some(
                gateway_admin::model::accounts::ExcelCredentialImportTarget {
                    account_id: id,
                    expected_revision: request
                        .expected_revision
                        .map(gateway_core::account::CredentialRevision::new)
                        .transpose()
                        .map_err(|_| AdminError::bad_request("Excel 凭据版本不合法"))?,
                },
            )
        }
        None if request.expected_revision.is_some() => {
            return Err(AdminError::bad_request("Excel 凭据版本需要指定账号"));
        }
        None => None,
    };
    if !request.data.is_object()
        || serde_json::to_vec(&request.data).map_or(true, |value| value.len() > 256 * 1024)
    {
        return Err(AdminError::bad_request("Excel 凭据格式或大小不合法"));
    }
    let rows = state
        .admin_services()
        .openai()
        .import_excel_document(
            ImportCredentials {
                outbound_proxy_id: request.outbound_proxy_id,
                settings: request
                    .settings
                    .map(AccountImportSettingsRequest::into_settings)
                    .transpose()
                    .map_err(map_wire_error)?,
                context: auth.context().mutation_context(),
                document: ProviderDocument::new(OpaqueProviderData::new(
                    request
                        .data
                        .as_object()
                        .cloned()
                        .ok_or_else(|| AdminError::bad_request("Excel 凭据格式不合法"))?,
                )),
            },
            target,
        )
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::CREATED,
        AdminEnvelope::ok(committed_ids(rows)),
    ))
}

async fn start<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<StartAccountAuthorizationRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let (provider, command) = request
        .into_command(auth.context().mutation_context())
        .map_err(map_wire_error)?;
    if provider != AccountProvider::OpenAi {
        return Err(AdminError::bad_request("Excel 仅支持 OpenAI 账号"));
    }
    let result = state
        .admin_services()
        .openai()
        .start_excel_authorization(command)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::CREATED,
        AdminEnvelope::ok(AccountAuthorizationData::from(result)),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExcelCompleteRequest {
    flow_id: String,
    callback_url: String,
    settings: Option<AccountImportSettingsRequest>,
}

async fn complete<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<ExcelCompleteRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    if Uuid::parse_str(&request.flow_id).is_err() || request.callback_url.len() > 16 * 1024 {
        return Err(AdminError::bad_request("Excel 授权回调不合法"));
    }
    let rows = state
        .admin_services()
        .openai()
        .complete_excel_authorization(CompleteAuthorization {
            context: auth.context().mutation_context(),
            flow_id: request.flow_id,
            callback_url: request.callback_url,
            settings: request
                .settings
                .map(AccountImportSettingsRequest::into_settings)
                .transpose()
                .map_err(map_wire_error)?,
        })
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::CREATED,
        AdminEnvelope::ok(committed_ids(rows)),
    ))
}

async fn refresh<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<AccountRefreshRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let id = request.into_command().map_err(map_wire_error)?;
    state
        .admin_services()
        .openai()
        .refresh_excel_credential(&id)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(serde_json::json!({"accountId":id.as_str(),"credentialTarget":"excel"})),
    ))
}

async fn models<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<AccountIdQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let id = query.into_id().map_err(map_wire_error)?;
    let result = state
        .admin_services()
        .openai()
        .excel_models(&id)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(account_models_data(result, state.deployment_timezone())),
    ))
}

async fn export<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<AccountIdQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let id = query.into_id().map_err(map_wire_error)?;
    let document = state
        .admin_services()
        .openai()
        .export_excel_credential(&id)
        .await
        .map_err(map_service_error)?;
    let mut response=AdminResponse::new(StatusCode::OK,AdminEnvelope::ok(serde_json::json!({
        "provider":"openai","credentialTarget":"excel","document":document.into_provider_data().into_inner()
    }))).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn committed_ids(rows: Vec<gateway_admin::model::accounts::AccountRecord>) -> Value {
    serde_json::json!({"accountIds":rows.into_iter().map(|row| row.id).collect::<Vec<_>>(),"credentialTarget":"excel"})
}
