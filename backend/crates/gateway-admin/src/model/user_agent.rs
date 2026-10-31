//! Provider-owned outbound UA previews and durable selection.

use chrono::{DateTime, Utc};
pub use gateway_core::provider_ports::ProviderUserAgentOverride;

#[derive(Debug, Clone, PartialEq)]
pub struct OutboundUserAgentView {
    pub profile_options: Option<gateway_core::account::OpaqueProviderData>,
    pub profile_preview: Option<gateway_core::account::OpaqueProviderData>,
    pub selection: ProviderUserAgentOverride,
    pub default_user_agent: String,
    pub effective_user_agent: String,
    pub effective_desktop_user_agent: String,
    pub core_version: String,
    pub desktop_version: String,
    pub os_type: String,
    pub os_version: String,
    pub arch: String,
    pub terminal: String,
    pub verified: bool,
    pub default_verified_at: DateTime<Utc>,
}
