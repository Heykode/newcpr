//! xAI 使用原生账号管理的共享凭据生命周期。
pub(crate) use super::native_credentials::DefaultNativeCredentialService as DefaultXaiService;
pub use super::native_credentials::NativeCredentialService as XaiService;
