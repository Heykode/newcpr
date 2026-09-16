use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use gateway_admin::model::user_agent::{
    OutboundUserAgentView, ProviderSessionPolicy, ProviderTlsProfile, ProviderUserAgentOverride,
};
use gateway_api::admin::outbound_user_agent::{self, UpdateOutboundUserAgentRequest};
use serde_json::json;
use tower::ServiceExt;

use super::{AdminTestFixture, AdminTestState};

#[test]
fn outbound_ua_wire_requires_explicit_mode_and_rejects_unknown_fields() {
    for value in [
        json!({}),
        json!({"mode": "custom"}),
        json!({"mode": "guess", "userAgent": "anything"}),
        json!({"mode": "default", "userAgent": "ignored"}),
        json!({"mode": "custom", "userAgent": "value", "other": true}),
        json!({"mode": "qx-compatible", "userAgent": 42}),
        json!({"mode": "qx-compatible", "tls": "guess"}),
    ] {
        assert!(serde_json::from_value::<UpdateOutboundUserAgentRequest>(value).is_err());
    }
    assert!(
        serde_json::from_value::<UpdateOutboundUserAgentRequest>(json!({"mode":"default"})).is_ok()
    );
    assert!(
        serde_json::from_value::<UpdateOutboundUserAgentRequest>(
            json!({"mode":"custom","userAgent":"value"})
        )
        .is_ok()
    );
}

#[test]
fn qx_wire_normalizes_absent_null_and_blank_without_inferring_mode_from_ua() {
    for value in [
        json!({"mode": "qx-compatible"}),
        json!({"mode": "qx-compatible", "userAgent": null}),
        json!({"mode": "qx-compatible", "userAgent": ""}),
        json!({"mode": "qx-compatible", "userAgent": "   "}),
    ] {
        let request: UpdateOutboundUserAgentRequest = serde_json::from_value(value).unwrap();
        assert_eq!(
            ProviderUserAgentOverride::from(request),
            ProviderUserAgentOverride::QxCompatible { user_agent: None }
        );
    }
    let raw = "codex_cli_rs/0.146.0 (Linux 6.8.0; x86_64) unknown";
    for mode in ["custom", "qx-compatible"] {
        let request: UpdateOutboundUserAgentRequest =
            serde_json::from_value(json!({"mode": mode, "userAgent": raw})).unwrap();
        let expected = if mode == "custom" {
            ProviderUserAgentOverride::Custom {
                user_agent: raw.to_owned(),
            }
        } else {
            ProviderUserAgentOverride::QxCompatible {
                user_agent: Some(raw.to_owned()),
            }
        };
        assert_eq!(ProviderUserAgentOverride::from(request), expected);
    }
}

#[test]
fn qx_view_serializes_explicit_mode_and_optional_override() {
    for user_agent in [None, Some("custom-cli".to_owned())] {
        let view = OutboundUserAgentView {
            selection: ProviderUserAgentOverride::QxCompatible {
                user_agent: user_agent.clone(),
            },
            default_user_agent: "default-desktop".to_owned(),
            qx_default_user_agent: "default-cli".to_owned(),
            effective_user_agent: user_agent
                .clone()
                .unwrap_or_else(|| "default-cli".to_owned()),
            effective_desktop_user_agent: "desktop-surface".to_owned(),
            core_version: "0.146.0".to_owned(),
            desktop_version: "26.803.81509".to_owned(),
            os_type: "Ubuntu".to_owned(),
            os_version: "22.4.0".to_owned(),
            arch: "x86_64".to_owned(),
            terminal: "xterm-256color".to_owned(),
            verified: false,
            default_verified_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        };
        let wire = serde_json::to_value(outbound_user_agent::OutboundUserAgentSettingsView::from(
            view,
        ))
        .unwrap();
        assert_eq!(wire["mode"], "qx-compatible");
        assert_eq!(wire["customUserAgent"], json!(user_agent));
        assert_eq!(wire["qxDefaultUserAgent"], "default-cli");
        assert_eq!(wire["effectiveDesktopUserAgent"], "desktop-surface");
        assert_eq!(wire["verified"], false);
    }
}

#[tokio::test]
async fn outbound_ua_read_preview_and_save_require_admin_authentication() {
    let fixture = AdminTestFixture::new().await;
    let app = outbound_user_agent::router::<AdminTestState>().with_state(fixture.state());
    for (method, path) in [
        ("GET", "/api/admin/settings/openai-user-agent"),
        ("POST", "/api/admin/settings/openai-user-agent"),
        ("POST", "/api/admin/settings/openai-user-agent/preview"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"mode":"default"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[test]
fn independent_wire_requires_both_choices_and_preserves_null_or_exact_custom_ua() {
    for user_agent in [
        None,
        Some("codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color"),
    ] {
        for tls_profile in [ProviderTlsProfile::Cpr, ProviderTlsProfile::QxCompatible] {
            for session_policy in [
                ProviderSessionPolicy::Native,
                ProviderSessionPolicy::QxCompatible,
            ] {
                let wire = json!({
                    "mode": "independent",
                    "userAgent": user_agent,
                    "tlsProfile": tls_profile.as_str(),
                    "sessionPolicy": session_policy.as_str(),
                });
                let decoded =
                    serde_json::from_value::<UpdateOutboundUserAgentRequest>(wire).unwrap();
                assert_eq!(
                    ProviderUserAgentOverride::from(decoded),
                    ProviderUserAgentOverride::Independent {
                        user_agent: user_agent.map(str::to_owned),
                        tls_profile,
                        session_policy,
                    }
                );
            }
        }
    }
    for wire in [
        json!({"mode":"independent","userAgent":null}),
        json!({"mode":"independent","userAgent":null,"tlsProfile":"cpr"}),
        json!({"mode":"independent","userAgent":null,"sessionPolicy":"native"}),
        json!({"mode":"independent","userAgent":null,"tlsProfile":null,"sessionPolicy":"native"}),
        json!({"mode":"independent","userAgent":null,"tlsProfile":"cpr","sessionPolicy":null}),
        json!({"mode":"independent","userAgent":null,"tlsProfile":"automatic","sessionPolicy":"native"}),
        json!({"mode":"independent","userAgent":null,"tlsProfile":"cpr","sessionPolicy":"automatic"}),
        json!({"mode":"independent","userAgent":"","tlsProfile":"cpr","sessionPolicy":"native"}),
        json!({"mode":"independent","userAgent":"   ","tlsProfile":"cpr","sessionPolicy":"native"}),
        json!({"mode":"independent","userAgent":42,"tlsProfile":"cpr","sessionPolicy":"native"}),
        json!({"mode":"independent","userAgent":null,"tlsProfile":"cpr","sessionPolicy":"native","guess":true}),
        json!({"mode":"default","tlsProfile":"cpr"}),
        json!({"mode":"custom","userAgent":"value","sessionPolicy":"native"}),
        json!({"mode":"qx-compatible","tlsProfile":null}),
    ] {
        assert!(
            serde_json::from_value::<UpdateOutboundUserAgentRequest>(wire.clone()).is_err(),
            "{wire}"
        );
    }
}

#[test]
fn independent_view_includes_atomic_tls_and_session_choices_without_ua_inference() {
    for user_agent in [None, Some("desktop-ua".to_owned())] {
        let view = OutboundUserAgentView {
            selection: ProviderUserAgentOverride::Independent {
                user_agent: user_agent.clone(),
                tls_profile: ProviderTlsProfile::QxCompatible,
                session_policy: ProviderSessionPolicy::Native,
            },
            default_user_agent: "default-desktop".to_owned(),
            qx_default_user_agent: "default-cli".to_owned(),
            effective_user_agent: user_agent
                .clone()
                .unwrap_or_else(|| "default-desktop".to_owned()),
            effective_desktop_user_agent: "desktop-surface".to_owned(),
            core_version: "1.0.0".to_owned(),
            desktop_version: "26.1".to_owned(),
            os_type: "Mac OS".to_owned(),
            os_version: "15.7.1".to_owned(),
            arch: "arm64".to_owned(),
            terminal: "unknown".to_owned(),
            verified: false,
            default_verified_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        };
        let wire = serde_json::to_value(outbound_user_agent::OutboundUserAgentSettingsView::from(
            view,
        ))
        .unwrap();
        assert_eq!(wire["mode"], "independent");
        assert_eq!(wire["customUserAgent"], json!(user_agent));
        assert_eq!(wire["tlsProfile"], "qx-compatible");
        assert_eq!(wire["sessionPolicy"], "native");
        assert_eq!(wire["verified"], false);
    }
}
