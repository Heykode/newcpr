//! 管理端 HTTP adapter、wire contract 与固定路由。

use axum::{
    Router,
    http::{HeaderValue, header},
    middleware,
    response::Response,
    routing::any,
};

pub mod account_groups;
pub mod accounts;
pub mod auth;
pub mod backups;
pub mod client_keys;
pub mod egress;
mod extract;
pub mod group_monitor;
pub mod log_cleanup;
pub mod mihomo;
pub mod notifications;
pub mod observability;
pub mod outbound_user_agent;
pub mod presenter;
pub mod proxies;
pub mod quality_ops;
pub mod relogin;
pub mod request_capture;
pub mod reset_credits;
pub mod settings;
pub mod system;
pub mod wire;

pub use auth::{AdminAuth, AdminSessionState};
pub use extract::{AdminJson, AdminQuery};
pub use wire::{
    ADMIN_OK_CODE, ADMIN_OK_MESSAGE, AdminEnvelope, AdminError, AdminErrorBody, AdminErrorCode,
    AdminPageData, AdminResponse, PageMeta, WireValidationError,
};

/// 构造完整且固定的 `/api/admin` 路由。
pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .merge(account_groups::router::<S>())
        .merge(group_monitor::router::<S>())
        .merge(log_cleanup::router::<S>())
        .merge(reset_credits::router::<S>())
        .merge(proxies::router::<S>())
        .merge(mihomo::router::<S>())
        .merge(relogin::router::<S>())
        .merge(request_capture::router::<S>())
        .merge(quality_ops::router::<S>())
        .merge(egress::router::<S>())
        .merge(accounts::router::<S>())
        .merge(auth::router::<S>())
        .merge(backups::router::<S>())
        .merge(client_keys::router::<S>())
        .merge(observability::router::<S>())
        .merge(notifications::router::<S>())
        .merge(settings::router::<S>())
        .merge(outbound_user_agent::router::<S>())
        .merge(system::router::<S>())
        .method_not_allowed_fallback(method_not_allowed)
        .route("/api/admin", any(admin_not_found))
        .route("/api/admin/{*path}", any(admin_not_found))
        .layer(middleware::map_response(no_store))
        .layer(middleware::from_fn(record_failure))
}

async fn record_failure(
    diagnostics: Option<
        axum::Extension<std::sync::Arc<dyn gateway_core::diagnostics::OperationalDiagnostics>>,
    >,
    request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let correlation_id = request
        .extensions()
        .get::<tower_http::request_id::RequestId>()
        .and_then(|id| id.header_value().to_str().ok())
        .map(str::to_owned);
    let mut response = next.run(request).await;
    let details = response
        .extensions_mut()
        .remove::<gateway_core::error::ErrorDetails>();
    if response.status().is_server_error()
        && let Some(axum::Extension(diagnostics)) = diagnostics
    {
        let mut failure = gateway_core::diagnostics::OperationalFailure::new(
            "admin_api",
            "admin_request",
            "admin_request_failed",
            "Administrator request failed",
        );
        failure.correlation_id = correlation_id;
        failure.details = details;
        if diagnostics.record_failure(failure).await.is_err() {
            tracing::warn!("Administrator failure diagnostic was not recorded");
        }
    }
    response
}

async fn method_not_allowed() -> AdminError {
    AdminError::method_not_allowed()
}

async fn admin_not_found() -> AdminError {
    AdminError::admin_route_not_found()
}

async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    response
}
