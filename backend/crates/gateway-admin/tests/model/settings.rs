use gateway_admin::model::settings::RequestTuningOverrides;
use gateway_core::routing::RequestTuning;
use serde_json::json;

#[test]
fn request_tuning_overrides_round_trip_all_live_fields() {
    let overrides = RequestTuningOverrides {
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
    };
    assert!(overrides.validate());
    let value = serde_json::to_value(&overrides).expect("serialize overrides");
    assert_eq!(
        value,
        json!({
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
        })
    );
    assert_eq!(
        serde_json::from_value::<RequestTuningOverrides>(value).expect("decode overrides"),
        overrides
    );
    let defaults =
        serde_json::to_value(RequestTuning::default()).expect("serialize runtime defaults");
    assert!(defaults.get("websocketMaxConnecting").is_none());
    assert_eq!(
        defaults["websocketLargeRequestThresholdBytes"],
        15 * 1024 * 1024
    );
    assert_eq!(defaults["openaiLocationOverrideEnabled"], false);
    assert_eq!(defaults["accountBusyWaitEnabled"], false);
    assert_eq!(defaults["accountBusyWaitStickyMaxWaiting"], 3);
    assert_eq!(defaults["accountBusyWaitStickyTimeoutSeconds"], 120);
    assert_eq!(defaults["accountBusyWaitFallbackMaxWaiting"], 100);
    assert_eq!(defaults["accountBusyWaitFallbackTimeoutSeconds"], 30);
}

#[test]
fn large_request_threshold_preserves_inheritance_zero_and_legacy_defaults() {
    let mut legacy = serde_json::to_value(RequestTuning::default()).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("websocketLargeRequestThresholdBytes");
    assert_eq!(
        serde_json::from_value::<RequestTuning>(legacy).unwrap(),
        RequestTuning::default()
    );
    for value in [json!(null), json!(0), json!(1), json!(64 * 1024 * 1024)] {
        let wire = json!({"websocketLargeRequestThresholdBytes": value});
        let overrides: RequestTuningOverrides = serde_json::from_value(wire).unwrap();
        assert!(overrides.validate());
        assert_eq!(
            serde_json::to_value(overrides).unwrap()["websocketLargeRequestThresholdBytes"],
            value
        );
    }
    let too_large: RequestTuningOverrides = serde_json::from_value(
        json!({"websocketLargeRequestThresholdBytes": 64 * 1024 * 1024 + 1}),
    )
    .unwrap();
    assert!(!too_large.validate());
    for value in [json!(-1), json!(1.5), json!("1024"), json!(true)] {
        assert!(
            serde_json::from_value::<RequestTuningOverrides>(
                json!({"websocketLargeRequestThresholdBytes": value}),
            )
            .is_err()
        );
    }
}

#[test]
fn account_busy_wait_overrides_preserve_null_inheritance_and_explicit_false() {
    let inherited = json!({
        "accountBusyWaitEnabled": null,
        "accountBusyWaitStickyMaxWaiting": null,
        "accountBusyWaitStickyTimeoutSeconds": null,
        "accountBusyWaitFallbackMaxWaiting": null,
        "accountBusyWaitFallbackTimeoutSeconds": null
    });
    let decoded: RequestTuningOverrides =
        serde_json::from_value(inherited).expect("decode inherited wait settings");
    assert_eq!(decoded, RequestTuningOverrides::default());
    assert!(decoded.validate());

    let disabled = json!({
        "accountBusyWaitEnabled": false,
        "accountBusyWaitStickyMaxWaiting": 7,
        "accountBusyWaitStickyTimeoutSeconds": 123,
        "accountBusyWaitFallbackMaxWaiting": 111,
        "accountBusyWaitFallbackTimeoutSeconds": 33
    });
    let decoded: RequestTuningOverrides =
        serde_json::from_value(disabled.clone()).expect("decode disabled wait settings");
    assert_eq!(decoded.account_busy_wait_enabled, Some(false));
    assert!(decoded.validate());
    let serialized = serde_json::to_value(decoded).expect("serialize disabled wait settings");
    for (key, value) in disabled.as_object().expect("settings object") {
        assert_eq!(&serialized[key], value);
    }
}

#[test]
fn account_busy_wait_overrides_validate_each_integer_range_even_when_disabled() {
    for (field, maximum) in [
        ("accountBusyWaitStickyMaxWaiting", 1000),
        ("accountBusyWaitStickyTimeoutSeconds", 600),
        ("accountBusyWaitFallbackMaxWaiting", 1000),
        ("accountBusyWaitFallbackTimeoutSeconds", 600),
    ] {
        for enabled in [false, true] {
            for (value, valid) in [(0, false), (1, true), (maximum, true), (maximum + 1, false)] {
                let mut wire = json!({"accountBusyWaitEnabled": enabled});
                wire[field] = json!(value);
                let decoded: RequestTuningOverrides =
                    serde_json::from_value(wire).expect("decode numeric setting");
                assert_eq!(
                    decoded.validate(),
                    valid,
                    "{field}={value}, enabled={enabled}"
                );
            }
        }
        for value in [
            json!(-1),
            json!(1.5),
            json!("3"),
            json!(true),
            json!([]),
            json!({}),
        ] {
            let mut wire = json!({});
            wire[field] = value;
            assert!(
                serde_json::from_value::<RequestTuningOverrides>(wire).is_err(),
                "{field}"
            );
        }
    }
    for value in [json!(0), json!(1), json!("false"), json!([]), json!({})] {
        assert!(
            serde_json::from_value::<RequestTuningOverrides>(
                json!({"accountBusyWaitEnabled": value})
            )
            .is_err()
        );
    }
}

#[test]
fn request_tuning_overrides_discard_only_the_legacy_global_opening_limit() {
    let expected = RequestTuningOverrides {
        max_account_switches: Some(7),
        websocket_http_fallback_enabled: Some(false),
        ..Default::default()
    };
    let serialized = serde_json::to_value(&expected).expect("serialize live overrides");
    for legacy in [
        json!(4),
        json!(null),
        json!(0),
        json!(257),
        json!({"old": true}),
    ] {
        let mut value = serialized.clone();
        value["websocketMaxConnecting"] = legacy;
        let decoded: RequestTuningOverrides =
            serde_json::from_value(value).expect("decode legacy overrides");
        assert_eq!(decoded, expected);
        assert!(decoded.validate());
        assert_eq!(
            serde_json::to_value(decoded).expect("serialize decoded overrides"),
            serialized
        );
    }
}

#[test]
fn request_tuning_overrides_preserve_inheritance_for_empty_and_legacy_only_input() {
    for value in [
        json!({}),
        json!({"websocketMaxConnecting": 4}),
        json!({"websocketMaxConnecting": null}),
    ] {
        let decoded: RequestTuningOverrides =
            serde_json::from_value(value).expect("decode inherited overrides");
        assert_eq!(decoded, RequestTuningOverrides::default());
        let serialized = serde_json::to_value(&decoded).expect("serialize inherited overrides");
        assert!(serialized.get("websocketMaxConnecting").is_none());
        assert_eq!(
            serde_json::from_value::<RequestTuningOverrides>(serialized)
                .expect("round trip inherited overrides"),
            decoded
        );
    }
}

#[test]
fn request_tuning_overrides_keep_unknown_and_malformed_live_fields_strict() {
    for unknown in [
        "unknown",
        "websocketMaxConnectng",
        "websocket_max_connecting",
    ] {
        let mut value = json!({"websocketMaxConnecting": 4});
        value[unknown] = json!(1);
        let error = serde_json::from_value::<RequestTuningOverrides>(value)
            .expect_err("reject unknown field");
        assert!(error.to_string().contains("unknown field"));
    }
    for value in [
        json!({"websocketMaxConnecting": 4, "websocketMaxRetries": "5"}),
        json!({"websocketMaxConnecting": 4, "websocketHttpFallbackEnabled": 1}),
    ] {
        assert!(serde_json::from_value::<RequestTuningOverrides>(value).is_err());
    }
    assert!(
        serde_json::from_str::<RequestTuningOverrides>(
            r#"{"websocketMaxConnecting":4,"maxRequestAttempts":8,"maxRequestAttempts":9}"#
        )
        .is_err()
    );
}
