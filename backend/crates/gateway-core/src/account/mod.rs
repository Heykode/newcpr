//! Provider 账号领域、持久化端口与同一 target 内的账号选择。

mod error;
mod fast_mode;
pub use fast_mode::FastMode;
mod model;
mod model_access;
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
pub use store::ProviderAccountStore;
mod location;
pub use location::RequestLocation;
pub mod smart_scheduling;
