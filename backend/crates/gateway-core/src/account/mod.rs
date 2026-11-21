//! Provider 账号领域、持久化端口与同一 target 内的账号选择。

mod error;
mod fast_mode;
pub use fast_mode::FastMode;
mod model;
mod model_access;
mod quota_settings;
mod request_proxy;
mod responses_upstream;
pub use model_access::{
    AccountModelAccess, AccountModelAccessMode, InvalidAccountModelAccess,
    MAX_ACCOUNT_ACCESS_MODELS,
};
pub use responses_upstream::{Excel403Action, ExcelModels, ResponsesUpstream};
mod proxy;
pub use proxy::{InvalidOutboundProxy, OutboundProxy};
pub mod scope;
mod selection;
mod store;

pub use error::CredentialError;
pub use model::*;
pub use request_proxy::RequestProxySource;
pub(crate) use selection::smart_score;
pub use selection::*;
pub use store::{
    AccountCatalogObservation, AccountCatalogOmissions, ProviderAccountStore, QuotaWarmupState,
    QuotaWarmupStateUpdate, StoredAccountCatalog,
};
mod location;
pub use location::{InvalidRequestLocation, OpenAiRequestCountry, RequestLocation};
pub mod scheduling;
pub use scheduling::{
    QuotaResetPeriod, ResetWindowWeights, SchedulingConfigError, SmartSchedulingConfig,
};

mod error_protection;
pub use error_protection::*;

pub use quota_settings::{
    AccountQuotaSettings, QuotaBucketSettings, QuotaBucketWarmup, QuotaMergeBase,
    QuotaPauseDecision,
};
