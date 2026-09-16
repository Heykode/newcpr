use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use gateway_admin::model::user_agent::{OutboundUserAgentView, ProviderUserAgentOverride};
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
fn retired_modes_are_rejected_and_custom_ua_is_preserved() {
    for value in [
        json!({"mode": "qx-compatible"}),
        json!({"mode": "qx-compatible", "userAgent": null}),
        json!({"mode": "qx-compatible", "userAgent": ""}),
        json!({"mode": "qx-compatible", "userAgent": "   "}),
    ] {
        assert!(serde_json::from_value::<UpdateOutboundUserAgentRequest>(value).is_err());
    }
    let raw = "codex_cli_rs/0.146.0 (Linux 6.8.0; x86_64) unknown";
    let request: UpdateOutboundUserAgentRequest =
        serde_json::from_value(json!({"mode": "custom", "userAgent": raw})).unwrap();
    let expected = ProviderUserAgentOverride::Custom {
        user_agent: raw.to_owned(),
    };
    assert_eq!(ProviderUserAgentOverride::from(request), expected);
}

#[test]
fn custom_view_does_not_emit_retired_transport_choices() {
    for user_agent in ["custom-cli".to_owned(), "custom-desktop".to_owned()] {
        let view = OutboundUserAgentView {
            selection: ProviderUserAgentOverride::Custom {
                user_agent: user_agent.clone(),
            },
            default_user_agent: "default-desktop".to_owned(),
            effective_user_agent: user_agent.clone(),
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
        assert_eq!(wire["mode"], "custom");
        assert_eq!(wire["customUserAgent"], json!(user_agent));
        for removed in ["qxDefaultUserAgent", "tlsProfile", "sessionPolicy"] {
            assert!(wire.get(removed).is_none());
        }
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
fn independent_wire_is_no_longer_a_live_configuration_mode() {
    for user_agent in [
        None,
        Some("codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color"),
    ] {
        for tls_profile in ["cpr", "qx-compatible"] {
            for session_policy in ["native", "qx-compatible"] {
                let wire = json!({
                    "mode": "independent",
                    "userAgent": user_agent,
                    "tlsProfile": tls_profile,
                    "sessionPolicy": session_policy,
                });
                assert!(serde_json::from_value::<UpdateOutboundUserAgentRequest>(wire).is_err());
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
fn default_and_custom_views_keep_explicit_ua_selection() {
    for user_agent in [None, Some("desktop-ua".to_owned())] {
        let view = OutboundUserAgentView {
            selection: user_agent
                .clone()
                .map_or(ProviderUserAgentOverride::Default, |user_agent| {
                    ProviderUserAgentOverride::Custom { user_agent }
                }),
            default_user_agent: "default-desktop".to_owned(),
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
        assert_eq!(
            wire["mode"],
            if user_agent.is_some() {
                "custom"
            } else {
                "default"
            }
        );
        assert_eq!(wire["customUserAgent"], json!(user_agent));
        assert!(wire.get("tlsProfile").is_none());
        assert!(wire.get("sessionPolicy").is_none());
        assert_eq!(wire["verified"], false);
    }
}
