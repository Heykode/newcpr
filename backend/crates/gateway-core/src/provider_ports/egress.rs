//! Provider-owned IPv6 egress policy and its runtime storage boundary.

use std::{collections::BTreeMap, net::Ipv6Addr, sync::Arc};

use futures::future::BoxFuture;

use crate::account::ProviderAccountId;

/// How a connection chooses a local IPv6 source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EgressMode {
    /// Do not select an IPv6 source.
    #[default]
    Unchanged,
    FixedIpv6Reuse,
    RandomIpv6Reuse,
    FixedIpv6Fresh,
    RandomIpv6Fresh,
}

#[must_use]
pub fn is_valid_source_address(address: Ipv6Addr) -> bool {
    !address.is_unspecified()
        && !address.is_loopback()
        && !address.is_multicast()
        && !address.is_unicast_link_local()
        && address.to_ipv4().is_none()
        && (address.segments()[0] & 0xffc0) != 0xfec0
}

impl EgressMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::FixedIpv6Reuse => "fixed_ipv6_reuse",
            Self::RandomIpv6Reuse => "random_ipv6_reuse",
            Self::FixedIpv6Fresh => "fixed_ipv6_fresh",
            Self::RandomIpv6Fresh => "random_ipv6_fresh",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unchanged" => Some(Self::Unchanged),
            "fixed_ipv6_reuse" => Some(Self::FixedIpv6Reuse),
            "random_ipv6_reuse" => Some(Self::RandomIpv6Reuse),
            "fixed_ipv6_fresh" => Some(Self::FixedIpv6Fresh),
            "random_ipv6_fresh" => Some(Self::RandomIpv6Fresh),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        !matches!(self, Self::Unchanged)
    }

    #[must_use]
    pub const fn is_fresh(self) -> bool {
        matches!(self, Self::FixedIpv6Fresh | Self::RandomIpv6Fresh)
    }

    #[must_use]
    pub const fn is_random(self) -> bool {
        matches!(self, Self::RandomIpv6Reuse | Self::RandomIpv6Fresh)
    }
}

/// A configured address in the ordered provider pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEgressAddress {
    pub id: String,
    pub address: Ipv6Addr,
    pub enabled: bool,
}

/// Immutable provider egress state loaded at runtime snapshot publication.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderEgressConfig {
    pub revision: u64,
    pub default_mode: EgressMode,
    pub addresses: Vec<ProviderEgressAddress>,
    /// Every live account is present. `None` inherits; `Some(Unchanged)` retains its original route.
    pub account_overrides: BTreeMap<ProviderAccountId, Option<EgressMode>>,
    pub fixed_bindings: BTreeMap<ProviderAccountId, Ipv6Addr>,
}

/// Store capability used by runtime publication and transactional affinity hooks.
pub trait ProviderEgressStorePort: Send + Sync {
    fn load(&self) -> BoxFuture<'_, Result<Arc<ProviderEgressConfig>, super::ProviderStoreError>>;

    /// Persist a fixed identity binding without allocating a new address on reads.
    fn ensure_fixed_affinity(
        &self,
        account_id: &ProviderAccountId,
    ) -> BoxFuture<'_, Result<Option<Ipv6Addr>, super::ProviderStoreError>>;
}
