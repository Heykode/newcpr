use super::{AdminTestFixture, AdminTestState};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt as _;

#[tokio::test]
async fn log_cleanup_requires_admin_auth_for_every_route() {
    let fixture = AdminTestFixture::new().await;
    let app = gateway_api::admin::router::<AdminTestState>().with_state(fixture.state());
    for (method, path) in [
        ("GET", "/api/admin/log-cleanup"),
        ("GET", "/api/admin/log-cleanup/usage"),
        ("POST", "/api/admin/log-cleanup"),
        ("POST", "/api/admin/log-cleanup/preview"),
        ("POST", "/api/admin/log-cleanup/start"),
        ("POST", "/api/admin/log-cleanup/captures/start"),
        ("POST", "/api/admin/log-cleanup/cancel"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-request-id", "cleanup-fixture")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
