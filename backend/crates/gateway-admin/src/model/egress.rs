//! IPv6 egress control-plane commands and projections.

use gateway_core::{
    account::ProviderAccountId,
    provider_ports::egress::{EgressMode, ProviderEgressAddress, ProviderEgressConfig},
};

use super::Revision;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceProviderEgress {
    pub expected_revision: Revision,
    pub default_mode: EgressMode,
    pub addresses: Vec<ProviderEgressAddress>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetProviderAccountEgress {
    pub account_id: ProviderAccountId,
    pub expected_revision: Revision,
    /// `None` removes the override and inherits the global mode.
    pub mode: Option<EgressMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEgressMutation {
    pub config_revision: Revision,
    pub config: ProviderEgressConfig,
}
