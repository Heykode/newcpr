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
        ("GET", "history?page=1"),
        ("POST", "confirm"),
        ("POST", "retry"),
        ("GET", "automatic"),
        ("POST", "automatic"),
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

#[tokio::test]
async fn automatic_policy_rejects_invalid_thresholds_and_payloads_before_store_access() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("reset-session");
    let app = gateway_api::admin::router::<AdminTestState>().with_state(fixture.state());
    for invalid in [
        serde_json::json!(-1),
        serde_json::json!(1),
        serde_json::json!(99),
        serde_json::json!(100001),
        serde_json::json!(1.5),
        serde_json::json!("100000"),
        serde_json::Value::Null,
    ] {
        let response = app.clone().oneshot(Request::builder()
            .method("POST")
            .uri("/api/admin/accounts/reset-credits/automatic")
            .header(header::COOKIE, "cpr_admin_session=reset-session")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-request-id", "reset-fixture")
            .body(Body::from(serde_json::json!({
                "accountId": "acct_fixture", "revision": 0,
                "config": {"enabled": true, "fiveHourUsedMillis": invalid, "sevenDayUsedMillis": 100000}
            }).to_string())).unwrap()).await.unwrap();
        let expected = if invalid.as_u64().is_some() {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        };
        assert_eq!(response.status(), expected, "{invalid}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    for threshold in [0, 100, 75500, 100000] {
        let response = app.clone().oneshot(Request::builder()
            .method("POST")
            .uri("/api/admin/accounts/reset-credits/automatic")
            .header(header::COOKIE, "cpr_admin_session=reset-session")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-request-id", "reset-fixture")
            .body(Body::from(serde_json::json!({
                "accountId": "acct_fixture", "revision": 0,
                "config": {"enabled": true, "fiveHourUsedMillis": threshold, "sevenDayUsedMillis": 0}
            }).to_string())).unwrap()).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "valid payload reaches missing store: {threshold}"
        );
    }
}
