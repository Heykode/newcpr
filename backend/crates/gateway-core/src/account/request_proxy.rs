//! Account-owned request exit selection, independent of Codex/Excel routing.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestProxySource {
    #[default]
    Account,
    Mihomo,
    ProxyPool,
}

impl RequestProxySource {
    #[must_use]
    pub fn supports_provider(self, provider: &str) -> bool {
        self == Self::Account || provider == "openai"
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Mihomo => "mihomo",
            Self::ProxyPool => "proxy_pool",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "account" => Some(Self::Account),
            "mihomo" => Some(Self::Mihomo),
            "proxy_pool" => Some(Self::ProxyPool),
            _ => None,
        }
    }
}
