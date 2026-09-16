//! Explicit outbound UA selection, separate from full runtime-settings replacement.

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use gateway_admin::model::user_agent::{OutboundUserAgentView, ProviderUserAgentOverride};
use gateway_core::routing::ProviderKind;
use serde::{Deserialize, Serialize};

use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminResponse, AdminSessionState,
    wire::map_admin_service_error,
};

#[derive(Debug, Clone)]
pub enum UpdateOutboundUserAgentRequest {
    Default,
    Custom { user_agent: String },
}

impl<'de> Deserialize<'de> for UpdateOutboundUserAgentRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(
            tag = "mode",
            rename_all = "kebab-case",
            rename_all_fields = "camelCase",
            deny_unknown_fields
        )]
        enum Wire {
            Default { user_agent: Option<String> },
            Custom { user_agent: String },
        }

        match Wire::deserialize(deserializer)? {
            Wire::Default { user_agent: None } => Ok(Self::Default),
            Wire::Custom { user_agent } => Ok(Self::Custom { user_agent }),
            Wire::Default {
                user_agent: Some(_),
            } => Err(serde::de::Error::custom(
                "userAgent is not allowed when mode is default",
            )),
        }
    }
}

impl From<UpdateOutboundUserAgentRequest> for ProviderUserAgentOverride {
    fn from(request: UpdateOutboundUserAgentRequest) -> Self {
        match request {
            UpdateOutboundUserAgentRequest::Default => Self::Default,
            UpdateOutboundUserAgentRequest::Custom { user_agent } => Self::Custom { user_agent },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundUserAgentSettingsView {
    mode: &'static str,
    custom_user_agent: Option<String>,
    default_user_agent: String,
    effective_user_agent: String,
    effective_desktop_user_agent: String,
    core_version: String,
    desktop_version: String,
    os_type: String,
    os_version: String,
    arch: String,
    terminal: String,
    verified: bool,
    default_verified_at: DateTime<Utc>,
}

impl From<OutboundUserAgentView> for OutboundUserAgentSettingsView {
    fn from(view: OutboundUserAgentView) -> Self {
        let (mode, custom_user_agent) = match view.selection {
            ProviderUserAgentOverride::Default => ("default", None),
            ProviderUserAgentOverride::Custom { user_agent } => ("custom", Some(user_agent)),
        };
        Self {
            mode,
            custom_user_agent,
            default_user_agent: view.default_user_agent,
            effective_user_agent: view.effective_user_agent,
            effective_desktop_user_agent: view.effective_desktop_user_agent,
            core_version: view.core_version,
            desktop_version: view.desktop_version,
            os_type: view.os_type,
            os_version: view.os_version,
            arch: view.arch,
            terminal: view.terminal,
            verified: view.verified,
            default_verified_at: view.default_verified_at,
        }
    }
}

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/admin/settings/openai-user-agent",
            get(load::<S>).post(save::<S>),
        )
        .route(
            "/api/admin/settings/openai-user-agent/preview",
            post(preview::<S>),
        )
}

fn provider_kind() -> Result<ProviderKind, AdminError> {
    ProviderKind::new("openai").map_err(|_| AdminError::bad_request("Provider 类型不合法"))
}

async fn load<S>(_auth: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let view = state
        .admin_services()
        .outbound_user_agent()
        .load(&provider_kind()?)
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(OutboundUserAgentSettingsView::from(view)),
    ))
}

async fn preview<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<UpdateOutboundUserAgentRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let view = state
        .admin_services()
        .outbound_user_agent()
        .preview(&provider_kind()?, &request.into())
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(OutboundUserAgentSettingsView::from(view)),
    ))
}

async fn save<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<UpdateOutboundUserAgentRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let view = state
        .admin_services()
        .outbound_user_agent()
        .replace(
            &auth.context().mutation_context(),
            &provider_kind()?,
            request.into(),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(OutboundUserAgentSettingsView::from(view)),
    ))
}
