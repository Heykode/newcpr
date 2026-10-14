//! Purpose-separated administrator credentials and bounded refresh replay.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use rand_core::{OsRng, RngCore as _};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;

use super::DefaultAuthService;
use crate::{
    model::{
        AdminError, AdminErrorKind,
        auth::{AdminSession, LoginResult, SessionRefreshReplay, SessionRenewal},
    },
    use_case::map_store_error,
};

pub(super) struct Credential<'a> {
    pub id: &'a str,
    secret: &'a str,
    pub refresh: bool,
}

impl<'a> Credential<'a> {
    pub fn parse(raw: &'a str) -> Option<Self> {
        let mut parts = raw.split('.');
        let refresh = match parts.next()? {
            "a2" => false,
            "r2" => true,
            _ => return None,
        };
        let id = parts.next()?;
        let secret = parts.next()?;
        if parts.next().is_some() || !valid_secret(id) || !valid_secret(secret) {
            return None;
        }
        Some(Self {
            id,
            secret,
            refresh,
        })
    }

    pub fn owns(&self, renewal: &SessionRenewal) -> bool {
        let digest = digest(self.secret);
        if self.refresh {
            equal(&renewal.refresh_digest, &digest)
                || renewal
                    .previous
                    .as_ref()
                    .is_some_and(|old| equal(&old.digest, &digest))
        } else {
            equal(&renewal.access_digest, &digest)
        }
    }
}

fn valid_secret(value: &str) -> bool {
    value.len() == 43
        && URL_SAFE_NO_PAD
            .decode(value)
            .is_ok_and(|bytes| bytes.len() == 32)
}
pub(super) fn storage_id(raw: &str) -> &str {
    Credential::parse(raw).map_or(raw, |credential| credential.id)
}
fn equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    bool::from(left.ct_eq(right))
}
fn digest(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}
fn random_bytes() -> [u8; 32] {
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes
}
fn derive(secret: &str, salt: Option<&[u8]>, purpose: &[u8]) -> Result<String, AdminError> {
    let mut output = [0; 32];
    hkdf::Hkdf::<Sha256>::new(salt, secret.as_bytes())
        .expand(purpose, &mut output)
        .map_err(|_| AdminError::internal("Session credential derivation failed"))?;
    Ok(URL_SAFE_NO_PAD.encode(output))
}
fn access_secret(refresh: &str) -> Result<String, AdminError> {
    derive(refresh, None, b"cpr-session-v2/access")
}
fn grant(id: &str, refresh: &str, session: &AdminSession) -> Result<LoginResult, AdminError> {
    Ok(LoginResult {
        session_id: format!("a2.{id}.{}", access_secret(refresh)?),
        expires_at: session.expires_at,
        refresh_token: Some(format!("r2.{id}.{refresh}")),
        refresh_expires_at: Some(session.retained_until()),
    })
}
pub(super) fn issue(
    mut session: AdminSession,
    refresh_expiry: chrono::DateTime<Utc>,
) -> Result<(String, AdminSession, LoginResult), AdminError> {
    let id = URL_SAFE_NO_PAD.encode(random_bytes());
    let secret = URL_SAFE_NO_PAD.encode(random_bytes());
    session.renewal = Some(SessionRenewal {
        expires_at: refresh_expiry,
        access_digest: digest(&access_secret(&secret)?),
        refresh_digest: digest(&secret),
        previous: None,
    });
    let result = grant(&id, &secret, &session)?;
    Ok((id, session, result))
}

impl DefaultAuthService {
    pub(super) async fn refresh(&self, raw: &str) -> Result<Option<LoginResult>, AdminError> {
        let Some(credential) = Credential::parse(raw).filter(|value| value.refresh) else {
            return Ok(None);
        };
        let mut current = self.load_live_session(credential.id).await?;
        for _ in 0..3 {
            let Some(session) = current else {
                return Ok(None);
            };
            let Some(renewal) = &session.renewal else {
                return Ok(None);
            };
            let now = Utc::now();
            if session.retained_until() <= now
                || session.absolute_expires_at.is_none_or(|limit| limit <= now)
            {
                return Ok(None);
            }
            if equal(&renewal.refresh_digest, &digest(credential.secret)) {
                if session.expires_at > now {
                    return grant(credential.id, credential.secret, &session).map(Some);
                }
                let absolute = session
                    .absolute_expires_at
                    .ok_or_else(|| AdminError::internal("Missing session lifetime"))?;
                let salt = random_bytes();
                let next = derive(credential.secret, Some(&salt), b"cpr-session-v2/refresh")?;
                let refresh_expiry = (now + self.session_ttl).min(absolute);
                let access_expiry = (now + self.access_ttl()).min(refresh_expiry);
                let replacement = AdminSession {
                    expires_at: access_expiry,
                    renewal: Some(SessionRenewal {
                        expires_at: refresh_expiry,
                        access_digest: digest(&access_secret(&next)?),
                        refresh_digest: digest(&next),
                        previous: Some(SessionRefreshReplay {
                            digest: digest(credential.secret),
                            salt,
                            expires_at: (now + Duration::seconds(30)).min(access_expiry),
                        }),
                    }),
                    ..session.clone()
                };
                current = self
                    .store
                    .compare_exchange_session(credential.id, &session, Some(&replacement))
                    .await
                    .map_err(|error| map_store_error(error, "administrator session rotation"))?;
                if current.as_ref() == Some(&replacement) {
                    return grant(credential.id, &next, &replacement).map(Some);
                }
            } else if let Some(previous) = renewal
                .previous
                .as_ref()
                .filter(|old| equal(&old.digest, &digest(credential.secret)))
            {
                if previous.expires_at > now {
                    let next = derive(
                        credential.secret,
                        Some(&previous.salt),
                        b"cpr-session-v2/refresh",
                    )?;
                    return grant(credential.id, &next, &session).map(Some);
                }
                current = self
                    .store
                    .compare_exchange_session(credential.id, &session, None)
                    .await
                    .map_err(|error| map_store_error(error, "administrator session revocation"))?;
            } else {
                return Ok(None);
            }
        }
        Err(AdminError::new(
            AdminErrorKind::Unavailable,
            "Session is changing; retry",
        ))
    }
}
