use super::{AdminTestFixture, AdminTestState};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt as _;

#[tokio::test]
async fn quality_routes_require_auth_and_disable_caching() {
    let fixture = AdminTestFixture::new().await;
    let app = gateway_api::admin::router::<AdminTestState>().with_state(fixture.state());
    for (method, path) in [
        ("GET", "rules"),
        ("GET", "runs"),
        ("GET", "detail"),
        ("POST", "save"),
        ("POST", "delete"),
        ("POST", "run"),
        ("GET", "templates"),
        ("POST", "templates/save"),
        ("POST", "templates/delete"),
        ("POST", "templates/apply"),
        ("POST", "monitoring"),
        ("POST", "models"),
        ("GET", "groups"),
        ("POST", "groups/save"),
        ("POST", "groups/delete"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/admin/quality-ops/{path}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-request-id", "quality-fixture")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[tokio::test]
async fn quality_without_store_is_unavailable_not_a_fake_success() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("quality-session");
    let app = gateway_api::admin::router::<AdminTestState>().with_state(fixture.state());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/quality-ops/rules")
                .header(header::COOKIE, "cpr_admin_session=quality-session")
                .header("x-request-id", "quality-fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn quality_models_reject_invalid_scope_and_unknown_fields() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("quality-session");
    let app = gateway_api::admin::router::<AdminTestState>().with_state(fixture.state());
    for body in [
        serde_json::json!({"page": 0}),
        serde_json::json!({"page": 1000001}),
        serde_json::json!({"page": 1, "accountIds": ["acct_fixture"], "group": "fixture-group"}),
        serde_json::json!({"page": 1, "accountIds": ["acct_fixture"], "statuses": ["normal"]}),
        serde_json::json!({"page": 1, "statuses": ["invented-status"]}),
        serde_json::json!({"page": 1, "forceRefresh": true}),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/quality-ops/models")
                    .header(header::COOKIE, "cpr_admin_session=quality-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-request-id", "quality-fixture")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let expected = if body.get("forceRefresh").is_some() {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::BAD_REQUEST
        };
        assert_eq!(response.status(), expected, "{body}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
