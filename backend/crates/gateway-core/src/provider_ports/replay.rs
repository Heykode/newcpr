//! Bounded, disposable provider history. Not an affinity record or a request log.

use futures::future::BoxFuture;

use super::{OpaqueProviderData, ProviderStoreError, ProviderStoreErrorKind};

pub const MAX_PROVIDER_REPLAY_BYTES: usize = 8 * 1024 * 1024;
pub const PROVIDER_REPLAY_TTL_SECONDS: u64 = 3600;
pub const PROVIDER_TOOL_REPLAY_IDLE_SECONDS: u64 = 2 * 3600;

pub trait ProviderReplayPort: Send + Sync {
    /// Optional image-policy digests, never conversation content or account state.
    fn read_image_policy<'a>(
        &'a self,
        _key: &'a str,
    ) -> BoxFuture<'a, Result<Option<OpaqueProviderData>, ProviderStoreError>> {
        Box::pin(async {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "image policy persistence unsupported",
            ))
        })
    }

    fn compare_exchange_image_policy<'a>(
        &'a self,
        _key: &'a str,
        _expected: Option<&'a OpaqueProviderData>,
        _payload: &'a OpaqueProviderData,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "image policy persistence unsupported",
            ))
        })
    }

    /// Small tool receipts must not compete with complete response snapshots.
    fn read_tool<'a>(
        &'a self,
        key: &'a str,
    ) -> BoxFuture<'a, Result<Option<OpaqueProviderData>, ProviderStoreError>> {
        self.read(key)
    }

    fn write_tool<'a>(
        &'a self,
        key: &'a str,
        payload: &'a OpaqueProviderData,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
        self.write(key, payload)
    }

    /// Optional mutable tool catalogs, separate from immutable response history.
    fn read_catalog<'a>(
        &'a self,
        _key: &'a str,
    ) -> BoxFuture<'a, Result<Option<OpaqueProviderData>, ProviderStoreError>> {
        Box::pin(async { Ok(None) })
    }

    /// Atomically replace only the snapshot observed by this request. A false return
    /// leaves a concurrent winner intact; unsupported adapters disable inheritance.
    fn compare_exchange_catalog<'a>(
        &'a self,
        _key: &'a str,
        _expected: Option<&'a OpaqueProviderData>,
        _payload: &'a OpaqueProviderData,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "provider catalog persistence unsupported",
            ))
        })
    }

    fn read<'a>(
        &'a self,
        key: &'a str,
    ) -> BoxFuture<'a, Result<Option<OpaqueProviderData>, ProviderStoreError>>;

    fn write<'a>(
        &'a self,
        key: &'a str,
        payload: &'a OpaqueProviderData,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>>;

    /// Disposable upstream asset references, isolated from immutable conversation records.
    /// Adapters without asset persistence may safely leave this optimization disabled.
    fn read_asset<'a>(
        &'a self,
        _key: &'a str,
    ) -> BoxFuture<'a, Result<Option<OpaqueProviderData>, ProviderStoreError>> {
        Box::pin(async { Ok(None) })
    }

    /// Return the existing winner on a concurrent insert; reads/writes never extend its life.
    fn store_asset<'a>(
        &'a self,
        _key: &'a str,
        payload: &'a OpaqueProviderData,
    ) -> BoxFuture<'a, Result<OpaqueProviderData, ProviderStoreError>> {
        Box::pin(async move { Ok(payload.clone()) })
    }

    /// Remove only the reference actually rejected, not a concurrently refreshed asset.
    fn invalidate_asset<'a>(
        &'a self,
        _key: &'a str,
        _expected: &'a OpaqueProviderData,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async { Ok(()) })
    }
}

pub struct UnavailableProviderReplay;

impl ProviderReplayPort for UnavailableProviderReplay {
    fn read<'a>(
        &'a self,
        _key: &'a str,
    ) -> BoxFuture<'a, Result<Option<OpaqueProviderData>, ProviderStoreError>> {
        Box::pin(async {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "read provider replay",
            ))
        })
    }

    fn write<'a>(
        &'a self,
        _key: &'a str,
        _payload: &'a OpaqueProviderData,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "write provider replay",
            ))
        })
    }
}
