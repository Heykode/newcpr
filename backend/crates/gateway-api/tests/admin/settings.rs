use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use gateway_api::admin::settings::{self, UpdateRuntimeSettingsRequest};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{AdminTestFixture, AdminTestState};

fn app(state: AdminTestState) -> Router {
    settings::router::<AdminTestState>().with_state(state)
}

fn request(method: Method, path: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, "cpr_admin_session=valid-session")
        .header("x-request-id", "req_admin_settings");
    let body = if let Some(value) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(value.to_string())
    } else {
        Body::empty()
    };
    builder.body(body).expect("build settings request")
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read response body");
    serde_json::from_slice(&bytes).expect("parse response JSON")
}

fn update_body() -> Value {
    json!({
        "excelDefaultModels": ["gpt-5.6-sol", "gpt-6-astra"],
        "disableFast": false,
        "turnStateInjectionEnabled": false,
        "turnStateModels": ["gpt-6-astra", "gpt-5.6-sol", "gpt-5.6-terra"],
        "turnStateProbeProxyId": null,
        "turnStateProbeConcurrency": 3,
        "responsesMaxDecompressedBodyBytes": 67108864,
        "modelMappings": {
            "gpt-5.4": "gpt-5.5",
            "grok-latest": "grok-4.5"
        },
        "refreshMarginSeconds": 1800,
        "refreshConcurrency": 4,
        "maxConcurrentPerAccount": 5,
        "requestIntervalMs": 25,
        "rotationStrategy": "round_robin",
        "minCodexDesktopVersion": "26.825.6671",
        "minCodexCliVersion": "0.40.0",
        "usageRetentionDays": 32,
        "opsEventRetentionDays": 31,
        "auditRetentionDays": 91,
        "requestTuning": {
            "maxAccountSwitches": 7,
            "maxRequestAttempts": 8,
            "websocketMaxRetries": 9,
            "websocketHttpFallbackEnabled": false,
            "websocketLargeRequestThresholdBytes": 4096,
            "websocketMaxAgeMs": 60000,
            "websocketStreamIdleTimeoutMs": 120000,
            "websocketFailureThreshold": 3,
            "websocketFailureWindowMs": 30000,
            "websocketFailureOpenDurationMs": 45000,
            "rateLimitCooldownSeconds": 60,
            "openaiLocationOverrideEnabled": false,
            "openaiRequestLocation": null,
            "maxWaitingPerKey": 0,
            "keyConcurrencyWaitTimeoutSeconds": 30,
            "accountBusyWaitEnabled": true,
            "accountBusyWaitStickyMaxWaiting": 4,
            "accountBusyWaitStickyTimeoutSeconds": 121,
            "accountBusyWaitFallbackMaxWaiting": 101,
            "accountBusyWaitFallbackTimeoutSeconds": 31
        }
    })
}

#[test]
fn settings_request_should_reject_unknown_rotation_strategy() {
    let mut body = update_body();
    body["rotationStrategy"] = json!("random");
    let request: UpdateRuntimeSettingsRequest =
        serde_json::from_value(body).expect("decode settings");

    assert_eq!(request.validate().unwrap_err().field(), "rotationStrategy");
}

#[tokio::test]
async fn probe_proxy_selection_preserves_omitted_and_clears_explicit_null() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let app = app(fixture.state());
    for (value, expected) in [
        (Some(json!("proxy-test")), json!("proxy-test")),
        (None, json!("proxy-test")),
        (Some(Value::Null), Value::Null),
    ] {
        let mut body = update_body();
        if let Some(value) = value {
            body["turnStateProbeProxyId"] = value;
        } else {
            body.as_object_mut()
                .unwrap()
                .remove("turnStateProbeProxyId");
        }
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/admin/settings/update",
                Some(body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app
            .clone()
            .oneshot(request(Method::GET, "/api/admin/settings", None))
            .await
            .unwrap();
        assert_eq!(
            response_json(response).await["data"]["turnStateProbeProxyId"],
            expected
        );
    }
}

#[test]
fn probe_concurrency_rejects_out_of_range_and_non_integer_values() {
    for value in [0, 11, u32::MAX] {
        let mut body = update_body();
        body["turnStateProbeConcurrency"] = json!(value);
        let request: UpdateRuntimeSettingsRequest = serde_json::from_value(body).unwrap();
        assert_eq!(
            request.validate().unwrap_err().field(),
            "turnStateProbeConcurrency"
        );
    }
    for value in [json!(-1), json!(1.5), json!("3"), json!(true)] {
        let mut body = update_body();
        body["turnStateProbeConcurrency"] = value;
        assert!(serde_json::from_value::<UpdateRuntimeSettingsRequest>(body).is_err());
    }
}

#[tokio::test]
async fn probe_concurrency_round_trips_and_omission_preserves_the_saved_value() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let app = app(fixture.state());
    for (value, expected) in [
        (None, 3),
        (Some(1), 1),
        (Some(10), 10),
        (None, 10),
        (Some(3), 3),
    ] {
        let mut body = update_body();
        if let Some(value) = value {
            body["turnStateProbeConcurrency"] = json!(value);
        } else {
            body.as_object_mut()
                .unwrap()
                .remove("turnStateProbeConcurrency");
        }
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/admin/settings/update",
                Some(body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]["turnStateProbeConcurrency"],
            expected
        );
        let response = app
            .clone()
            .oneshot(request(Method::GET, "/api/admin/settings", None))
            .await
            .unwrap();
        assert_eq!(
            response_json(response).await["data"]["turnStateProbeConcurrency"],
            expected
        );
    }
}

#[test]
fn probe_proxy_selection_rejects_invalid_ids() {
    for id in ["", " ", "proxy\nid", "proxy id", &"a".repeat(257)] {
        let mut body = update_body();
        body["turnStateProbeProxyId"] = json!(id);
        let request: UpdateRuntimeSettingsRequest = serde_json::from_value(body).unwrap();
        assert_eq!(
            request.validate().unwrap_err().field(),
            "turnStateProbeProxyId"
        );
    }
}

#[test]
fn settings_request_should_reject_non_semver_client_min() {
    let mut body = update_body();
    body["minCodexCliVersion"] = json!("v0.40.0");
    let request: UpdateRuntimeSettingsRequest =
        serde_json::from_value(body).expect("decode settings");

    assert_eq!(
        request.validate().unwrap_err().field(),
        "minCodexCliVersion"
    );
}

#[test]
fn settings_request_should_reject_unbounded_request_tuning() {
    let mut body = update_body();
    body["requestTuning"]["maxRequestAttempts"] = json!(33);
    let request: UpdateRuntimeSettingsRequest =
        serde_json::from_value(body).expect("decode settings");

    assert_eq!(request.validate().unwrap_err().field(), "requestTuning");
}

#[test]
fn decompression_setting_should_reject_zero_and_values_above_the_safety_limit() {
    for invalid in [json!(0), json!(268435457_u64)] {
        let mut body = update_body();
        body["responsesMaxDecompressedBodyBytes"] = invalid;
        let request: UpdateRuntimeSettingsRequest =
            serde_json::from_value(body).expect("decode settings");
        assert_eq!(
            request.validate().unwrap_err().field(),
            "responsesMaxDecompressedBodyBytes"
        );
    }
}

#[tokio::test]
async fn disable_fast_settings_updates_preserve_omitted_values() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let app = app(fixture.state());
    for (value, expected) in [(Some(true), true), (None, true), (Some(false), false)] {
        let mut body = update_body();
        if let Some(value) = value {
            body["disableFast"] = json!(value);
        } else {
            body.as_object_mut().unwrap().remove("disableFast");
        }
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/admin/settings/update",
                Some(body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app
            .clone()
            .oneshot(request(Method::GET, "/api/admin/settings", None))
            .await
            .unwrap();
        assert_eq!(
            response_json(response).await["data"]["disableFast"],
            expected
        );
    }
}

#[test]
fn settings_request_should_reject_unknown_request_tuning_field() {
    for field in [
        "unknown",
        "websocketMaxConnectng",
        "websocket_max_connecting",
    ] {
        let mut body = update_body();
        body["requestTuning"]["websocketMaxConnecting"] = json!(4);
        body["requestTuning"][field] = json!(1);

        assert!(serde_json::from_value::<UpdateRuntimeSettingsRequest>(body).is_err());
    }
}

#[test]
fn settings_request_should_accept_and_discard_legacy_global_opening_limit() {
    let expected: UpdateRuntimeSettingsRequest =
        serde_json::from_value(update_body()).expect("decode current settings");
    for legacy in [json!(4), json!(null), json!(0), json!(257)] {
        let mut body = update_body();
        body["requestTuning"]["websocketMaxConnecting"] = legacy;
        let request: UpdateRuntimeSettingsRequest =
            serde_json::from_value(body).expect("decode legacy settings");
        request.validate().expect("legacy field has no effect");
        assert_eq!(request, expected);
    }
}

#[test]
fn settings_request_should_not_accept_legacy_global_opening_limit_at_top_level() {
    let mut body = update_body();
    body["websocketMaxConnecting"] = json!(4);

    assert!(serde_json::from_value::<UpdateRuntimeSettingsRequest>(body).is_err());
}

#[test]
fn settings_response_should_cover_the_full_runtime_settings_contract() {
    use std::collections::BTreeMap;

    use chrono::{TimeZone, Utc};
    use gateway_admin::model::Revision;
    use gateway_admin::model::settings::{RequestTuningOverrides, RuntimeSettings};
    use gateway_api::admin::settings::RuntimeSettingsView;
    use gateway_core::account::RotationStrategy;
    use gateway_core::routing::{PublicModelId, UpstreamModelId};

    let settings = RuntimeSettings {
        turn_state_probe_proxy_id: None,
        turn_state_probe_concurrency: 3,
        config_revision: Revision::new(7).expect("revision"),
        disable_fast: false,
        turn_state_injection_enabled: false,
        excel_default_models: Default::default(),
        turn_state_models: vec![
            UpstreamModelId::new("gpt-6-astra").expect("turn state model"),
            UpstreamModelId::new("gpt-5.6-sol").expect("turn state model"),
            UpstreamModelId::new("gpt-5.6-terra").expect("turn state model"),
        ],
        responses_max_decompressed_body_bytes: 64 * 1024 * 1024,
        model_mappings: BTreeMap::from_iter([
            (
                PublicModelId::new("gpt-5.4").expect("public model"),
                UpstreamModelId::new("gpt-5.5").expect("upstream model"),
            ),
            (
                PublicModelId::new("grok-latest").expect("public model"),
                UpstreamModelId::new("grok-4.5").expect("upstream model"),
            ),
        ]),
        refresh_margin_seconds: 1800,
        refresh_concurrency: 4,
        max_concurrent_per_account: 5,
        request_interval_ms: 25,
        rotation_strategy: RotationStrategy::RoundRobin,
        min_codex_desktop_version: Some("26.825.6671".to_owned()),
        min_codex_cli_version: Some("0.40.0".to_owned()),
        usage_retention_days: 32,
        ops_event_retention_days: 31,
        audit_retention_days: 91,
        request_tuning: RequestTuningOverrides {
            openai_request_location: None,
            max_account_switches: Some(7),
            max_request_attempts: Some(8),
            websocket_max_retries: Some(9),
            websocket_http_fallback_enabled: Some(false),
            websocket_large_request_threshold_bytes: Some(4096),
            websocket_max_age_ms: Some(60_000),
            websocket_stream_idle_timeout_ms: Some(120_000),
            websocket_failure_threshold: Some(3),
            websocket_failure_window_ms: Some(30_000),
            websocket_failure_open_duration_ms: Some(45_000),
            rate_limit_cooldown_seconds: Some(60),
            openai_location_override_enabled: Some(true),
            max_waiting_per_key: Some(8),
            key_concurrency_wait_timeout_seconds: Some(30),
            account_busy_wait_enabled: Some(true),
            account_busy_wait_sticky_max_waiting: Some(4),
            account_busy_wait_sticky_timeout_seconds: Some(121),
            account_busy_wait_fallback_max_waiting: Some(101),
            account_busy_wait_fallback_timeout_seconds: Some(31),
        },
        updated_at: Utc
            .with_ymd_and_hms(2026, 8, 2, 10, 30, 0)
            .single()
            .expect("timestamp"),
    };

    let value = serde_json::to_value(RuntimeSettingsView::from(settings)).expect("serialize view");
    assert_eq!(
        value,
        json!({
            "disableFast": false,
            "excelDefaultModels": ["gpt-5.6-sol", "gpt-6-astra"],
            "turnStateInjectionEnabled": false,
            "turnStateModels": ["gpt-6-astra", "gpt-5.6-sol", "gpt-5.6-terra"],
            "turnStateProbeProxyId": null,
            "turnStateProbeConcurrency": 3,
            "responsesMaxDecompressedBodyBytes": 67108864,
            "modelMappings": {
                "gpt-5.4": "gpt-5.5",
                "grok-latest": "grok-4.5"
            },
            "refreshMarginSeconds": 1800,
            "refreshConcurrency": 4,
            "maxConcurrentPerAccount": 5,
            "requestIntervalMs": 25,
            "rotationStrategy": "round_robin",
            "minCodexDesktopVersion": "26.825.6671",
            "minCodexCliVersion": "0.40.0",
            "usageRetentionDays": 32,
            "opsEventRetentionDays": 31,
            "auditRetentionDays": 91,
            "requestTuning": {
                "maxAccountSwitches": 7,
                "maxRequestAttempts": 8,
                "websocketMaxRetries": 9,
                "websocketHttpFallbackEnabled": false,
                "websocketLargeRequestThresholdBytes": 4096,
                "websocketMaxAgeMs": 60000,
                "websocketStreamIdleTimeoutMs": 120000,
                "websocketFailureThreshold": 3,
                "websocketFailureWindowMs": 30000,
                "websocketFailureOpenDurationMs": 45000,
                "rateLimitCooldownSeconds": 60,
                "openaiLocationOverrideEnabled": true,
                "openaiRequestLocation": null,
                "maxWaitingPerKey": 8,
                "keyConcurrencyWaitTimeoutSeconds": 30,
                "accountBusyWaitEnabled": true,
                "accountBusyWaitStickyMaxWaiting": 4,
                "accountBusyWaitStickyTimeoutSeconds": 121,
                "accountBusyWaitFallbackMaxWaiting": 101,
                "accountBusyWaitFallbackTimeoutSeconds": 31
            },
            "updatedAt": "2026-08-02T10:30:00Z"
        })
    );
}

#[test]
fn settings_request_and_response_fields_should_stay_in_lockstep() {
    use std::collections::{BTreeMap, BTreeSet};

    use gateway_admin::model::Revision;
    use gateway_admin::model::settings::RuntimeSettings;
    use gateway_api::admin::settings::RuntimeSettingsView;
    use gateway_core::account::RotationStrategy;
    use gateway_core::routing::{PublicModelId, UpstreamModelId};

    let request: UpdateRuntimeSettingsRequest =
        serde_json::from_value(update_body()).expect("decode settings");
    request.validate().expect("fixture settings must validate");

    let request_fields: BTreeSet<String> = update_body()
        .as_object()
        .expect("request body object")
        .keys()
        .cloned()
        .collect();
    let settings = RuntimeSettings {
        config_revision: Revision::new(7).expect("revision"),
        turn_state_probe_proxy_id: request.turn_state_probe_proxy_id.clone().flatten(),
        turn_state_probe_concurrency: request.turn_state_probe_concurrency.unwrap_or(3),
        disable_fast: request.disable_fast.unwrap_or(false),
        turn_state_injection_enabled: request.turn_state_injection_enabled.unwrap_or(false),
        excel_default_models: Default::default(),
        turn_state_models: request
            .turn_state_models
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|model| UpstreamModelId::new(model.clone()).expect("turn state model"))
            .collect(),
        responses_max_decompressed_body_bytes: request
            .responses_max_decompressed_body_bytes
            .unwrap_or(64 * 1024 * 1024),
        model_mappings: request
            .model_mappings
            .iter()
            .map(|(public, upstream)| {
                Ok((
                    PublicModelId::new(public.clone())?,
                    UpstreamModelId::new(upstream.clone())?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, gateway_core::error::IdentifierError>>()
            .expect("valid model mappings"),
        refresh_margin_seconds: request.refresh_margin_seconds,
        refresh_concurrency: u32::try_from(request.refresh_concurrency).expect("u32"),
        max_concurrent_per_account: u32::try_from(request.max_concurrent_per_account).expect("u32"),
        request_interval_ms: request.request_interval_ms,
        rotation_strategy: RotationStrategy::parse(&request.rotation_strategy)
            .expect("fixture rotation strategy"),
        min_codex_desktop_version: request.min_codex_desktop_version,
        min_codex_cli_version: request.min_codex_cli_version,
        usage_retention_days: u32::try_from(request.usage_retention_days).expect("u32"),
        ops_event_retention_days: u32::try_from(request.ops_event_retention_days).expect("u32"),
        audit_retention_days: u32::try_from(request.audit_retention_days).expect("u32"),
        request_tuning: request.request_tuning,
        updated_at: chrono::Utc::now(),
    };

    let response_fields: BTreeSet<String> =
        serde_json::to_value(RuntimeSettingsView::from(settings))
            .expect("serialize view")
            .as_object()
            .expect("view object")
            .keys()
            .cloned()
            .collect();
    let mut expected_fields = request_fields;
    expected_fields.insert("updatedAt".to_owned());

    assert_eq!(response_fields, expected_fields);
}

#[test]
fn settings_request_should_reject_unknown_revision_field() {
    let mut body = update_body();
    body["expectedConfigRevision"] = json!(7);

    assert!(serde_json::from_value::<UpdateRuntimeSettingsRequest>(body).is_err());
}

#[test]
fn settings_request_should_reject_removed_bucket_retention() {
    let mut body = update_body();
    body["bucketRetentionDays"] = json!(365);

    assert!(serde_json::from_value::<UpdateRuntimeSettingsRequest>(body).is_err());
}

#[tokio::test]
async fn settings_get_should_preserve_global_model_mappings() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let response = app(fixture.state())
        .oneshot(request(Method::GET, "/api/admin/settings", None))
        .await
        .expect("settings response");
    let data = response_json(response).await["data"].clone();

    assert_eq!(
        (
            data["modelMappings"]["coding-default"].as_str(),
            data["modelMappings"]["grok-latest"].as_str(),
            data["rotationStrategy"].as_str()
        ),
        (Some("gpt-5.4"), Some("grok-4.5"), Some("smart"))
    );
}

#[tokio::test]
async fn settings_post_should_replace_global_model_mappings() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let response = app(fixture.state())
        .oneshot(request(
            Method::POST,
            "/api/admin/settings/update",
            Some(update_body()),
        ))
        .await
        .expect("settings update response");
    let data = response_json(response).await["data"].clone();

    assert!(data.get("configRevision").is_none());
    assert_eq!(data["modelMappings"]["gpt-5.4"], "gpt-5.5");
    assert_eq!(data["modelMappings"]["grok-latest"], "grok-4.5");
}

#[tokio::test]
async fn settings_post_and_reload_should_omit_legacy_global_opening_limit() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let expected_tuning = update_body()["requestTuning"].clone();
    let mut legacy_body = update_body();
    legacy_body["requestTuning"]["websocketMaxConnecting"] = json!(4);

    for body in [legacy_body, update_body()] {
        let response = app(fixture.state())
            .oneshot(request(
                Method::POST,
                "/api/admin/settings/update",
                Some(body),
            ))
            .await
            .expect("settings update response");
        assert_eq!(response.status(), StatusCode::OK);
        let data = response_json(response).await["data"].clone();
        assert_eq!(data["requestTuning"], expected_tuning);

        let response = app(fixture.state())
            .oneshot(request(Method::GET, "/api/admin/settings", None))
            .await
            .expect("settings reload response");
        assert_eq!(response.status(), StatusCode::OK);
        let data = response_json(response).await["data"].clone();
        assert_eq!(data["requestTuning"], expected_tuning);
    }
}

#[tokio::test]
async fn large_request_threshold_round_trips_and_rejects_out_of_range_settings() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for value in [json!(null), json!(0), json!(4096), json!(64 * 1024 * 1024)] {
        let mut body = update_body();
        body["requestTuning"]["websocketLargeRequestThresholdBytes"] = value;
        let response = app(fixture.state())
            .oneshot(request(
                Method::POST,
                "/api/admin/settings/update",
                Some(body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app(fixture.state())
            .oneshot(request(Method::GET, "/api/admin/settings", None))
            .await
            .unwrap();
        let data = response_json(response).await["data"].clone();
        assert_eq!(data["requestTuning"], body["requestTuning"]);
        assert_eq!(data["rotationStrategy"], body["rotationStrategy"]);
    }
    for (value, expected_status) in [
        (json!(-1), StatusCode::UNPROCESSABLE_ENTITY),
        (json!(1.5), StatusCode::UNPROCESSABLE_ENTITY),
        (json!(64 * 1024 * 1024 + 1), StatusCode::BAD_REQUEST),
    ] {
        let mut body = update_body();
        body["requestTuning"]["websocketLargeRequestThresholdBytes"] = value;
        let response = app(fixture.state())
            .oneshot(request(
                Method::POST,
                "/api/admin/settings/update",
                Some(body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected_status);
    }
}

#[tokio::test]
async fn location_and_key_wait_settings_round_trip_without_changing_account_waiting() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (enabled, waiting, timeout) in [(true, 8, 45), (false, 0, 45)] {
        let mut body = update_body();
        body["requestTuning"]["openaiLocationOverrideEnabled"] = json!(enabled);
        body["requestTuning"]["openaiRequestLocation"] = if enabled {
            json!({"country":"JP","region":"Tokyo","city":"Tokyo","timezone":"Asia/Tokyo"})
        } else {
            serde_json::Value::Null
        };
        body["requestTuning"]["maxWaitingPerKey"] = json!(waiting);
        body["requestTuning"]["keyConcurrencyWaitTimeoutSeconds"] = json!(timeout);
        let response = app(fixture.state())
            .oneshot(request(
                Method::POST,
                "/api/admin/settings/update",
                Some(body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app(fixture.state())
            .oneshot(request(Method::GET, "/api/admin/settings", None))
            .await
            .unwrap();
        let data = response_json(response).await["data"].clone();
        assert_eq!(data["requestTuning"], body["requestTuning"]);
        assert_eq!(data["rotationStrategy"], body["rotationStrategy"]);
    }
}

#[tokio::test]
async fn custom_location_settings_reject_invalid_values_without_mutation() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let response = app(fixture.state())
        .oneshot(request(Method::GET, "/api/admin/settings", None))
        .await
        .unwrap();
    let original = response_json(response).await["data"].clone();
    for (field, invalid, expected) in [
        ("country", json!("us"), StatusCode::BAD_REQUEST),
        ("city", json!(""), StatusCode::BAD_REQUEST),
        ("region", json!("bad\nvalue"), StatusCode::BAD_REQUEST),
        (
            "timezone",
            json!("Invalid/Zone"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let mut body = update_body();
        let mut location =
            json!({"country":"JP","region":"Tokyo","city":"Tokyo","timezone":"Asia/Tokyo"});
        location[field] = invalid;
        body["requestTuning"]["openaiRequestLocation"] = location;
        let response = app(fixture.state())
            .oneshot(request(
                Method::POST,
                "/api/admin/settings/update",
                Some(body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{field}");
    }
    let response = app(fixture.state())
        .oneshot(request(Method::GET, "/api/admin/settings", None))
        .await
        .unwrap();
    assert_eq!(response_json(response).await["data"], original);
}

#[tokio::test]
async fn account_busy_wait_settings_should_round_trip_enabled_disabled_and_inherited() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let mut disabled = update_body();
    disabled["requestTuning"]["accountBusyWaitEnabled"] = json!(false);
    let mut inherited = update_body();
    inherited["requestTuning"] = json!({
        "accountBusyWaitEnabled": null,
        "accountBusyWaitStickyMaxWaiting": null,
        "accountBusyWaitStickyTimeoutSeconds": null,
        "accountBusyWaitFallbackMaxWaiting": null,
        "accountBusyWaitFallbackTimeoutSeconds": null
    });
    let mut legacy = update_body();
    legacy
        .as_object_mut()
        .expect("settings object")
        .remove("requestTuning");
    for body in [update_body(), disabled, inherited, legacy] {
        let decoded: UpdateRuntimeSettingsRequest =
            serde_json::from_value(body.clone()).expect("decode settings");
        let expected = serde_json::to_value(decoded.request_tuning).expect("serialize overrides");
        let response = app(fixture.state())
            .oneshot(request(
                Method::POST,
                "/api/admin/settings/update",
                Some(body),
            ))
            .await
            .expect("save wait settings");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]["requestTuning"],
            expected
        );
        let response = app(fixture.state())
            .oneshot(request(Method::GET, "/api/admin/settings", None))
            .await
            .expect("reload wait settings");
        assert_eq!(response.status(), StatusCode::OK);
        let data = response_json(response).await["data"].clone();
        assert_eq!(data["requestTuning"], expected);
        assert_eq!(data["modelMappings"], update_body()["modelMappings"]);
        assert_eq!(data["maxConcurrentPerAccount"], 5);
    }
}

#[tokio::test]
async fn account_busy_wait_settings_should_reject_invalid_values_without_mutation() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let response = app(fixture.state())
        .oneshot(request(Method::GET, "/api/admin/settings", None))
        .await
        .expect("read original settings");
    let original = response_json(response).await["data"].clone();
    for (field, maximum) in [
        ("accountBusyWaitStickyMaxWaiting", 1000),
        ("accountBusyWaitStickyTimeoutSeconds", 600),
        ("accountBusyWaitFallbackMaxWaiting", 1000),
        ("accountBusyWaitFallbackTimeoutSeconds", 600),
    ] {
        for (invalid, expected_status) in [
            (json!(0), StatusCode::BAD_REQUEST),
            (json!(maximum + 1), StatusCode::BAD_REQUEST),
            (json!(-1), StatusCode::UNPROCESSABLE_ENTITY),
            (json!(1.5), StatusCode::UNPROCESSABLE_ENTITY),
            (json!("3"), StatusCode::UNPROCESSABLE_ENTITY),
        ] {
            let mut body = update_body();
            body["requestTuning"]["accountBusyWaitEnabled"] = json!(false);
            body["requestTuning"][field] = invalid;
            let response = app(fixture.state())
                .oneshot(request(
                    Method::POST,
                    "/api/admin/settings/update",
                    Some(body),
                ))
                .await
                .expect("reject invalid wait setting");
            assert_eq!(response.status(), expected_status, "{field}");
        }
    }
    for invalid in [json!(0), json!(1), json!("false")] {
        let mut body = update_body();
        body["requestTuning"]["accountBusyWaitEnabled"] = invalid;
        let response = app(fixture.state())
            .oneshot(request(
                Method::POST,
                "/api/admin/settings/update",
                Some(body),
            ))
            .await
            .expect("reject non-boolean switch");
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    let response = app(fixture.state())
        .oneshot(request(Method::GET, "/api/admin/settings", None))
        .await
        .expect("reload unchanged settings");
    assert_eq!(response_json(response).await["data"], original);
}

#[tokio::test]
async fn client_downloads_should_return_validated_direct_links() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let response = app(fixture.state())
        .oneshot(request(
            Method::GET,
            "/api/admin/settings/client-downloads/codex-desktop/windows?refresh=true",
            None,
        ))
        .await
        .expect("client downloads response");

    assert_eq!(response.status(), StatusCode::OK);
    let data = response_json(response).await["data"].clone();
    assert_eq!(data["packages"][0]["architecture"], "x64");
    assert_eq!(data["packages"][0]["source"], "microsoft_store");
    assert_eq!(data["packages"][0]["version"], "26.825.6671.0");
    assert!(
        data["packages"][0]["downloadUrl"]
            .as_str()
            .is_some_and(|url| url.starts_with("https://dl.delivery.mp.microsoft.com/"))
    );
}

#[test]
fn settings_request_should_reject_invalid_model_mapping_name() {
    let mut body = update_body();
    body["modelMappings"] = json!({ "\0": "gpt-5.5" });
    let request: UpdateRuntimeSettingsRequest =
        serde_json::from_value(body).expect("decode settings");

    assert_eq!(request.validate().unwrap_err().field(), "modelMappings");
}

#[tokio::test]
async fn settings_should_require_admin_auth() {
    let fixture = AdminTestFixture::new().await;
    let response = app(fixture.state())
        .oneshot(
            Request::builder()
                .uri("/api/admin/settings")
                .header("x-request-id", "req_unauthorized")
                .body(Body::empty())
                .expect("unauthorized request"),
        )
        .await
        .expect("unauthorized response");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_key_should_return_secret_only_on_regenerate() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let response = app(fixture.state())
        .oneshot(request(
            Method::POST,
            "/api/admin/settings/admin-api-key/regenerate",
            None,
        ))
        .await
        .expect("regenerate response");
    let data = response_json(response).await["data"].clone();

    assert!(
        data["key"]
            .as_str()
            .is_some_and(|key| key.starts_with("admin-") && key.len() == 70)
    );
}

#[tokio::test]
async fn admin_key_delete_should_use_fixed_post_path() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    fixture.settings.set_api_key("admin-valid-test-key");
    let response = app(fixture.state())
        .oneshot(request(
            Method::POST,
            "/api/admin/settings/admin-api-key/delete",
            None,
        ))
        .await
        .expect("delete response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn settings_should_accept_admin_api_key_header() {
    let fixture = AdminTestFixture::new().await;
    let key = format!("admin-{}", "a".repeat(64));
    fixture.auth.set_api_key(&key);
    let response = app(fixture.state())
        .oneshot(
            Request::builder()
                .uri("/api/admin/settings")
                .header("x-api-key", key)
                .header("x-request-id", "req_api_key")
                .body(Body::empty())
                .expect("api key request"),
        )
        .await
        .expect("api key response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_auth_should_accept_a_configured_request_id_header_name() {
    use axum::http::HeaderName;
    use tower_http::request_id::{MakeRequestUuid, SetRequestIdLayer};

    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    // 部署把 api.request_id_header 改名后，注入的 header 不再叫 x-request-id；
    // 管理请求仍须拿到请求上下文，而不是退化为 500。
    let custom = HeaderName::from_static("x-trace-id");
    let app = app(fixture.state()).layer(SetRequestIdLayer::new(custom, MakeRequestUuid));
    let unlabelled = Request::builder()
        .method(Method::GET)
        .uri("/api/admin/settings")
        .header(header::COOKIE, "cpr_admin_session=valid-session")
        .body(Body::empty())
        .expect("build settings request");

    let response = app.oneshot(unlabelled).await.expect("settings response");

    assert_eq!(response.status(), StatusCode::OK);
}
