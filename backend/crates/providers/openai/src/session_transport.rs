//! 按客户端线程维护 WS 失败预算及大包 HTTP 回退区间，空闲过期后释放状态

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use gateway_core::provider_ports::ProviderSessionAffinityKey;
use tokio::time::Instant;

// 用 2 MiB 回差避免完整请求在配置阈值附近频繁切换传输。
const MIB: usize = 1024 * 1024;

const SESSION_IDLE_RETENTION: Duration = Duration::from_secs(8 * 60 * 60);
const MAX_SESSION_TRANSPORTS: usize = 16_384;

#[derive(Clone, Default)]
pub(crate) struct CodexSessionTransportRecovery {
    sessions: Arc<Mutex<HashMap<ProviderSessionAffinityKey, SessionTransport>>>,
}

struct SessionTransport {
    state: TransportState,
    oversized: bool,
    last_used: Instant,
}

enum TransportState {
    WebSocket { failures: u32 },
    Http,
}

impl CodexSessionTransportRecovery {
    pub(crate) fn uses_http(&self, key: &ProviderSessionAffinityKey) -> bool {
        let now = Instant::now();
        let mut sessions = self.lock();
        let Some(session) = sessions.get_mut(key) else {
            return false;
        };
        if now.saturating_duration_since(session.last_used) >= SESSION_IDLE_RETENTION {
            sessions.remove(key);
            return false;
        }
        session.last_used = now;
        matches!(session.state, TransportState::Http)
    }

    /// 返回连续失败是否已耗尽首发加重试预算。
    pub(crate) fn record_websocket_failure(
        &self,
        key: &ProviderSessionAffinityKey,
        max_retries: u32,
    ) -> bool {
        self.update(key, |session| {
            session.state = match session.state {
                TransportState::WebSocket { failures } if failures < max_retries => {
                    TransportState::WebSocket {
                        failures: failures + 1,
                    }
                }
                _ => TransportState::Http,
            };
            matches!(session.state, TransportState::Http)
        })
    }

    pub(crate) fn disable_websocket(&self, key: &ProviderSessionAffinityKey) {
        self.update(key, |session| session.state = TransportState::Http);
    }

    pub(crate) fn websocket_succeeded(&self, key: &ProviderSessionAffinityKey) {
        // 预热或增量成功不能证明下一份完整请求能放进 WS
        self.update(key, |session| {
            session.state = TransportState::WebSocket { failures: 0 }
        });
    }

    /// 只用完整新链的实际帧长度更新大小偏好，重连沿用同一线程的判断
    pub(crate) fn http_for_full_payload(
        &self,
        key: Option<&ProviderSessionAffinityKey>,
        bytes: usize,
        limit: usize,
    ) -> bool {
        if limit == 0 {
            if let Some(key) = key {
                self.update(key, |session| session.oversized = false);
            }
            return false;
        }
        let resume = limit.saturating_sub(2 * MIB).max(1);
        let Some(key) = key else {
            return bytes >= limit;
        };
        self.update(key, |session| {
            if bytes >= limit {
                session.oversized = true;
            } else if bytes < resume {
                session.oversized = false;
            }
            session.oversized
        })
    }

    fn update<T>(
        &self,
        key: &ProviderSessionAffinityKey,
        update: impl FnOnce(&mut SessionTransport) -> T,
    ) -> T {
        let now = Instant::now();
        let mut sessions = self.lock();
        let mut session = sessions
            .remove(key)
            .filter(|s| now.saturating_duration_since(s.last_used) < SESSION_IDLE_RETENTION)
            .unwrap_or(SessionTransport {
                state: TransportState::WebSocket { failures: 0 },
                oversized: false,
                last_used: now,
            });
        let result = update(&mut session);
        if !session.oversized && matches!(session.state, TransportState::WebSocket { failures: 0 })
        {
            return result;
        }
        session.last_used = now;
        if sessions.len() >= MAX_SESSION_TRANSPORTS {
            sessions.retain(|_, session| {
                now.saturating_duration_since(session.last_used) < SESSION_IDLE_RETENTION
            });
            if sessions.len() >= MAX_SESSION_TRANSPORTS
                && let Some(oldest) = sessions
                    .iter()
                    .min_by_key(|(_, session)| session.last_used)
                    .map(|(key, _)| key.clone())
            {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(key.clone(), session);
        result
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<ProviderSessionAffinityKey, SessionTransport>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_thread_stays_http_until_full_payload_is_below_resume_threshold() {
        let recovery = CodexSessionTransportRecovery::default();
        let key = ProviderSessionAffinityKey::try_new("thread_test").unwrap();
        let other = ProviderSessionAffinityKey::try_new("thread_other").unwrap();
        let limit = 31 * MIB / 2;
        assert!(!recovery.http_for_full_payload(Some(&key), limit - 1, limit));
        assert!(recovery.http_for_full_payload(Some(&key), limit, limit));
        recovery.websocket_succeeded(&key);
        assert!(recovery.http_for_full_payload(Some(&key), limit - 1, limit));
        assert!(!recovery.http_for_full_payload(Some(&other), limit - 1, limit));
        assert!(recovery.http_for_full_payload(Some(&key), limit - 2 * MIB, limit));
        assert!(!recovery.http_for_full_payload(Some(&key), limit - 2 * MIB - 1, limit));
        assert!(!recovery.http_for_full_payload(Some(&key), limit - 1, limit));
    }

    #[test]
    fn zero_and_expiry_clear_size_memory_without_erasing_failure_budget() {
        let recovery = CodexSessionTransportRecovery::default();
        let key = ProviderSessionAffinityKey::try_new("thread_test").unwrap();
        let limit = 15 * MIB;
        assert!(recovery.http_for_full_payload(Some(&key), limit, limit));
        recovery.disable_websocket(&key);
        assert!(!recovery.http_for_full_payload(Some(&key), limit, 0));
        assert!(recovery.uses_http(&key));
        recovery.websocket_succeeded(&key);
        assert!(!recovery.uses_http(&key));
        assert!(recovery.http_for_full_payload(Some(&key), limit, limit));
        recovery.lock().get_mut(&key).unwrap().last_used = Instant::now() - SESSION_IDLE_RETENTION;
        assert!(!recovery.http_for_full_payload(Some(&key), limit - 1, limit));
        assert!(recovery.http_for_full_payload(None, limit, limit));
        assert!(!recovery.http_for_full_payload(None, limit - 1, limit));
    }
}
