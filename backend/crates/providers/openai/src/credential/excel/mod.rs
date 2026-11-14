//! Independent BPS authorization and credential boundary.

mod catalog;
mod codec;
mod oauth;
mod service;

pub(crate) use catalog::request_headers;
pub use catalog::{ExcelModel, ExcelModelCatalog};
pub use codec::{ExcelCredentialCodec, ExcelCredentialData, ExcelIdentity};
pub use oauth::{ExcelOAuthFlow, ExcelTokenClient};
pub(crate) use service::ExcelCredentialService;

pub const EXCEL_CLIENT_ID: &str = "app_fnr0pYvVwwFDocDumLG3H2Bp";
pub const EXCEL_CALLBACK_URI: &str = "https://bps.openai.com/basispoints/extension/360590d7-f8f9-4d88-bf75-0edfe0a4b9f3/auth/callback";
pub const EXCEL_AUTHORIZATION_ENDPOINT: &str = "https://auth.openai.com/api/accounts/authorize";
pub const EXCEL_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
pub const EXCEL_AUTHORIZATION_SCOPE: &str = "openid offline_access email profile organization.read";
pub const EXCEL_REFRESH_SCOPE: &str = "openid profile email";
pub const EXCEL_ACCESS_URL: &str =
    "https://bps.openai.com/basispoints/api/responses/access?include_models=true";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExcelAuthError {
    #[error("Excel credential is invalid")]
    InvalidCredential,
    #[error("Independent Excel authorization is required")]
    RequiresAuthorization,
    #[error("Excel credential identity does not match the selected account and workspace")]
    IdentityMismatch,
    #[error("Excel credential has expired or expires too soon")]
    Expired,
    #[error("Excel authorization callback is invalid or expired")]
    InvalidCallback,
    #[error("Excel authorization was rejected; authorize again")]
    ExchangeRejected,
    #[error("Excel token exchange outcome is uncertain; authorize again")]
    ExchangeUncertain,
    #[error("Excel refresh token was not rotated; authorize again")]
    RotationMissing,
    #[error("Excel model catalog is invalid")]
    InvalidCatalog,
    #[error("Excel access is unavailable for this account")]
    AccessDenied,
    #[error("Excel transport is unavailable")]
    Transport,
}

pub(crate) fn identity_from_access_token(token: &str) -> Option<ExcelIdentity> {
    codec::access_claims(token)
        .ok()
        .map(|claims| claims.identity)
}
