//! Optional account exits; independent of routing, account admission and affinity.
pub use crate::account::RequestProxySource;
use crate::account::{OutboundProxy, ResponsesUpstream};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionProxyOutcome {
    Completed,
    NetworkFailure,
    StreamFailure,
    UpstreamFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionProxyError {
    #[error("manager_unavailable")]
    Unavailable,
    #[error("warm_pool_empty")]
    Warming,
    #[error("session_draining")]
    Draining,
    #[error("session_capacity")]
    Capacity,
    #[error("session_identity_required")]
    Identity,
}

/// Clones share one logical reference; dropping the last owner releases it.
pub trait SessionProxyLease: Send + Sync {
    fn proxy(&self) -> &OutboundProxy;
    fn node_id(&self) -> &str;
    fn report(&self, outcome: SessionProxyOutcome);
    /// Only after transport proves no request bytes were sent. Concurrent live
    /// requests prevent rebinding; no probing or direct fallback is permitted.
    fn retry_unsent(&self) -> Option<Arc<dyn SessionProxyLease>> {
        None
    }
}

pub trait SessionProxyPool: Send + Sync {
    /// Must not perform network I/O, database queries or cold-node qualification.
    fn acquire(
        &self,
        source: RequestProxySource,
        route: ResponsesUpstream,
        scope: &str,
        transient: bool,
    ) -> Result<Arc<dyn SessionProxyLease>, SessionProxyError>;
}
