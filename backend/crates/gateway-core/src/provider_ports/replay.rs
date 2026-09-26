//! Bounded, disposable provider history. Not an affinity record or a request log.

use futures::future::BoxFuture;

use super::{OpaqueProviderData, ProviderStoreError, ProviderStoreErrorKind};

pub const MAX_PROVIDER_REPLAY_BYTES: usize = 8 * 1024 * 1024;
pub const PROVIDER_REPLAY_TTL_SECONDS: u64 = 3600;

pub trait ProviderReplayPort: Send + Sync {
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
