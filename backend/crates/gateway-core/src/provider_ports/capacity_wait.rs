//! Account wait ownership is separate from an acquired execution slot.

use std::fmt;
use std::num::NonZeroU32;
use std::time::{Duration, SystemTime};

use futures::future::BoxFuture;

use super::{ProviderLeaseGuard, ProviderSchedulingLeaseRequest, ProviderStoreError};
use crate::account::ProviderAccountId;
use crate::engine::{AccountWaitMode, ModelRequestId};
use crate::identity::ProviderKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderWaitLeaseRequest {
    provider_kind: ProviderKind,
    account_id: ProviderAccountId,
    request_id: ModelRequestId,
    mode: AccountWaitMode,
    max_waiting: NonZeroU32,
    deadline: SystemTime,
}

impl ProviderWaitLeaseRequest {
    #[must_use]
    pub const fn new(
        provider_kind: ProviderKind,
        account_id: ProviderAccountId,
        request_id: ModelRequestId,
        mode: AccountWaitMode,
        max_waiting: NonZeroU32,
        deadline: SystemTime,
    ) -> Self {
        Self {
            provider_kind,
            account_id,
            request_id,
            mode,
            max_waiting,
            deadline,
        }
    }

    #[must_use]
    pub const fn provider_kind(&self) -> &ProviderKind {
        &self.provider_kind
    }

    #[must_use]
    pub const fn account_id(&self) -> &ProviderAccountId {
        &self.account_id
    }

    #[must_use]
    pub const fn request_id(&self) -> &ModelRequestId {
        &self.request_id
    }

    #[must_use]
    pub const fn mode(&self) -> AccountWaitMode {
        self.mode
    }

    #[must_use]
    pub const fn max_waiting(&self) -> NonZeroU32 {
        self.max_waiting
    }

    #[must_use]
    pub const fn deadline(&self) -> SystemTime {
        self.deadline
    }
}

pub enum ProviderWaitLeaseAcquisition {
    Acquired(Box<dyn ProviderWaitLease>),
    Full,
}

impl fmt::Debug for ProviderWaitLeaseAcquisition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Acquired(_) => formatter.write_str("Acquired([WAIT_LEASE])"),
            Self::Full => formatter.write_str("Full"),
        }
    }
}

pub enum ProviderWaitPromotion {
    Acquired(Box<dyn ProviderLeaseGuard>),
    Busy { retry_after: Option<Duration> },
    Expired,
}

impl fmt::Debug for ProviderWaitPromotion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Acquired(_) => formatter.write_str("Acquired([LEASE])"),
            Self::Busy { retry_after } => formatter
                .debug_struct("Busy")
                .field("retry_after", retry_after)
                .finish(),
            Self::Expired => formatter.write_str("Expired"),
        }
    }
}

/// Store owns atomic promotion, cancellation-safe handoff and expiring cleanup.
///
/// Promotion must verify the same provider/account, a live wait token, execution
/// capacity and request interval. Success removes the wait registration and
/// disarms its cleanup before handing off the execution guard. Dropping either
/// an acquisition or promotion future must also clean up an uncertain grant.
pub trait ProviderWaitLease: Send + Sync + 'static {
    fn try_promote(
        &mut self,
        request: ProviderSchedulingLeaseRequest,
    ) -> BoxFuture<'_, Result<ProviderWaitPromotion, ProviderStoreError>>;

    /// Idempotent normal exit; Drop must also cover an unpolled release future.
    fn release(self: Box<Self>) -> BoxFuture<'static, Result<(), ProviderStoreError>>;
}
