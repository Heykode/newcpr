//! Account-scoped, disposable file IDs; no original pictures or credentials in storage.

use std::{
    collections::BTreeMap,
    sync::{Arc, LazyLock, Mutex, Weak},
    time::Duration,
};

use gateway_core::{account::OpaqueProviderData, provider_ports::ProviderReplayPort};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

const CACHE_TIMEOUT: Duration = Duration::from_millis(200);
const MAX_PENDING_KEYS: usize = 512;

#[derive(Default)]
struct UploadLocks(Mutex<BTreeMap<String, Weak<AsyncMutex<()>>>>);

impl UploadLocks {
    fn for_key(&self, key: &str) -> Option<Arc<AsyncMutex<()>>> {
        let mut locks = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
            return Some(lock);
        }
        if locks.len() >= MAX_PENDING_KEYS {
            return None;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert(key.into(), Arc::downgrade(&lock));
        Some(lock)
    }
}

static UPLOAD_LOCKS: LazyLock<UploadLocks> = LazyLock::new(UploadLocks::default);

pub(super) struct AssetCache {
    store: Arc<dyn ProviderReplayPort>,
    key: String,
}

pub(super) struct AssetReceipt {
    cache: AssetCache,
    payload: OpaqueProviderData,
}

impl AssetCache {
    pub(super) fn new(
        store: Arc<dyn ProviderReplayPort>,
        owner: &str,
        endpoint: &str,
        picture: &super::images::Picture,
    ) -> Self {
        let content = hex::encode(Sha256::digest(&picture.bytes));
        let key = hex::encode(Sha256::digest(
            json!([
                "cpr-excel-image-v1",
                owner,
                endpoint,
                picture.media,
                content
            ])
            .to_string()
            .as_bytes(),
        ));
        Self { store, key }
    }

    pub(super) async fn lock(&self) -> Option<OwnedMutexGuard<()>> {
        match UPLOAD_LOCKS.for_key(&self.key) {
            Some(lock) => Some(lock.lock_owned().await),
            None => None,
        }
    }

    pub(super) async fn read(&self) -> Option<OpaqueProviderData> {
        let payload = tokio::time::timeout(CACHE_TIMEOUT, self.store.read_asset(&self.key))
            .await
            .ok()?
            .ok()??;
        if cached_file_id(&payload).is_some() {
            Some(payload)
        } else {
            let _ = tokio::time::timeout(
                CACHE_TIMEOUT,
                self.store.invalidate_asset(&self.key, &payload),
            )
            .await;
            None
        }
    }

    pub(super) async fn store(&self, file_id: &str) -> OpaqueProviderData {
        let payload = OpaqueProviderData::new(
            json!({"version":1,"file_id":file_id})
                .as_object()
                .unwrap()
                .clone(),
        );
        tokio::time::timeout(CACHE_TIMEOUT, self.store.store_asset(&self.key, &payload))
            .await
            .ok()
            .and_then(Result::ok)
            .filter(|stored| cached_file_id(stored).is_some())
            .unwrap_or(payload)
    }

    pub(super) fn receipt(self, payload: OpaqueProviderData) -> AssetReceipt {
        AssetReceipt {
            cache: self,
            payload,
        }
    }
}

impl AssetReceipt {
    pub(super) async fn invalidate(&self) {
        let _ = tokio::time::timeout(
            CACHE_TIMEOUT,
            self.cache
                .store
                .invalidate_asset(&self.cache.key, &self.payload),
        )
        .await;
    }
}

pub(super) fn valid_file_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 512
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub(super) fn cached_file_id(payload: &OpaqueProviderData) -> Option<&str> {
    let fields = payload.expose_to_provider();
    if fields.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return None;
    }
    fields
        .get("file_id")?
        .as_str()
        .filter(|id| valid_file_id(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_keys_bind_owner_endpoint_media_and_content_not_credentials() {
        let store: Arc<dyn ProviderReplayPort> =
            Arc::new(super::super::tests::MemoryReplay::default());
        let mut picture = super::super::images::Picture {
            url: "data:image/png;base64,AQID".into(),
            media: "image/png",
            extension: "png",
            bytes: vec![1, 2, 3],
        };
        let key = |owner, endpoint, picture: &super::super::images::Picture| {
            AssetCache::new(store.clone(), owner, endpoint, picture).key
        };
        let original = key("owner-a", "https://example.com/responses", &picture);
        assert_ne!(
            original,
            key("owner-b", "https://example.com/responses", &picture)
        );
        assert_ne!(
            original,
            key("owner-a", "https://other.example.com/responses", &picture)
        );
        picture.url = "alternate representation of the same decoded bytes".into();
        assert_eq!(
            original,
            key("owner-a", "https://example.com/responses", &picture)
        );
        picture.media = "image/jpeg";
        assert_ne!(
            original,
            key("owner-a", "https://example.com/responses", &picture)
        );
        picture.media = "image/png";
        picture.bytes.push(4);
        assert_ne!(
            original,
            key("owner-a", "https://example.com/responses", &picture)
        );
    }

    #[tokio::test]
    async fn upload_locks_are_per_key_bounded_and_cancel_safe() {
        let locks = UploadLocks::default();
        let first = locks.for_key("first").unwrap();
        assert!(Arc::ptr_eq(&first, &locks.for_key("first").unwrap()));
        let held = first.clone().lock_owned().await;
        assert!(first.clone().try_lock_owned().is_err());
        let other = locks.for_key("other").unwrap().try_lock_owned().unwrap();
        drop(held);
        assert!(first.try_lock_owned().is_ok());
        drop(other);
        let mut held = Vec::new();
        for n in 0..MAX_PENDING_KEYS {
            held.push(locks.for_key(&n.to_string()).unwrap());
        }
        assert!(locks.for_key("overflow").is_none());
        drop(held);
        assert!(locks.for_key("reclaimed").is_some());
    }
}
