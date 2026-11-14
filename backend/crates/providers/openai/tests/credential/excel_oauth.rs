use super::*;
use crate::credential::excel::{ExcelModelCatalog, request_headers};
use serde_json::{Map, Value};

fn jwt(value: Value) -> String {
    format!(
        "fixture.{}.unsigned",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap())
    )
}

fn document(client: &str, workspace: &str, user: &str) -> Map<String, Value> {
    json!({"type":"cpa-bps", "access_token": jwt(json!({
        "client_id":client,"exp":(Utc::now()+TimeDelta::hours(1)).timestamp(),
        "https://api.openai.com/auth":{"chatgpt_account_id":workspace,"chatgpt_user_id":user,"chatgpt_plan_type":"plus"},
        "https://api.openai.com/profile":{"email":"fixture@example.invalid"}
    })),"refresh_token":"fixture-refresh"}).as_object().unwrap().clone()
}

#[test]
fn excel_rejects_codex_client_and_conflicting_identity() {
    assert_eq!(
        ExcelCredentialCodec::import_document(&document("codex-client", "workspace-a", "user-a"))
            .unwrap_err(),
        ExcelAuthError::RequiresAuthorization
    );
    let mut doc = document(EXCEL_CLIENT_ID, "workspace-a", "user-a");
    doc.insert("account_id".into(), json!("workspace-b"));
    assert_eq!(
        ExcelCredentialCodec::import_document(&doc).unwrap_err(),
        ExcelAuthError::IdentityMismatch
    );
    doc.insert("account_id".into(), json!(42));
    assert_eq!(
        ExcelCredentialCodec::import_document(&doc).unwrap_err(),
        ExcelAuthError::IdentityMismatch
    );
}

#[test]
fn excel_material_roundtrips_without_codex_device_or_cookie_state() {
    let data =
        ExcelCredentialCodec::import_document(&document(EXCEL_CLIENT_ID, "workspace-a", "user-a"))
            .unwrap();
    let material = ExcelCredentialCodec::encode(&data).unwrap();
    let decoded = ExcelCredentialCodec::decode(&material).unwrap();
    assert_eq!(decoded.identity(), data.identity());
    assert!(decoded.runtime().cookies.is_empty());
    assert!(decoded.runtime().installation_id.is_empty());
    assert_eq!(
        decoded.runtime().oauth_client_id.as_deref(),
        Some(EXCEL_CLIENT_ID)
    );
    assert!(!format!("{data:?}").contains(&data.access_token));
}

#[test]
fn excel_access_and_identity_token_must_agree() {
    let mut doc = document(EXCEL_CLIENT_ID, "workspace-a", "user-a");
    doc.insert(
        "id_token".into(),
        json!(jwt(
            json!({"https://api.openai.com/auth":{"chatgpt_account_id":"workspace-b"}})
        )),
    );
    assert_eq!(
        ExcelCredentialCodec::import_document(&doc).unwrap_err(),
        ExcelAuthError::IdentityMismatch
    );
}

#[test]
fn supplied_expiration_cannot_extend_token_lifetime() {
    let mut doc = document(EXCEL_CLIENT_ID, "workspace-a", "user-a");
    let original = ExcelCredentialCodec::import_document(&doc).unwrap();
    doc.insert("expires_at".into(), json!(Utc::now() + TimeDelta::days(1)));
    assert_eq!(
        ExcelCredentialCodec::import_document(&doc)
            .unwrap()
            .expires_at,
        original.expires_at
    );
    doc.insert(
        "expires_at".into(),
        json!(Utc::now() - TimeDelta::seconds(1)),
    );
    assert_eq!(
        ExcelCredentialCodec::import_document(&doc)
            .unwrap()
            .ensure_usable(),
        Err(ExcelAuthError::Expired)
    );
}

#[test]
fn authorization_uses_independent_bps_client_callback_and_scope() {
    let flow = ExcelOAuthFlow::start().unwrap();
    let url = Url::parse(&flow.authorization_url).unwrap();
    assert_eq!(
        url.origin().ascii_serialization(),
        "https://auth.openai.com"
    );
    assert_eq!(url.path(), "/api/accounts/authorize");
    let query: std::collections::BTreeMap<_, _> = url
        .query_pairs()
        .map(|(a, b)| (a.into_owned(), b.into_owned()))
        .collect();
    assert_eq!(query.len(), 9);
    assert_eq!(query["client_id"], EXCEL_CLIENT_ID);
    assert_eq!(query["redirect_uri"], EXCEL_CALLBACK_URI);
    assert_eq!(query["scope"], EXCEL_AUTHORIZATION_SCOPE);
    assert_eq!(query["platform"], "PC");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(
        query["code_challenge"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(
            flow.code_verifier.expose_secret().as_bytes()
        ))
    );
    assert_eq!(
        URL_SAFE_NO_PAD
            .decode(flow.state.expose_secret())
            .unwrap()
            .len(),
        32
    );
    assert_eq!(
        URL_SAFE_NO_PAD
            .decode(flow.code_verifier.expose_secret())
            .unwrap()
            .len(),
        32
    );
}

#[test]
fn callback_requires_exact_origin_state_and_single_code() {
    let flow = ExcelOAuthFlow::start().unwrap();
    let callback = format!(
        "{EXCEL_CALLBACK_URI}?code=fixture-code&state={}",
        flow.state.expose_secret()
    );
    assert_eq!(
        flow.callback_code(&callback).unwrap().expose_secret(),
        "fixture-code"
    );
    for invalid in [
        format!("{callback}&code=other"),
        format!("{callback}&state=other"),
        format!("{callback}#fragment"),
        callback.replace("bps.openai.com", "example.invalid"),
        format!("{callback}&error=denied"),
    ] {
        assert_eq!(
            flow.callback_code(&invalid).unwrap_err(),
            ExcelAuthError::InvalidCallback
        );
    }
}

#[test]
fn token_response_rejects_wrong_type_and_invalid_expiry() {
    let doc = document(EXCEL_CLIENT_ID, "workspace-a", "user-a");
    for (token_type, expires_in) in [("Basic", 3600), ("Bearer", 0), ("Bearer", -1)] {
        let response = json!({"access_token":doc["access_token"],"token_type":token_type,"expires_in":expires_in});
        assert_eq!(
            ExcelTokenClient::parse_response(&serde_json::to_vec(&response).unwrap(), None)
                .unwrap_err(),
            ExcelAuthError::InvalidCredential
        );
    }
}

#[tokio::test]
async fn token_exchange_is_one_form_post_and_never_retries_rejection() {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string_contains, header, method},
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(header("accept", "application/json"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(429))
        .expect(1)
        .mount(&server)
        .await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        ExcelTokenClient::send(
            &client,
            &server.uri(),
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", "fixture-refresh")
            ]
        )
        .await
        .unwrap_err(),
        ExcelAuthError::ExchangeRejected
    );
}

#[test]
fn headers_use_bps_identity_without_office_or_stainless_fingerprints() {
    let headers = request_headers(
        "Bearer fixture",
        "workspace-a",
        Some("user-a"),
        "text/event-stream",
    )
    .unwrap();
    assert_eq!(headers["user-agent"], "Mozilla/5.0");
    assert_eq!(headers["chatgpt-account-id"], "workspace-a");
    assert_eq!(headers["x-openai-account-id"], "workspace-a");
    assert_eq!(headers["x-openai-account-user-id"], "user-a");
    assert_eq!(headers["x-basispoints-auth-mode"], "chatgpt");
    assert_eq!(
        headers["x-openai-internal-basispoints-client-product"],
        "basispoints-excel-plugin"
    );
    assert!(headers["authorization"].is_sensitive());
    assert!(
        !headers
            .keys()
            .any(|key| key.as_str().starts_with("x-stainless")
                || key.as_str().starts_with("x-office"))
    );
}

#[test]
fn model_catalog_filters_restrictions_and_does_not_invent_models() {
    let body = json!({"allowed":true,"model_catalog":{"models":[
        {"id":"gpt-example","label":"Example","model_picker_enabled":true,"policy":{"state":"enabled"},"efforts":[{"value":"high"},{"value":"invalid"}]},
        {"id":"gpt-restricted"},{"id":"gpt-disabled","policy":{"state":"disabled"}},
        {"id":"gpt-hidden","model_picker_enabled":false},{"id":"gpt-example-excel"},{"id":"other-model"},
        {"id":"gpt-example"}],"restricted_models":["gpt-restricted"]}});
    let models = ExcelModelCatalog::parse(&serde_json::to_vec(&body).unwrap()).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-example");
    assert_eq!(models[0].efforts, vec!["high"]);
    assert!(
        ExcelModelCatalog::parse(br#"{"allowed":false}"#)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        ExcelModelCatalog::parse(br#"{"allowed":true}"#).unwrap_err(),
        ExcelAuthError::InvalidCatalog
    );
    assert_eq!(
        ExcelModelCatalog::parse(br#"{"model_catalog":{"models":[]}}"#).unwrap_err(),
        ExcelAuthError::InvalidCatalog
    );
}
