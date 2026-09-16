use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use crate::account::AccountConcurrencyLimits;
use crate::routing::ConfigRevision;

use super::{
    RUNTIME_SNAPSHOT_STALE_GRACE, RuntimeSnapshotUnavailable, read_unpoisoned, write_unpoisoned,
};

/// Effective limits from one consistent configuration revision, including disabled accounts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountConcurrencySnapshot {
    revision: ConfigRevision,
    limits: BTreeMap<String, NonZeroU32>,
}

impl AccountConcurrencySnapshot {
    #[must_use]
    pub fn new(revision: ConfigRevision, limits: BTreeMap<String, NonZeroU32>) -> Self {
        Self { revision, limits }
    }

    #[must_use]
    pub const fn revision(&self) -> ConfigRevision {
        self.revision
    }

    /// Missing accounts have been deleted or were absent from this publication.
    #[must_use]
    pub fn limit_for(&self, account_id: &str) -> Option<NonZeroU32> {
        self.limits.get(account_id).copied()
    }
}

impl AccountConcurrencyLimits for AccountConcurrencySnapshot {
    fn limit_for(&self, account_id: &str) -> Option<NonZeroU32> {
        AccountConcurrencySnapshot::limit_for(self, account_id)
    }
}

/// Live pool admission state; never republish per-request frozen account data here.
#[derive(Debug, Clone, Default)]
pub struct AccountConcurrencyHandle {
    pub(super) state: Arc<RwLock<AccountConcurrencyState>>,
}

#[derive(Debug, Default)]
pub(super) struct AccountConcurrencyState {
    // Retain the latest snapshot while suspended to preserve the revision high water mark.
    current: Option<Arc<AccountConcurrencySnapshot>>,
    suspended: bool,
    suspended_at: Option<Instant>,
    pub(super) fence: Arc<()>,
}

impl AccountConcurrencyState {
    pub(super) fn publish(&mut self, snapshot: Arc<AccountConcurrencySnapshot>) -> bool {
        if self.current.as_ref().is_some_and(|current| {
            snapshot.revision() < current.revision()
                || (snapshot.revision() == current.revision() && snapshot != *current)
        }) {
            return false;
        }
        self.current = Some(snapshot);
        self.suspended = false;
        self.suspended_at = None;
        true
    }

    pub(super) fn suspend(&mut self) {
        self.suspended = true;
        self.suspended_at.get_or_insert_with(Instant::now);
        self.fence = Arc::new(());
    }

    pub(super) fn is_suspended(&self) -> bool {
        self.suspended
    }

    fn is_available_at(&self, now: Instant) -> bool {
        self.current.is_some()
            && self
                .suspended_at
                .is_none_or(|started| now.duration_since(started) < RUNTIME_SNAPSHOT_STALE_GRACE)
    }
}

impl AccountConcurrencyHandle {
    #[must_use]
    pub fn new(revision: ConfigRevision, limits: BTreeMap<String, NonZeroU32>) -> Self {
        let handle = Self::default();
        handle.publish(revision, limits);
        handle
    }

    /// `None` means no snapshot has ever been published; a fence always returns an error.
    pub fn load(
        &self,
    ) -> Result<Option<Arc<AccountConcurrencySnapshot>>, RuntimeSnapshotUnavailable> {
        let state = read_unpoisoned(&self.state);
        if state.suspended && !state.is_available_at(Instant::now()) {
            return Err(RuntimeSnapshotUnavailable);
        }
        Ok(state.current.clone())
    }

    /// Publish authoritative state. Reject older revisions and conflicting equal revisions.
    /// A matching equal revision may recover a suspended handle after a successful reload.
    pub fn publish(&self, revision: ConfigRevision, limits: BTreeMap<String, NonZeroU32>) -> bool {
        self.publish_snapshot(Arc::new(AccountConcurrencySnapshot::new(revision, limits)))
    }

    pub(crate) fn publish_snapshot(&self, snapshot: Arc<AccountConcurrencySnapshot>) -> bool {
        write_unpoisoned(&self.state).publish(snapshot)
    }

    pub fn suspend(&self) {
        write_unpoisoned(&self.state).suspend();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspended_capacity_remains_available_only_inside_grace_period() {
        let mut state = AccountConcurrencyState {
            current: Some(Arc::new(AccountConcurrencySnapshot::new(
                ConfigRevision::new(1).expect("revision"),
                BTreeMap::new(),
            ))),
            suspended: false,
            suspended_at: None,
            fence: Arc::new(()),
        };
        let suspended_at = Instant::now();

        state.suspended_at = Some(suspended_at);
        assert!(state.is_available_at(suspended_at));
        assert!(
            state.is_available_at(
                suspended_at
                    .checked_add(RUNTIME_SNAPSHOT_STALE_GRACE)
                    .expect("grace deadline")
                    .checked_sub(std::time::Duration::from_nanos(1))
                    .expect("before deadline"),
            )
        );
        assert!(
            !state.is_available_at(
                suspended_at
                    .checked_add(RUNTIME_SNAPSHOT_STALE_GRACE)
                    .expect("grace deadline"),
            )
        );
    }

    #[test]
    fn repeated_suspension_does_not_extend_the_original_grace_period() {
        let mut state = AccountConcurrencyState {
            current: Some(Arc::new(AccountConcurrencySnapshot::new(
                ConfigRevision::new(1).expect("revision"),
                BTreeMap::new(),
            ))),
            suspended: false,
            suspended_at: None,
            fence: Arc::new(()),
        };

        state.suspend();
        let first_suspension = state.suspended_at.expect("first suspension");
        state.suspend();

        assert_eq!(state.suspended_at, Some(first_suspension));
    }
}
