use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use gateway_api::admin;
use serde_json::{Value, json};
use tower::ServiceExt as _;

use super::super::{AdminTestFixture, AdminTestState};

async fn send(
    fixture: &AdminTestFixture,
    method: &str,
    uri: &str,
    body: Value,
    authenticated: bool,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-request-id", "import-task-contract")
        .header(header::CONTENT_TYPE, "application/json");
    if authenticated {
        request = request.header(header::COOKIE, "cpr_admin_session=valid-session");
    }
    let response = admin::router::<AdminTestState>()
        .with_state(fixture.state())
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(!value.to_string().contains("synthetic-sensitive-token"));
    (status, value)
}

fn input() -> Value {
    json!({
        "submissionId": "01994fcb-a333-7333-8000-000000000001",
        "items": [{
            "provider": "openai",
            "data": {"refreshToken": "synthetic-sensitive-token"}
        }]
    })
}

#[tokio::test]
async fn import_template_selection_is_strict_and_unavailable_catalog_never_queues() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (selection, status) in [
        (
            json!({"id": "template-a"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"id": "template-a", "revision": 1, "overwrite": true}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"id": "template-a", "revision": 0}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"id": "template-a", "revision": 1}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let mut request = input();
        request["items"][0]["template"] = selection;
        for (path, body) in [
            ("/api/admin/accounts/import", request["items"][0].clone()),
            ("/api/admin/accounts/import-tasks", request),
        ] {
            assert_eq!(
                send(&fixture, "POST", path, body, true).await.0,
                status,
                "{path}"
            );
        }
    }
    let (_, list) = send(
        &fixture,
        "GET",
        "/api/admin/accounts/import-tasks",
        Value::Null,
        true,
    )
    .await;
    assert_eq!(list["data"]["items"], json!([]));
}

#[test]
fn omitted_import_template_preserves_the_wire_fingerprint_and_null_means_omission() {
    use gateway_api::admin::accounts::AccountImportRequest;
    let body = input()["items"][0].clone();
    let original: AccountImportRequest = serde_json::from_value(body.clone()).unwrap();
    let mut with_null = body;
    with_null["template"] = Value::Null;
    let with_null: AccountImportRequest = serde_json::from_value(with_null).unwrap();
    let serialized = serde_json::to_value(&original).unwrap();
    assert!(serialized.get("template").is_none());
    assert!(serialized.get("templateSettingsOverride").is_none());
    assert_eq!(serialized, serde_json::to_value(&with_null).unwrap());
}

#[test]
fn prefilled_template_import_requires_both_selection_and_final_settings() {
    use gateway_api::admin::accounts::AccountImportRequest;
    let mut body = input()["items"][0].clone();
    body["templateSettingsOverride"] = json!(true);
    let parse = |body: Value| serde_json::from_value::<AccountImportRequest>(body).unwrap();
    assert!(parse(body.clone()).validate().is_err());
    body["template"] = json!({"id":"template-a","revision":1});
    assert!(parse(body.clone()).validate().is_err());
    body["settings"] = json!({"enabled":true,"weight":31,"concurrencyLimit":null,"groupIds":[]});
    assert!(parse(body.clone()).validate().is_ok());
    let snapshot = serde_json::to_value(parse(body.clone())).unwrap();
    assert_eq!(snapshot["templateSettingsOverride"], true);
    assert_eq!(snapshot["settings"]["weight"], 31);
    body["template"] = Value::Null;
    assert!(parse(body).validate().is_err());
}

#[tokio::test]
async fn prefilled_import_still_requires_an_available_template_catalog_before_queueing() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let mut request = input();
    request["items"][0]["templateSettingsOverride"] = json!(true);
    request["items"][0]["template"] = json!({"id":"template-a","revision":1});
    request["items"][0]["settings"] =
        json!({"enabled":true,"weight":31,"concurrencyLimit":null,"groupIds":[]});
    for (path, body) in [
        ("/api/admin/accounts/import", request["items"][0].clone()),
        ("/api/admin/accounts/import-tasks", request),
    ] {
        assert_eq!(
            send(&fixture, "POST", path, body, true).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    let (_, list) = send(
        &fixture,
        "GET",
        "/api/admin/accounts/import-tasks",
        Value::Null,
        true,
    )
    .await;
    assert_eq!(list["data"]["items"], json!([]));
}

#[tokio::test]
async fn accepts_lists_restores_stops_and_deduplicates_without_echoing_credentials() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let (status, accepted) = send(
        &fixture,
        "POST",
        "/api/admin/accounts/import-tasks",
        input(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let id = accepted["data"]["taskId"].as_str().unwrap();
    assert_eq!(accepted["data"]["total"], 1);
    assert_eq!(accepted["data"]["stopRequested"], false);
    assert!(accepted["data"]["createdAt"].is_string());
    assert!(accepted["data"]["finishedAt"].is_null());
    assert_eq!(
        accepted["data"]["counts"],
        json!({
            "pending": 1, "running": 0, "succeeded": 0, "failed": 0,
            "unknown": 0, "skipped": 0, "importedAccounts": 0
        })
    );
    let (status, retried) = send(
        &fixture,
        "POST",
        "/api/admin/accounts/import-tasks",
        input(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(retried["data"]["taskId"], id);
    let (status, list) = send(
        &fixture,
        "GET",
        "/api/admin/accounts/import-tasks",
        Value::Null,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["data"]["items"].as_array().unwrap().len(), 1);
    let (status, detail) = send(
        &fixture,
        "GET",
        &format!("/api/admin/accounts/import-tasks/detail?taskId={id}"),
        Value::Null,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["data"]["taskId"], id);
    assert_eq!(
        detail["data"]["items"][0],
        json!({"index": 1, "provider": "openai", "status": "pending", "accountIds": [], "accountEmails": {}, "message": null})
    );
    let (status, stopped) = send(
        &fixture,
        "POST",
        "/api/admin/accounts/import-tasks/stop",
        json!({"taskId": id}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stopped["data"]["counts"]["skipped"], 1);
    assert_eq!(stopped["data"]["items"][0]["status"], "skipped");
    assert!(stopped["data"]["finishedAt"].is_string());
    let mut changed = input();
    changed["items"][0]["data"]["refreshToken"] = json!("changed-synthetic-input");
    assert_eq!(
        send(
            &fixture,
            "POST",
            "/api/admin/accounts/import-tasks",
            changed,
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn requires_admin_on_all_routes_and_enforces_input_bounds() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (method, uri, body) in [
        ("POST", "/api/admin/accounts/import-tasks", input()),
        ("GET", "/api/admin/accounts/import-tasks", Value::Null),
        (
            "GET",
            "/api/admin/accounts/import-tasks/detail?taskId=01994fcb-a333-7333-8000-000000000001",
            Value::Null,
        ),
        (
            "POST",
            "/api/admin/accounts/import-tasks/stop",
            json!({"taskId": "01994fcb-a333-7333-8000-000000000001"}),
        ),
    ] {
        assert_eq!(
            send(&fixture, method, uri, body, false).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    for count in [0, 201] {
        let mut body = input();
        body["items"] = json!(vec![body["items"][0].clone(); count]);
        assert_eq!(
            send(
                &fixture,
                "POST",
                "/api/admin/accounts/import-tasks",
                body,
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut body = input();
    body["items"][0]["data"]["refreshToken"] = json!("x".repeat(4 * 1024 * 1024));
    assert_eq!(
        send(
            &fixture,
            "POST",
            "/api/admin/accounts/import-tasks",
            body,
            true
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut body = input();
    body["items"] = json!(vec![body["items"][0].clone(); 200]);
    assert_eq!(
        send(
            &fixture,
            "POST",
            "/api/admin/accounts/import-tasks",
            body,
            true
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
}

#[tokio::test]
async fn rejects_invalid_ids_unknown_fields_and_invalid_items_without_partial_submission() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for id in ["invalid", "00000000-0000-0000-0000-000000000000"] {
        let mut body = input();
        body["submissionId"] = json!(id);
        assert_eq!(
            send(
                &fixture,
                "POST",
                "/api/admin/accounts/import-tasks",
                body,
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    for (change, expected) in [
        (
            json!({"provider": "unsupported", "data": {}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"provider": "openai", "data": []}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"provider": "openai", "data": {}, "unknownField": true}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"provider": "openai", "data": {}, "settings": {"enabled": true}}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let mut body = input();
        body["items"].as_array_mut().unwrap().push(change);
        assert_eq!(
            send(
                &fixture,
                "POST",
                "/api/admin/accounts/import-tasks",
                body,
                true
            )
            .await
            .0,
            expected
        );
    }
    let mut body = input();
    body["unknownField"] = json!(true);
    assert_eq!(
        send(
            &fixture,
            "POST",
            "/api/admin/accounts/import-tasks",
            body,
            true
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    for (uri, status) in [
        (
            "/api/admin/accounts/import-tasks/detail?taskId=invalid",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/admin/accounts/import-tasks/detail?taskId=01994fcb-a333-7333-8000-000000000002",
            StatusCode::NOT_FOUND,
        ),
        (
            "/api/admin/accounts/import-tasks/detail?taskId=01994fcb-a333-7333-8000-000000000002&extra=1",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            send(&fixture, "GET", uri, Value::Null, true).await.0,
            status
        );
    }
    let (_, list) = send(
        &fixture,
        "GET",
        "/api/admin/accounts/import-tasks",
        Value::Null,
        true,
    )
    .await;
    assert_eq!(list["data"]["items"], json!([]));
}

#[tokio::test]
async fn fingerprints_include_provider_proxy_and_all_import_settings() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let mut body = input();
    body["items"][0]["settings"] = json!({
        "customName": "Import batch", "enabled": true, "turnStateInjectionEnabled": false,
        "concurrencyLimit": null, "weight": 100, "groupIds": []
    });
    assert_eq!(
        send(
            &fixture,
            "POST",
            "/api/admin/accounts/import-tasks",
            body.clone(),
            true
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    for (pointer, value) in [
        ("/items/0/provider", json!("xai")),
        ("/items/0/settings/customName", json!("Another batch")),
        ("/items/0/settings/enabled", json!(false)),
        ("/items/0/settings/turnStateInjectionEnabled", json!(true)),
        ("/items/0/settings/concurrencyLimit", json!(3)),
        ("/items/0/settings/weight", json!(50)),
        (
            "/items/0/settings/groupIds",
            json!(["grp_01994fcba33373338000000000000003"]),
        ),
    ] {
        let mut changed = body.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            send(
                &fixture,
                "POST",
                "/api/admin/accounts/import-tasks",
                changed,
                true
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
    }
    let mut changed = body;
    changed["items"][0]["outboundProxyId"] = json!("synthetic-proxy");
    assert_eq!(
        send(
            &fixture,
            "POST",
            "/api/admin/accounts/import-tasks",
            changed,
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}
