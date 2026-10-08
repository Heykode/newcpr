//! Account serialization lives only as long as an active consumer or waiter.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

use gateway_core::account::ProviderAccountId;

#[derive(Default)]
pub(super) struct ResetCreditLocks {
    entries: Mutex<HashMap<ProviderAccountId, Weak<tokio::sync::Mutex<()>>>>,
}

impl ResetCreditLocks {
    pub(super) fn entry(&self, account_id: &ProviderAccountId) -> ResetCreditEntry<'_> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let lock = entries
            .get(account_id)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                entries.insert(account_id.clone(), Arc::downgrade(&lock));
                lock
            });
        ResetCreditEntry {
            owner: self,
            account_id: account_id.clone(),
            lock: Some(lock),
        }
    }
}

pub(super) struct ResetCreditEntry<'a> {
    owner: &'a ResetCreditLocks,
    account_id: ProviderAccountId,
    lock: Option<Arc<tokio::sync::Mutex<()>>>,
}

impl ResetCreditEntry<'_> {
    pub(super) async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.lock
            .as_ref()
            .expect("entry retains its lock until drop")
            .lock()
            .await
    }
}

impl Drop for ResetCreditEntry<'_> {
    fn drop(&mut self) {
        let mut entries = self
            .owner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Drop the strong reference under the map lock: concurrent drops must not
        // both see the other reference and leave a dead weak entry behind.
        drop(self.lock.take());
        if entries
            .get(&self.account_id)
            .is_some_and(|lock| lock.strong_count() == 0)
        {
            entries.remove(&self.account_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn same_account_waiters_share_lock_and_cancelled_waiters_are_reclaimed() {
        let locks = ResetCreditLocks::default();
        let account = ProviderAccountId::new("acct_reset_lock_a").unwrap();
        let first = locks.entry(&account);
        let held = first.lock().await;
        let second = locks.entry(&account);
        let mut waiting = Box::pin(second.lock());
        assert!(futures::poll!(&mut waiting).is_pending());
        let other = locks.entry(&ProviderAccountId::new("acct_reset_lock_b").unwrap());
        let other_held = other.lock().await;
        drop(waiting);
        drop(second);
        assert_eq!(locks.entries.lock().unwrap().len(), 2);
        drop(held);
        drop(first);
        assert_eq!(locks.entries.lock().unwrap().len(), 1);
        drop(other_held);
        drop(other);
        assert!(locks.entries.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn last_waiter_preserves_serialization_until_released() {
        let locks = ResetCreditLocks::default();
        let account = ProviderAccountId::new("acct_reset_lock_a").unwrap();
        let first = locks.entry(&account);
        let held = first.lock().await;
        let second = locks.entry(&account);
        drop(held);
        drop(first);
        let held = second.lock().await;
        let third = locks.entry(&account);
        let mut waiting = Box::pin(third.lock());
        assert!(futures::poll!(&mut waiting).is_pending());
        drop(held);
        drop(second);
        let held = waiting.await;
        assert_eq!(locks.entries.lock().unwrap().len(), 1);
        drop(held);
        drop(third);
        assert!(locks.entries.lock().unwrap().is_empty());
    }
}
