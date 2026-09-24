use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use gateway_api::admin::notifications;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{AdminTestFixture, AdminTestState, PRIMARY_GROUP_ID};

fn app(state: AdminTestState) -> Router {
    notifications::router::<AdminTestState>().with_state(state)
}

fn request(method: Method, path: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, "cpr_admin_session=valid-session")
        .header("x-request-id", "req_notification_test");
    match body {
        Some(value) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn json_body(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn notification_reads_are_authenticated_and_never_expose_secret_fields() {
    let fixture = AdminTestFixture::new().await;
    let state = fixture.state();
    let app = app(state);
    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/admin/notifications/channels")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    fixture.auth.insert_session("valid-session");
    let response = app
        .oneshot(request(
            Method::GET,
            "/api/admin/notifications/channels",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value = json_body(response).await;
    let text = value.to_string();
    assert!(!text.contains("password\""));
    assert!(!text.contains("deviceKey\""));
    assert_eq!(value["data"]["smtp"]["passwordSet"], false);
    assert_eq!(value["data"]["bark"]["deviceKeySet"], false);
}

#[tokio::test]
async fn group_policy_defaults_are_strict_and_unknown_fields_are_rejected() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let app = app(fixture.state());
    let response = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/admin/account-groups/alert-policy?groupId={PRIMARY_GROUP_ID}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value = json_body(response).await;
    assert_eq!(value["data"]["enabled"], false);
    assert_eq!(value["data"]["concurrency"]["threshold"], 90.0);
    assert_eq!(value["data"]["eta"]["threshold"], 10.0);

    let rejected = app.oneshot(request(Method::POST, "/api/admin/notifications/channels/update", Some(json!({
        "smtp": { "enabled": false, "host": "", "port": 587, "security": "starttls", "username": null, "password": null, "passwordSet": false, "fromName": null, "fromEmail": null },
        "bark": { "enabled": false, "serverUrl": "https://push.example.com", "deviceKey": null, "deviceKeySet": false, "level": "active", "sound": null, "volume": 5, "call": false },
        "unexpected": true
    })))).await.unwrap();
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn secret_presence_flags_cannot_enable_unconfigured_channels() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let app = app(fixture.state());
    for channel in ["smtp", "bark"] {
        let response = app.clone().oneshot(request(Method::POST, "/api/admin/notifications/channels/update", Some(json!({
            "smtp": { "enabled": channel == "smtp", "host": "smtp.example.com", "port": 587, "security": "starttls", "username": "synthetic-user", "password": null, "passwordSet": true, "fromName": null, "fromEmail": "alerts@example.com" },
            "bark": { "enabled": channel == "bark", "serverUrl": "https://push.example.com", "deviceKey": null, "deviceKeySet": true, "level": "active", "sound": null, "volume": 5, "call": false }
        })))).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let response = app
        .oneshot(request(
            Method::GET,
            "/api/admin/notifications/deliveries",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["data"], json!([]));
}

#[tokio::test]
async fn bark_full_links_and_path_fragments_are_rejected_without_echoing_secrets() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let app = app(fixture.state());
    for key in [
        "https://push.example.com/synthetic-private-value",
        "synthetic-private-value/path",
    ] {
        let response = app.clone().oneshot(request(Method::POST, "/api/admin/notifications/channels/update", Some(json!({
            "smtp": { "enabled": false, "host": "", "port": 465, "security": "tls", "username": null, "password": null, "passwordSet": false, "fromName": null, "fromEmail": null },
            "bark": { "enabled": true, "serverUrl": "https://push.example.com", "deviceKey": key, "deviceKeySet": false, "level": "active", "sound": null, "volume": 5, "call": false }
        })))).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let value = json_body(response).await.to_string();
        assert!(value.contains("Device Key"));
        assert!(!value.contains("synthetic-private-value"));
    }
}
