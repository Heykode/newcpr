use std::fmt;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use gateway_core::account::PlaintextCredential;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{EXCEL_CLIENT_ID, ExcelAuthError};
use crate::credential::{CodexOAuthSecret, CodexRuntimeAuthentication, CodexRuntimeCredential};

const MAX_CREDENTIAL_BYTES: usize = 256 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcelIdentity {
    pub account_id: String,
    pub user_id: String,
}

impl fmt::Debug for ExcelIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExcelIdentity([REDACTED])")
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ExcelCredentialData {
    pub schema_version: u32,
    #[serde(rename = "type")]
    pub credential_type: String,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    pub chatgpt_account_id: String,
    pub user_id: String,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
}

impl fmt::Debug for ExcelCredentialData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExcelCredentialData")
            .field("credential_type", &self.credential_type)
            .field("expires_at", &self.expires_at)
            .field("material", &"[REDACTED]")
            .finish()
    }
}

impl ExcelCredentialData {
    pub fn identity(&self) -> ExcelIdentity {
        ExcelIdentity {
            account_id: self.chatgpt_account_id.clone(),
            user_id: self.user_id.clone(),
        }
    }

    pub fn matches_identity(&self, identity: &ExcelIdentity) -> Result<(), ExcelAuthError> {
        if self.chatgpt_account_id != identity.account_id || self.user_id != identity.user_id {
            return Err(ExcelAuthError::IdentityMismatch);
        }
        Ok(())
    }

    /// Reuses the transport carrier only; Excel material never enters the Codex codec.
    pub fn runtime(&self) -> CodexRuntimeCredential {
        CodexRuntimeCredential {
            authentication: CodexRuntimeAuthentication::OAuth(CodexOAuthSecret {
                access_token: SecretString::from(self.access_token.clone()),
                refresh_token: self.refresh_token.clone().map(SecretString::from),
                id_token: self.id_token.clone().map(SecretString::from),
            }),
            principal: None,
            installation_id: String::new(),
            cookies: Vec::new(),
            oauth_client_id: Some(EXCEL_CLIENT_ID.into()),
            oauth_scope: None,
        }
    }

    pub fn ensure_usable(&self) -> Result<(), ExcelAuthError> {
        if self
            .expires_at
            .signed_duration_since(Utc::now())
            .num_seconds()
            < 30
        {
            return Err(ExcelAuthError::Expired);
        }
        Ok(())
    }

    pub fn export_document(&self) -> Result<Map<String, Value>, ExcelAuthError> {
        serde_json::to_value(self)
            .map_err(|_| ExcelAuthError::InvalidCredential)?
            .as_object()
            .cloned()
            .ok_or(ExcelAuthError::InvalidCredential)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ExcelCredentialCodec;

impl ExcelCredentialCodec {
    pub fn import_document(
        document: &Map<String, Value>,
    ) -> Result<ExcelCredentialData, ExcelAuthError> {
        check_size(document)?;
        if let Some(kind) = document.get("type")
            && !matches!(kind.as_str(), Some("cpa-bps" | "excel"))
        {
            return Err(ExcelAuthError::RequiresAuthorization);
        }
        let token = document
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or(ExcelAuthError::InvalidCredential)?;
        let claims = access_claims(token)?;
        for (key, expected) in [
            ("chatgpt_account_id", &claims.identity.account_id),
            ("account_id", &claims.identity.account_id),
            ("user_id", &claims.identity.user_id),
        ] {
            if let Some(value) = document.get(key).filter(|value| !value.is_null())
                && value.as_str().is_none_or(|value| value != expected)
            {
                return Err(ExcelAuthError::IdentityMismatch);
            }
        }
        let supplied_expiry = document
            .get("expires_at")
            .filter(|value| !value.is_null())
            .map(|value| {
                serde_json::from_value::<DateTime<Utc>>(value.clone())
                    .map_err(|_| ExcelAuthError::InvalidCredential)
            })
            .transpose()?;
        let id_token = optional_secret(document, "id_token")?;
        if let Some(token) = &id_token {
            let identity = jwt_payload(token)?;
            for (key, expected) in [
                ("chatgpt_account_id", &claims.identity.account_id),
                ("chatgpt_user_id", &claims.identity.user_id),
            ] {
                if identity["https://api.openai.com/auth"]
                    .get(key)
                    .is_some_and(|value| value.as_str() != Some(expected))
                {
                    return Err(ExcelAuthError::IdentityMismatch);
                }
            }
        }
        let email = claims
            .email
            .or_else(|| safe_email(document.get("email").and_then(Value::as_str)))
            .or_else(|| {
                id_token
                    .as_deref()
                    .and_then(|token| jwt_payload(token).ok())
                    .and_then(|value| safe_email(value.get("email").and_then(Value::as_str)))
            });
        Ok(ExcelCredentialData {
            schema_version: 1,
            credential_type: "cpa-bps".into(),
            access_token: token.into(),
            refresh_token: optional_secret(document, "refresh_token")?,
            id_token,
            chatgpt_account_id: claims.identity.account_id,
            user_id: claims.identity.user_id,
            expires_at: supplied_expiry
                .map_or(claims.expires_at, |expiry| expiry.min(claims.expires_at)),
            email,
            plan_type: claims.plan_type,
        })
    }

    pub fn encode(data: &ExcelCredentialData) -> Result<PlaintextCredential, ExcelAuthError> {
        if data.schema_version != 1 || data.credential_type != "cpa-bps" {
            return Err(ExcelAuthError::InvalidCredential);
        }
        let normalized = Self::import_document(&data.export_document()?)?;
        Ok(PlaintextCredential::new(normalized.export_document()?))
    }

    pub fn decode(material: &PlaintextCredential) -> Result<ExcelCredentialData, ExcelAuthError> {
        let document = material.expose_to_provider();
        if document.get("schema_version").and_then(Value::as_u64) != Some(1)
            || document.get("type").and_then(Value::as_str) != Some("cpa-bps")
        {
            return Err(ExcelAuthError::InvalidCredential);
        }
        Self::import_document(document)
    }
}

pub(super) struct AccessClaims {
    pub(super) identity: ExcelIdentity,
    pub(super) email: Option<String>,
    pub(super) plan_type: Option<String>,
    pub(super) expires_at: DateTime<Utc>,
}

pub(super) fn access_claims(token: &str) -> Result<AccessClaims, ExcelAuthError> {
    let claims = jwt_payload(token)?;
    let client = nonempty(&claims["client_id"]).or_else(|| nonempty(&claims["azp"]));
    if client != Some(EXCEL_CLIENT_ID) {
        return Err(ExcelAuthError::RequiresAuthorization);
    }
    let auth = &claims["https://api.openai.com/auth"];
    let account_id =
        nonempty(&auth["chatgpt_account_id"]).ok_or(ExcelAuthError::RequiresAuthorization)?;
    let user_id = nonempty(&auth["chatgpt_user_id"])
        .or_else(|| nonempty(&auth["user_id"]))
        .ok_or(ExcelAuthError::RequiresAuthorization)?;
    if !safe_identity(account_id) || !safe_identity(user_id) {
        return Err(ExcelAuthError::InvalidCredential);
    }
    let expires_at = claims["exp"]
        .as_i64()
        .filter(|value| *value > 0)
        .and_then(|value| DateTime::from_timestamp(value, 0))
        .ok_or(ExcelAuthError::RequiresAuthorization)?;
    Ok(AccessClaims {
        identity: ExcelIdentity {
            account_id: account_id.into(),
            user_id: user_id.into(),
        },
        email: safe_email(claims["https://api.openai.com/profile"]["email"].as_str())
            .or_else(|| safe_email(claims["email"].as_str())),
        plan_type: nonempty(&auth["chatgpt_plan_type"]).map(str::to_owned),
        expires_at,
    })
}

fn jwt_payload(token: &str) -> Result<Value, ExcelAuthError> {
    if token.is_empty() || token.len() > 65_536 || token.chars().any(char::is_whitespace) {
        return Err(ExcelAuthError::InvalidCredential);
    }
    let mut parts = token.split('.');
    let header = parts.next().unwrap_or_default();
    let payload = parts.next().ok_or(ExcelAuthError::InvalidCredential)?;
    let signature = parts.next().ok_or(ExcelAuthError::InvalidCredential)?;
    if header.is_empty() || signature.is_empty() || parts.next().is_some() {
        return Err(ExcelAuthError::InvalidCredential);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| ExcelAuthError::InvalidCredential)?;
    // Identity projection of issuer-delivered material, not JWT signature verification.
    serde_json::from_slice(&bytes).map_err(|_| ExcelAuthError::InvalidCredential)
}

fn optional_secret(
    document: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, ExcelAuthError> {
    match document.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Ok(None),
        Some(Value::String(value))
            if value.len() <= 65_536 && !value.chars().any(char::is_whitespace) =>
        {
            Ok(Some(value.clone()))
        }
        _ => Err(ExcelAuthError::InvalidCredential),
    }
}

fn check_size(document: &Map<String, Value>) -> Result<(), ExcelAuthError> {
    if serde_json::to_vec(document)
        .map_err(|_| ExcelAuthError::InvalidCredential)?
        .len()
        > MAX_CREDENTIAL_BYTES
    {
        return Err(ExcelAuthError::InvalidCredential);
    }
    Ok(())
}

fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| !value.is_empty())
}
fn safe_identity(value: &str) -> bool {
    value.len() <= 512
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control)
}
fn safe_email(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| {
            value.len() <= 320
                && value.contains('@')
                && !value.chars().any(char::is_whitespace)
                && !value.chars().any(char::is_control)
        })
        .map(str::to_owned)
}
