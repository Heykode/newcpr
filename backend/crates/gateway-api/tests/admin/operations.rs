use super::{AdminTestFixture, AdminTestState};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt as _;

#[tokio::test]
async fn capture_and_guard_routes_require_admin_auth_before_reading_or_mutating() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("operations-session");
    let app = gateway_api::admin::router::<AdminTestState>().with_state(fixture.state());
    for (method, path, body) in [
        ("GET", "/api/admin/token-guard", ""),
        ("POST", "/api/admin/token-guard/run", ""),
        ("POST", "/api/admin/token-guard/config", "{}"),
        (
            "POST",
            "/api/admin/token-guard/relogin",
            r#"{"accountId":"fixture"}"#,
        ),
        ("GET", "/api/admin/request-captures", ""),
        ("POST", "/api/admin/request-captures", "{}"),
        ("POST", "/api/admin/request-captures/config", "{}"),
        (
            "POST",
            "/api/admin/request-captures/00000000-0000-0000-0000-000000000001/stop",
            "",
        ),
        (
            "DELETE",
            "/api/admin/request-captures/00000000-0000-0000-0000-000000000001",
            "",
        ),
        (
            "GET",
            "/api/admin/request-captures/records/00000000-0000-0000-0000-000000000001",
            "",
        ),
        (
            "GET",
            "/api/admin/request-captures/records/00000000-0000-0000-0000-000000000001/export",
            "",
        ),
        (
            "GET",
            "/api/admin/request-captures/00000000-0000-0000-0000-000000000001/export",
            "",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-request-id", "operations-fixture")
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
    for path in ["/api/admin/token-guard", "/api/admin/request-captures"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::COOKIE, "cpr_admin_session=operations-session")
                    .header("x-request-id", "operations-fixture")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
