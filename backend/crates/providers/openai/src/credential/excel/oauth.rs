use std::{fmt, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use futures::StreamExt;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;

use super::{
    EXCEL_AUTHORIZATION_ENDPOINT, EXCEL_AUTHORIZATION_SCOPE, EXCEL_CALLBACK_URI, EXCEL_CLIENT_ID,
    EXCEL_REFRESH_SCOPE, EXCEL_TOKEN_ENDPOINT, ExcelAuthError, ExcelCredentialCodec,
    ExcelCredentialData, ExcelIdentity,
};

/// Persist these secrets in the existing protected OAuth pending store, never logs.
pub struct ExcelOAuthFlow {
    pub state: SecretString,
    pub code_verifier: SecretString,
    pub authorization_url: String,
    pub expires_at: DateTime<Utc>,
}

impl fmt::Debug for ExcelOAuthFlow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExcelOAuthFlow")
            .field("expires_at", &self.expires_at)
            .field("flow", &"[REDACTED]")
            .finish()
    }
}

impl ExcelOAuthFlow {
    pub fn start() -> Result<Self, ExcelAuthError> {
        let mut state = [0_u8; 32];
        let mut verifier = [0_u8; 32];
        getrandom::fill(&mut state).map_err(|_| ExcelAuthError::Transport)?;
        getrandom::fill(&mut verifier).map_err(|_| ExcelAuthError::Transport)?;
        let state = URL_SAFE_NO_PAD.encode(state);
        let verifier = URL_SAFE_NO_PAD.encode(verifier);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url =
            Url::parse(EXCEL_AUTHORIZATION_ENDPOINT).map_err(|_| ExcelAuthError::Transport)?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", EXCEL_CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", EXCEL_CALLBACK_URI),
            ("scope", EXCEL_AUTHORIZATION_SCOPE),
            ("state", state.as_str()),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge.as_str()),
            ("audience", "https://api.openai.com/v1"),
            ("platform", "PC"),
        ]);
        Ok(Self {
            state: state.into(),
            code_verifier: verifier.into(),
            authorization_url: url.into(),
            expires_at: Utc::now() + TimeDelta::minutes(10),
        })
    }

    pub async fn complete(
        self,
        client: &reqwest::Client,
        callback: &SecretString,
        expected: Option<&ExcelIdentity>,
    ) -> Result<ExcelCredentialData, ExcelAuthError> {
        let code = self.callback_code(callback.expose_secret())?;
        ExcelTokenClient::exchange(client, &code, &self.code_verifier, expected).await
    }

    pub(crate) fn callback_code(&self, callback: &str) -> Result<SecretString, ExcelAuthError> {
        if callback.len() > 16 * 1024 || self.expires_at <= Utc::now() {
            return Err(ExcelAuthError::InvalidCallback);
        }
        let url = Url::parse(callback).map_err(|_| ExcelAuthError::InvalidCallback)?;
        let expected =
            Url::parse(EXCEL_CALLBACK_URI).map_err(|_| ExcelAuthError::InvalidCallback)?;
        if url.origin() != expected.origin()
            || url.path() != expected.path()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(ExcelAuthError::InvalidCallback);
        }
        let query: Vec<_> = url.query_pairs().collect();
        let state: Vec<_> = query.iter().filter(|(key, _)| key == "state").collect();
        let code: Vec<_> = query.iter().filter(|(key, _)| key == "code").collect();
        if state.len() != 1
            || code.len() != 1
            || code[0].1.is_empty()
            || code[0].1.len() > 8192
            || query.iter().any(|(key, _)| key == "error")
            || !bool::from(
                state[0]
                    .1
                    .as_bytes()
                    .ct_eq(self.state.expose_secret().as_bytes()),
            )
        {
            return Err(ExcelAuthError::InvalidCallback);
        }
        Ok(SecretString::from(code[0].1.to_string()))
    }
}

pub struct ExcelTokenClient;

impl ExcelTokenClient {
    pub async fn exchange(
        client: &reqwest::Client,
        code: &SecretString,
        verifier: &SecretString,
        expected: Option<&ExcelIdentity>,
    ) -> Result<ExcelCredentialData, ExcelAuthError> {
        let form = [
            ("grant_type", "authorization_code"),
            ("client_id", EXCEL_CLIENT_ID),
            ("code", code.expose_secret()),
            ("redirect_uri", EXCEL_CALLBACK_URI),
            ("code_verifier", verifier.expose_secret()),
        ];
        let body = Self::send(client, EXCEL_TOKEN_ENDPOINT, &form).await?;
        let data = Self::parse_response(&body, None)?;
        if let Some(identity) = expected {
            data.matches_identity(identity)?;
        }
        Ok(data)
    }

    /// Call only while holding the independent Excel refresh claim. Never retry an uncertain exchange.
    pub async fn refresh(
        client: &reqwest::Client,
        previous: &ExcelCredentialData,
    ) -> Result<ExcelCredentialData, ExcelAuthError> {
        if previous.expires_at > Utc::now() + TimeDelta::seconds(300) {
            return Ok(previous.clone());
        }
        let refresh = previous
            .refresh_token
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(ExcelAuthError::RequiresAuthorization)?;
        let form = [
            ("grant_type", "refresh_token"),
            ("client_id", EXCEL_CLIENT_ID),
            ("refresh_token", refresh),
            ("scope", EXCEL_REFRESH_SCOPE),
        ];
        let body = Self::send(client, EXCEL_TOKEN_ENDPOINT, &form).await?;
        let data = Self::parse_response(&body, Some(previous))?;
        data.matches_identity(&previous.identity())?;
        if data.refresh_token.as_deref() == Some(refresh) || data.refresh_token.is_none() {
            return Err(ExcelAuthError::RotationMissing);
        }
        Ok(data)
    }

    async fn send(
        client: &reqwest::Client,
        endpoint: &str,
        form: &[(&str, &str)],
    ) -> Result<Vec<u8>, ExcelAuthError> {
        let response = client
            .post(endpoint)
            .header("accept", "application/json")
            .timeout(Duration::from_secs(45))
            .form(form)
            .send()
            .await
            .map_err(|_| ExcelAuthError::ExchangeUncertain)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(ExcelAuthError::ExchangeRejected);
        }
        read_bounded(response, 64 * 1024)
            .await
            .map_err(|_| ExcelAuthError::ExchangeUncertain)
    }

    fn parse_response(
        body: &[u8],
        previous: Option<&ExcelCredentialData>,
    ) -> Result<ExcelCredentialData, ExcelAuthError> {
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
            #[serde(default)]
            id_token: Option<String>,
            #[serde(default)]
            expires_in: Option<i64>,
            token_type: String,
        }
        let response: TokenResponse =
            serde_json::from_slice(body).map_err(|_| ExcelAuthError::InvalidCredential)?;
        if !response.token_type.eq_ignore_ascii_case("Bearer") {
            return Err(ExcelAuthError::InvalidCredential);
        }
        if response
            .expires_in
            .is_some_and(|value| !(1..31_536_000).contains(&value))
        {
            return Err(ExcelAuthError::InvalidCredential);
        }
        let expiry = response
            .expires_in
            .filter(|value| (1..31_536_000).contains(value))
            .map(|seconds| Utc::now() + TimeDelta::seconds(seconds));
        let document = json!({
            "type": "cpa-bps", "access_token": response.access_token,
            "refresh_token": response.refresh_token.filter(|value| !value.is_empty()).or_else(|| previous.and_then(|value| value.refresh_token.clone())),
            "id_token": response.id_token,
            "expires_at": expiry,
            "chatgpt_account_id": previous.map(|value| value.chatgpt_account_id.as_str()),
            "user_id": previous.map(|value| value.user_id.as_str()),
            "email": previous.and_then(|value| value.email.as_deref()),
        });
        let data = ExcelCredentialCodec::import_document(
            document
                .as_object()
                .ok_or(ExcelAuthError::InvalidCredential)?,
        )?;
        data.ensure_usable()?;
        Ok(data)
    }
}

pub(super) async fn read_bounded(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ExcelAuthError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(ExcelAuthError::Transport);
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ExcelAuthError::Transport)?;
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > limit)
        {
            return Err(ExcelAuthError::Transport);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
#[path = "../../../tests/credential/excel_oauth.rs"]
mod tests;
