use chrono::Utc;
use provider_openai::credential::{
    CodexAccountProfile, CodexCookie, CodexCredentialData, CodexCredentialPrincipal,
    CodexOAuthCredentialData, CodexOAuthSecret,
};
use secrecy::SecretString;

#[test]
fn oauth_secret_debug_redacts_every_token() {
    let secret = CodexOAuthSecret {
        access_token: SecretString::from("access-private"),
        refresh_token: Some(SecretString::from("refresh-private")),
        id_token: Some(SecretString::from("id-private")),
    };
    let debug = format!("{secret:?}");
    for value in ["access-private", "refresh-private", "id-private"] {
        assert!(!debug.contains(value));
    }
}

#[test]
fn account_profile_debug_redacts_identity_fields() {
    let profile = CodexAccountProfile {
        email: Some("private@example.com".to_owned()),
        oauth_subject: "subject-private".to_owned(),
        poid: Some("poid-private".to_owned()),
        chatgpt_account_id: "chatgpt-private".to_owned(),
        chatgpt_user_id: "user-private".to_owned(),
        plan_type: Some("pro".to_owned()),
        access_token_expires_at: Some(Utc::now()),
    };
    let debug = format!("{profile:?}");
    assert!(!debug.contains("private@example.com"));
    assert!(!debug.contains("chatgpt-private"));
    assert!(!debug.contains("user-private"));
    assert!(debug.contains("pro"));
}

#[test]
fn plaintext_provider_schema_round_trips_dynamic_cookie_data() {
    let data = CodexCredentialData::OAuth(CodexOAuthCredentialData {
        schema_version: 1,
        principal: Some(CodexCredentialPrincipal {
            oauth_subject: "subject-private".to_owned(),
            poid: Some("poid-private".to_owned()),
        }),
        installation_id: "00000000-0000-4000-8000-000000000001".to_owned(),
        access_token: "at".to_owned(),
        refresh_token: Some("rt".to_owned()),
        id_token: None,
        oauth_client_id: Some("client".to_owned()),
        oauth_scope: Some("openid profile".to_owned()),
        cookies: vec![CodexCookie {
            name: "oai-did".to_owned(),
            value: "cookie-private".to_owned(),
            domain: "chatgpt.com".to_owned(),
            path: "/".to_owned(),
            host_only: false,
            secure: true,
            expires_at: None,
        }],
    });
    let encoded = serde_json::to_value(&data).expect("serialize provider JSON");
    let decoded: CodexCredentialData =
        serde_json::from_value(encoded).expect("deserialize provider JSON");
    assert_eq!(decoded.oauth().expect("OAuth data").schema_version, 1);
    assert_eq!(decoded.cookies()[0].name, "oai-did");
    assert!(!format!("{decoded:?}").contains("cookie-private"));
}

#[test]
fn provider_schema_rejects_unknown_public_layer_fields() {
    let value = serde_json::json!({
        "schema_version": 1,
        "principal": {"oauth_subject": "subject-private", "poid": null},
        "installation_id": "00000000-0000-4000-8000-000000000001",
        "access_token": "at",
        "cookies": [],
        "unknown_field": 9
    });
    assert!(serde_json::from_value::<CodexCredentialData>(value).is_err());
    assert!(
        serde_json::from_value::<CodexCredentialData>(serde_json::json!({
            "schema_version": 1,
            "access_token": "at",
            "cookies": []
        }))
        .is_err()
    );
}

#[test]
fn state_retention_allows_auth_renewal_but_requires_the_same_verified_owner() {
    use gateway_core::account::{PlaintextCredential, ProviderDeviceCodec};
    use provider_openai::credential::CodexDeviceCodec;
    use serde_json::json;

    let old = json!({
        "schema_version": 1,
        "principal": {"oauth_subject": "subject-test", "poid": "workspace-test"},
        "installation_id": "00000000-0000-4000-8000-000000000001",
        "access_token": "synthetic-old",
        "refresh_token": "synthetic-refresh",
        "id_token": "synthetic-id",
        "oauth_client_id": "client-test",
        "oauth_scope": "openid profile",
        "cookies": []
    });
    let material =
        |value: &serde_json::Value| PlaintextCredential::new(value.as_object().unwrap().clone());
    let mut renewed = old.clone();
    renewed["access_token"] = json!("synthetic-new");
    renewed["refresh_token"] = serde_json::Value::Null;
    renewed["id_token"] = json!("synthetic-new-id");
    let codec = CodexDeviceCodec;
    assert!(codec.can_retain_turn_state(&material(&old), &material(&old)));
    assert!(codec.can_retain_turn_state(&material(&old), &material(&renewed)));
    for (key, value) in [
        ("principal", serde_json::Value::Null),
        (
            "principal",
            json!({"oauth_subject": "other", "poid": "workspace-test"}),
        ),
        (
            "principal",
            json!({"oauth_subject": "subject-test", "poid": "other"}),
        ),
        (
            "installation_id",
            json!("00000000-0000-4000-8000-000000000002"),
        ),
        ("oauth_client_id", json!("other-client")),
        ("oauth_scope", json!("other-scope")),
    ] {
        let mut changed = renewed.clone();
        changed[key] = value;
        assert!(
            !codec.can_retain_turn_state(&material(&old), &material(&changed)),
            "{key}"
        );
    }
    for principal in [
        serde_json::Value::Null,
        json!({"oauth_subject": "", "poid": null}),
    ] {
        let mut unresolved = old.clone();
        unresolved["principal"] = principal;
        assert!(!codec.can_retain_turn_state(&material(&unresolved), &material(&unresolved)));
    }
    let malformed = PlaintextCredential::new(Default::default());
    assert!(!codec.can_retain_turn_state(&malformed, &material(&renewed)));
}
