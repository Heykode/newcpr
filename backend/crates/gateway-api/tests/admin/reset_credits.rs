use super::{AdminTestFixture, AdminTestState};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt as _;

#[tokio::test]
async fn reset_credit_batch_routes_require_admin_authentication() {
    let fixture = AdminTestFixture::new().await;
    let app = gateway_api::admin::router::<AdminTestState>().with_state(fixture.state());
    for (method, suffix) in [
        ("POST", "cache"),
        ("POST", "refresh"),
        ("POST", "preview"),
        ("GET", "batches"),
        ("POST", "confirm"),
        ("POST", "retry"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/admin/accounts/reset-credits/{suffix}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-request-id", "reset-fixture")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
