//! Physical proxy sockets share a process-local budget; account identities stay separate.

use gateway_core::account::OutboundProxy;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

use super::{CodexWebSocketPool, state::WebSocketPoolSlot};
use crate::transport::websocket::CodexWebSocketExchangeError;

struct Capacity {
    used: AtomicUsize,
    changed: Notify,
}

pub(crate) struct ProxyConnectionPermit(Arc<Capacity>);

impl Drop for ProxyConnectionPermit {
    fn drop(&mut self) {
        self.0.used.fetch_sub(1, Ordering::AcqRel);
        self.0.changed.notify_waiters();
    }
}

pub(crate) fn key(proxy: Option<&OutboundProxy>) -> String {
    proxy.map_or_else(String::new, |p| {
        hex::encode(Sha256::digest(p.expose_url().as_bytes()))
    })
}

fn capacity(key: &str) -> Arc<Capacity> {
    static CAPACITIES: OnceLock<Mutex<HashMap<String, Weak<Capacity>>>> = OnceLock::new();
    let mut entries = CAPACITIES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(value) = entries.get(key).and_then(Weak::upgrade) {
        return value;
    }
    entries.retain(|_, value| value.strong_count() > 0);
    let value = Arc::new(Capacity {
        used: AtomicUsize::new(0),
        changed: Notify::new(),
    });
    entries.insert(key.to_owned(), Arc::downgrade(&value));
    value
}

pub(crate) async fn acquire(
    proxy: Option<&OutboundProxy>,
    pool: Option<&CodexWebSocketPool>,
) -> Result<Option<ProxyConnectionPermit>, CodexWebSocketExchangeError> {
    let Some(proxy) = proxy else {
        return Ok(None);
    };
    let key = key(Some(proxy));
    let capacity = capacity(&key);
    let limit = proxy.max_websocket_connections() as usize;
    loop {
        let changed = capacity.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let mut pool_changed = pool.map(|p| p.capacity_changed.subscribe());
        if pool.is_some_and(|p| p.shutdown.is_cancelled()) {
            return Err(CodexWebSocketExchangeError::SharedConnectFailed);
        }
        if capacity
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                if limit != 0 && used >= limit {
                    None
                } else {
                    used.checked_add(1)
                }
            })
            .is_ok()
        {
            return Ok(Some(ProxyConnectionPermit(Arc::clone(&capacity))));
        }
        if let Some(pool) = pool {
            let retired = {
                let mut state = pool.lock_state();
                let oldest = state
                    .slots
                    .iter()
                    .filter_map(|(candidate, slot)| match slot {
                        WebSocketPoolSlot::Idle { connection }
                            if candidate.proxy_capacity_key == key
                                && candidate.downstream_connection_id.is_empty() =>
                        {
                            Some((candidate.clone(), connection.created_at))
                        }
                        _ => None,
                    })
                    .min_by_key(|(_, since)| *since)
                    .map(|(key, _)| key);
                oldest.and_then(|key| {
                    let WebSocketPoolSlot::Idle { connection } = state.slots.remove(&key)? else {
                        return None;
                    };
                    Some(CodexWebSocketPool::retire_connection(
                        &mut state,
                        key,
                        *connection,
                        Some("proxy_capacity_evicted"),
                    ))
                })
            };
            if let Some(retired) = retired {
                let _ = pool.start_closing_one(retired).await;
                continue;
            }
        }
        tokio::select! {
            _ = &mut changed => {}
            _ = async {
                match &mut pool_changed {
                    Some(changed) => { let _ = changed.changed().await; }
                    None => std::future::pending().await,
                }
            } => {}
            _ = async {
                match pool {
                    Some(pool) => pool.shutdown.cancelled().await,
                    None => std::future::pending().await,
                }
            } => return Err(CodexWebSocketExchangeError::SharedConnectFailed),
        }
    }
}
