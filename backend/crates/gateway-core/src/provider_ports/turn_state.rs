//! Provider-owned opaque turn-state storage boundary.

use std::{
    fmt,
    time::{Duration, SystemTime},
};

use futures::future::BoxFuture;

use crate::{
    account::{CredentialRevision, ProviderAccountId},
    routing::UpstreamModelId,
};

use super::ProviderStoreError;

/// Opaque state returned by an upstream Provider. The plaintext is never included in Debug output.
#[derive(Clone, PartialEq, Eq)]
pub struct OpaqueTurnState(String);

impl OpaqueTurnState {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn expose_to_provider(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OpaqueTurnState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpaqueTurnState([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTurnStateValue {
    state: OpaqueTurnState,
    /// Provider-local capture clock; the historical field name is not an upstream claim.
    issued_at: SystemTime,
    expires_at: SystemTime,
}

impl ProviderTurnStateValue {
    #[must_use]
    pub const fn new(
        state: OpaqueTurnState,
        issued_at: SystemTime,
        expires_at: SystemTime,
    ) -> Self {
        Self {
            state,
            issued_at,
            expires_at,
        }
    }

    #[must_use]
    pub const fn state(&self) -> &OpaqueTurnState {
        &self.state
    }

    #[must_use]
    pub const fn issued_at(&self) -> SystemTime {
        self.issued_at
    }

    #[must_use]
    pub const fn expires_at(&self) -> SystemTime {
        self.expires_at
    }

    #[must_use]
    pub fn is_valid_at(&self, observed_at: SystemTime) -> bool {
        self.issued_at <= observed_at && self.expires_at > observed_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTurnStateRefreshStatus {
    Missing,
    Ready,
    Refreshing,
    Failed,
}

impl ProviderTurnStateRefreshStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Ready => "ready",
            Self::Refreshing => "refreshing",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "missing" => Some(Self::Missing),
            "ready" => Some(Self::Ready),
            "refreshing" => Some(Self::Refreshing),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTurnStateRecord {
    account_id: ProviderAccountId,
    upstream_model: UpstreamModelId,
    normal_length: u16,
    active: Option<ProviderTurnStateValue>,
    standby: Option<ProviderTurnStateValue>,
    state_version: u64,
    refresh_status: ProviderTurnStateRefreshStatus,
    last_observed_length: Option<u16>,
}

impl ProviderTurnStateRecord {
    #[must_use]
    #[expect(clippy::too_many_arguments)]
    pub const fn new(
        account_id: ProviderAccountId,
        upstream_model: UpstreamModelId,
        normal_length: u16,
        active: Option<ProviderTurnStateValue>,
        standby: Option<ProviderTurnStateValue>,
        state_version: u64,
        refresh_status: ProviderTurnStateRefreshStatus,
        last_observed_length: Option<u16>,
    ) -> Self {
        Self {
            account_id,
            upstream_model,
            normal_length,
            active,
            standby,
            state_version,
            refresh_status,
            last_observed_length,
        }
    }

    #[must_use]
    pub const fn account_id(&self) -> &ProviderAccountId {
        &self.account_id
    }

    #[must_use]
    pub const fn upstream_model(&self) -> &UpstreamModelId {
        &self.upstream_model
    }

    #[must_use]
    pub const fn normal_length(&self) -> u16 {
        self.normal_length
    }

    #[must_use]
    pub const fn active(&self) -> Option<&ProviderTurnStateValue> {
        self.active.as_ref()
    }

    #[must_use]
    pub const fn standby(&self) -> Option<&ProviderTurnStateValue> {
        self.standby.as_ref()
    }

    #[must_use]
    pub const fn state_version(&self) -> u64 {
        self.state_version
    }

    #[must_use]
    pub const fn refresh_status(&self) -> ProviderTurnStateRefreshStatus {
        self.refresh_status
    }

    #[must_use]
    pub const fn last_observed_length(&self) -> Option<u16> {
        self.last_observed_length
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTurnStateSlot {
    Active,
    Standby,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTurnStateCandidate {
    pub account_id: ProviderAccountId,
    pub expected_revision: CredentialRevision,
    /// Fence passive responses to their injected version; independent probes use None.
    pub expected_active_version: Option<u64>,
    pub upstream_model: UpstreamModelId,
    pub normal_length: u16,
    pub slot: ProviderTurnStateSlot,
    pub value: ProviderTurnStateValue,
    pub observed_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTurnStateAnomaly {
    pub account_id: ProviderAccountId,
    pub expected_revision: CredentialRevision,
    pub expected_active_version: u64,
    pub upstream_model: UpstreamModelId,
    pub normal_length: u16,
    pub observed_length: Option<u16>,
    pub promote_standby: bool,
    pub observed_at: SystemTime,
}

#[derive(Debug, Clone)]
pub struct ProviderTurnStatePromotion {
    pub account_id: ProviderAccountId,
    pub expected_revision: CredentialRevision,
    pub expected_active_version: u64,
    pub upstream_model: UpstreamModelId,
    pub normal_length: u16,
    pub observed_at: SystemTime,
    pub minimum_remaining: Duration,
}

pub trait ProviderTurnStatePort: Send + Sync {
    /// Promote a still-valid standby atomically without restarting its capture clock.
    fn promote_standby(
        &self,
        _promotion: ProviderTurnStatePromotion,
    ) -> BoxFuture<'_, Result<Option<ProviderTurnStateRecord>, ProviderStoreError>> {
        Box::pin(async { Ok(None) })
    }

    fn cancel_refresh<'a>(
        &'a self,
        _account_id: &'a ProviderAccountId,
        _upstream_model: &'a UpstreamModelId,
        _expected_revision: CredentialRevision,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async { Ok(()) })
    }

    fn read<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
        upstream_model: &'a UpstreamModelId,
        expected_revision: CredentialRevision,
    ) -> BoxFuture<'a, Result<Option<ProviderTurnStateRecord>, ProviderStoreError>>;

    fn put_candidate(
        &self,
        candidate: ProviderTurnStateCandidate,
    ) -> BoxFuture<'_, Result<ProviderTurnStateRecord, ProviderStoreError>>;

    fn record_anomaly(
        &self,
        anomaly: ProviderTurnStateAnomaly,
    ) -> BoxFuture<'_, Result<ProviderTurnStateRecord, ProviderStoreError>>;

    fn mark_refresh_status<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
        upstream_model: &'a UpstreamModelId,
        expected_revision: CredentialRevision,
        normal_length: u16,
        status: ProviderTurnStateRefreshStatus,
        observed_at: SystemTime,
    ) -> BoxFuture<'a, Result<ProviderTurnStateRecord, ProviderStoreError>>;
}

#[derive(Debug, Default)]
pub struct NoopProviderTurnStatePort;

impl ProviderTurnStatePort for NoopProviderTurnStatePort {
    fn read<'a>(
        &'a self,
        _account_id: &'a ProviderAccountId,
        _upstream_model: &'a UpstreamModelId,
        _expected_revision: CredentialRevision,
    ) -> BoxFuture<'a, Result<Option<ProviderTurnStateRecord>, ProviderStoreError>> {
        Box::pin(async { Ok(None) })
    }

    fn put_candidate(
        &self,
        _candidate: ProviderTurnStateCandidate,
    ) -> BoxFuture<'_, Result<ProviderTurnStateRecord, ProviderStoreError>> {
        Box::pin(async {
            Err(ProviderStoreError::new(
                super::ProviderStoreErrorKind::Unavailable,
                "put provider turn state",
            ))
        })
    }

    fn record_anomaly(
        &self,
        _anomaly: ProviderTurnStateAnomaly,
    ) -> BoxFuture<'_, Result<ProviderTurnStateRecord, ProviderStoreError>> {
        Box::pin(async {
            Err(ProviderStoreError::new(
                super::ProviderStoreErrorKind::Unavailable,
                "record provider turn state anomaly",
            ))
        })
    }

    fn mark_refresh_status<'a>(
        &'a self,
        _account_id: &'a ProviderAccountId,
        _upstream_model: &'a UpstreamModelId,
        _expected_revision: CredentialRevision,
        _normal_length: u16,
        _status: ProviderTurnStateRefreshStatus,
        _observed_at: SystemTime,
    ) -> BoxFuture<'a, Result<ProviderTurnStateRecord, ProviderStoreError>> {
        Box::pin(async {
            Err(ProviderStoreError::new(
                super::ProviderStoreErrorKind::Unavailable,
                "mark provider turn state refresh",
            ))
        })
    }
}
