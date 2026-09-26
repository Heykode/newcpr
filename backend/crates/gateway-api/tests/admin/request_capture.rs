use super::{AdminTestFixture, AdminTestState};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt as _;

#[tokio::test]
async fn capture_routes_require_admin_auth_before_reading_or_mutating() {
    let fixture = AdminTestFixture::new().await;
    let app = gateway_api::admin::router::<AdminTestState>().with_state(fixture.state());
    for (method, path, body) in [
        ("GET", "/api/admin/request-captures", ""),
        ("POST", "/api/admin/request-captures", "{}"),
        ("POST", "/api/admin/request-captures/config", "{}"),
        ("POST", "/api/admin/request-captures/stop", "{}"),
        ("POST", "/api/admin/request-captures/delete", "{}"),
        ("GET", "/api/admin/request-captures/records", ""),
        ("GET", "/api/admin/request-captures/records/export", ""),
        ("GET", "/api/admin/request-captures/export", ""),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-request-id", "capture-fixture")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[tokio::test]
async fn capture_without_store_fails_closed_for_authenticated_admin() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("capture-session");
    let app = gateway_api::admin::router::<AdminTestState>().with_state(fixture.state());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/request-captures")
                .header(header::COOKIE, "cpr_admin_session=capture-session")
                .header("x-request-id", "capture-fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}
