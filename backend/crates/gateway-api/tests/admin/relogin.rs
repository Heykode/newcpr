use super::{AdminTestFixture, AdminTestState};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt as _;

async fn request(
    fixture: &AdminTestFixture,
    path: &str,
    body: Option<Value>,
    authenticated: bool,
) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .uri(path)
        .method(if body.is_some() { "POST" } else { "GET" })
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-request-id", "relogin-api-test");
    if authenticated {
        builder = builder.header(header::COOKIE, "cpr_admin_session=valid-session");
    }
    let response = gateway_api::admin::relogin::router::<AdminTestState>()
        .with_state(fixture.state())
        .oneshot(
            builder
                .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn relogin_every_endpoint_requires_admin_auth_before_reading_material() {
    let fixture = AdminTestFixture::new().await;
    for (path, body) in [
        ("/api/admin/relogin", None),
        ("/api/admin/relogin/templates", None),
        ("/api/admin/relogin/templates/save", Some(json!({}))),
        ("/api/admin/relogin/templates/delete", Some(json!({}))),
        (
            "/api/admin/relogin/import",
            Some(json!({"text":"test-only-private-material"})),
        ),
        ("/api/admin/relogin/queue", Some(json!({"ids":["a"]}))),
        (
            "/api/admin/relogin/accounts/query",
            Some(json!({"ids":["a"]})),
        ),
        ("/api/admin/relogin/accounts/queue", Some(json!({}))),
        (
            "/api/admin/relogin/push",
            Some(json!({"ids":["a"],"revisions":{"a":1}})),
        ),
        ("/api/admin/relogin/delete", Some(json!({"ids":["a"]}))),
        (
            "/api/admin/relogin/automatic",
            Some(json!({"ids":["a"],"enabled":false})),
        ),
        (
            "/api/admin/relogin/workspace",
            Some(json!({"id":"a","workspaceId":null})),
        ),
        (
            "/api/admin/relogin/settings",
            Some(json!({"concurrency":1,"paused":false})),
        ),
    ] {
        let (status, body) = request(&fixture, path, body, false).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert!(!body.contains("test-only-private-material"));
    }
}

#[tokio::test]
async fn relogin_rejects_missing_confirmation_versions_and_unknown_fields() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (path, body) in [
        ("/api/admin/relogin/push", json!({"ids":["a"]})),
        (
            "/api/admin/relogin/push",
            json!({"ids":["a"],"revisions":{"a":1},"template":{"id":"t"}}),
        ),
        ("/api/admin/relogin/templates/delete", json!({"id":"t"})),
        (
            "/api/admin/relogin/templates/save",
            json!({"config":{
                "name":"example","enabled":true,"weight":1,"concurrencyLimit":0.5,
                "groupIds":[],"outboundProxyId":null
            }}),
        ),
        (
            "/api/admin/relogin/templates/save",
            json!({"config":{
                "name":"example","enabled":true,"weight":1,"concurrencyLimit":null,
                "groupIds":[],"outboundProxyId":null,"password":"test-only-private-material"
            }}),
        ),
        (
            "/api/admin/relogin/templates/save",
            json!({"config":{
                "name":"example","enabled":true,"weight":1,"concurrencyLimit":null,
                "groupIds":[],"outboundProxyId":null,"customName":"Never apply"
            }}),
        ),
        (
            "/api/admin/relogin/push",
            json!({"ids":["a"],"revisions":{"a":1},"customName":42}),
        ),
        (
            "/api/admin/relogin/accounts/query",
            json!({"ids":["a"],"surprise":true}),
        ),
        (
            "/api/admin/relogin/accounts/queue",
            json!({"entryId":"a","revision":1}),
        ),
        (
            "/api/admin/relogin/accounts/queue",
            json!({
                "entryId":"a","revision":1,
                "target":{"account_id":"a","credential_revision":1,"user_id":"u","workspace_id":"w","surprise":true}
            }),
        ),
        (
            "/api/admin/relogin/settings",
            json!({"concurrency":1,"paused":false,"surprise":true}),
        ),
        (
            "/api/admin/relogin/import",
            json!({"text":"test-only-private-material","surprise":true}),
        ),
    ] {
        let (status, body) = request(&fixture, path, Some(body), true).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!body.contains("test-only-private-material"));
    }
}

#[tokio::test]
async fn relogin_push_accepts_optional_camel_case_name_and_validates_before_store_access() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for name in [json!(null), json!("Batch A"), json!("")] {
        let (status, _) = request(
            &fixture,
            "/api/admin/relogin/push",
            Some(json!({"ids":["a"],"revisions":{"a":1},"customName":name})),
            true,
        )
        .await;
        // This fixture has no relogin store; a parsed, valid request reaches it.
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
    for name in ["x".repeat(129), "bad\nname".to_owned()] {
        let (status, _) = request(
            &fixture,
            "/api/admin/relogin/push",
            Some(json!({"ids":["a"],"revisions":{"a":1},"customName":name})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
