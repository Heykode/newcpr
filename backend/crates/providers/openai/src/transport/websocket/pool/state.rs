//! WebSocket 连接池状态和值对象。

use std::{
    collections::{HashMap, VecDeque},
    hash::{Hash, Hasher},
    sync::Arc,
    time::Duration,
};

use gateway_core::{account::ProviderAccountId, provider_ports::egress::ProviderEgressConfig};
use sha2::{Digest, Sha256};
use tokio::{sync::watch, time::Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::super::{
    diagnostics::CodexUpstreamDiagnostics, response_meta::CodexResponseMetadata,
};
use super::super::pump::{PumpedWebSocket, WebSocketConnectionObservation};
use super::lease::WebSocketPoolConnectOutcome;
use crate::transport::egress::CodexEgressError;

/// WebSocket 连接池 key。
#[derive(Debug, Clone)]
pub struct CodexWebSocketPoolKey {
    base_url: String,
    account_id: String,
    client_scope: String,
    conversation_id: String,
    connection_profile: String,
    downstream_connection_id: String,
    egress_key: String,
    pub(crate) routing: Option<CodexWebSocketRouting>,
}

/// Transport facts only; no token, cookie or per-request header is retained here.
#[derive(Debug, Clone)]
pub(crate) struct CodexWebSocketRouting {
    pub(crate) account_id: Option<ProviderAccountId>,
    pub(crate) profile: crate::transport::profile::CodexWireProfile,
    pub(crate) egress: Option<crate::transport::egress::CodexEgressRoute>,
    pub(crate) original_egress_key: String,
}

impl PartialEq for CodexWebSocketPoolKey {
    fn eq(&self, other: &Self) -> bool {
        self.base_url == other.base_url
            && self.account_id == other.account_id
            && self.client_scope == other.client_scope
            && self.conversation_id == other.conversation_id
            && self.connection_profile == other.connection_profile
            && self.downstream_connection_id == other.downstream_connection_id
            && self.egress_key == other.egress_key
    }
}

impl Eq for CodexWebSocketPoolKey {}

impl Hash for CodexWebSocketPoolKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.base_url.hash(state);
        self.account_id.hash(state);
        self.conversation_id.hash(state);
        self.connection_profile.hash(state);
        self.downstream_connection_id.hash(state);
        self.egress_key.hash(state);
        if !self.client_scope.is_empty() {
            self.client_scope.hash(state);
        }
    }
}

impl CodexWebSocketPoolKey {
    /// 构造连接池 key。
    pub fn new(
        base_url: impl Into<String>,
        account_id: impl Into<String>,
        conversation_id: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            account_id: account_id.into(),
            client_scope: String::new(),
            conversation_id: conversation_id.into(),
            connection_profile: String::new(),
            downstream_connection_id: String::new(),
            egress_key: String::new(),
            routing: None,
        }
    }

    /// 只保存完整摘要，避免连接池 Debug 暴露调用方 Key ID；空值保留低层旧调用。
    pub(crate) fn with_client_scope(mut self, scope: &str) -> Self {
        self.client_scope = if scope.is_empty() {
            String::new()
        } else {
            let mut hasher = Sha256::new();
            hasher.update(b"codex-websocket-client-scope-v1\0");
            hasher.update(scope.as_bytes());
            hex::encode(hasher.finalize())
        };
        self
    }

    /// 区分实际 WebSocket opening 画像，防止复用旧 UA 或不同握手语义的连接。
    pub(crate) fn with_connection_profile(mut self, connection_profile: impl Into<String>) -> Self {
        self.connection_profile = connection_profile.into();
        self
    }

    /// 隔离同一逻辑会话中由不同下游 WebSocket 驱动的并发响应链。
    pub(crate) fn with_downstream_connection_id(
        mut self,
        connection_id: impl Into<String>,
    ) -> Self {
        self.downstream_connection_id = connection_id.into();
        self
    }

    pub(crate) fn account_id(&self) -> &str {
        &self.account_id
    }

    pub(crate) fn with_egress_key(mut self, key: &str) -> Self {
        self.egress_key = key.to_owned();
        self
    }

    pub(crate) fn with_routing(mut self, routing: CodexWebSocketRouting) -> Self {
        self.routing = Some(routing);
        self
    }

    pub(super) fn matches_routing_owner(&self, other: &Self) -> bool {
        // Exact response ownership survives a downstream reconnect. Independent
        // chains still use the full key, including the downstream lane.
        self.base_url == other.base_url
            && self.account_id == other.account_id
            && self.client_scope == other.client_scope
            && self.conversation_id == other.conversation_id
            && self
                .routing
                .as_ref()
                .map_or(self.egress_key.as_str(), |route| {
                    route.original_egress_key.as_str()
                })
                == other.egress_key
    }

    pub(super) fn unavailable_egress(
        &self,
        config: &ProviderEgressConfig,
    ) -> Option<crate::transport::egress::CodexEgressError> {
        use crate::transport::egress::CodexEgressError;
        let routing = self.routing.as_ref()?;
        let account_id = routing
            .account_id
            .as_ref()
            .or_else(|| routing.egress.as_ref().map(|route| &route.account_id));
        if account_id.is_some_and(|id| !config.account_overrides.contains_key(id)) {
            return Some(CodexEgressError::ConfigurationChanged);
        }
        if routing.egress.as_ref().is_some_and(|route| {
            !config
                .addresses
                .iter()
                .any(|address| address.enabled && address.address == route.source)
        }) {
            return Some(CodexEgressError::SourceDisabled);
        }
        // Mode, generation and affinity changes do not invalidate a physical owner.
        None
    }

    pub(crate) fn conversation_id_hash(&self) -> String {
        short_sha256([self.conversation_id.as_str()])
    }

    pub(crate) fn stable_hash(&self) -> String {
        let parts = [
            self.base_url.as_str(),
            self.account_id.as_str(),
            self.conversation_id.as_str(),
            self.connection_profile.as_str(),
            self.downstream_connection_id.as_str(),
            self.egress_key.as_str(),
        ];
        short_sha256(
            parts
                .into_iter()
                .chain((!self.client_scope.is_empty()).then_some(self.client_scope.as_str())),
        )
    }

    pub(super) fn same_logical_connection(&self, other: &Self) -> bool {
        self.base_url == other.base_url
            && self.account_id == other.account_id
            && self.client_scope == other.client_scope
            && self.conversation_id == other.conversation_id
            && self.egress_key == other.egress_key
    }
}

const CONTINUATION_TOMBSTONE_TTL: Duration = Duration::from_mins(30);
const MAX_CONTINUATION_TOMBSTONES: usize = 4_096;

#[derive(Default)]
pub(super) struct WebSocketPoolState {
    pub(super) slots: HashMap<CodexWebSocketPoolKey, WebSocketPoolSlot>,
    pub(super) egress_config: Option<Arc<ProviderEgressConfig>>,
    continuation_tombstones: VecDeque<WebSocketContinuationTombstone>,
    pub(super) shutting_down: bool,
}

struct WebSocketContinuationTombstone {
    key: CodexWebSocketPoolKey,
    response_id: String,
    observation: WebSocketConnectionObservation,
    expires_at: Instant,
}

impl WebSocketPoolState {
    pub(super) fn remember_continuation_loss(
        &mut self,
        key: &CodexWebSocketPoolKey,
        response_id: Option<&str>,
        observation: WebSocketConnectionObservation,
        now: Instant,
    ) {
        let Some(response_id) = response_id else {
            return;
        };
        self.prune_continuation_tombstones(now);
        self.continuation_tombstones.retain(|tombstone| {
            !(tombstone.key.same_logical_connection(key) && tombstone.response_id == response_id)
        });
        while self.continuation_tombstones.len() >= MAX_CONTINUATION_TOMBSTONES {
            self.continuation_tombstones.pop_front();
        }
        self.continuation_tombstones
            .push_back(WebSocketContinuationTombstone {
                key: key.clone(),
                response_id: response_id.to_owned(),
                observation,
                expires_at: now + CONTINUATION_TOMBSTONE_TTL,
            });
    }

    pub(super) fn continuation_loss(
        &mut self,
        key: &CodexWebSocketPoolKey,
        response_id: &str,
        now: Instant,
    ) -> Option<WebSocketConnectionObservation> {
        self.prune_continuation_tombstones(now);
        self.continuation_tombstones
            .iter()
            .rev()
            .find(|tombstone| {
                tombstone.key.same_logical_connection(key) && tombstone.response_id == response_id
            })
            .map(|tombstone| tombstone.observation.clone())
    }

    fn prune_continuation_tombstones(&mut self, now: Instant) {
        self.continuation_tombstones
            .retain(|tombstone| tombstone.expires_at > now);
    }
}

#[derive(Clone)]
pub(crate) struct CodexWebSocketConnectionMetadata {
    pub(crate) turn_state: Option<String>,
    pub(crate) set_cookie_headers: Vec<String>,
    pub(crate) rate_limit_headers: Vec<(String, String)>,
    pub(crate) rate_limit_observed_at: std::time::SystemTime,
    pub(crate) response_metadata: CodexResponseMetadata,
    pub(crate) diagnostics: CodexUpstreamDiagnostics,
}

pub(crate) struct PooledWebSocketConnection {
    pub(crate) websocket: PumpedWebSocket,
    pub(crate) metadata: CodexWebSocketConnectionMetadata,
    pub(crate) continuation: WebSocketContinuationState,
    pub(crate) created_at: Instant,
}

/// 只随具体 WebSocket 生命周期存在的续接状态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WebSocketContinuationState {
    latest_response_id: Option<String>,
}

impl WebSocketContinuationState {
    pub(crate) fn latest_response_id(&self) -> Option<&str> {
        self.latest_response_id.as_deref()
    }

    pub(crate) fn record_completed(&mut self, response_id: String) {
        self.latest_response_id = Some(response_id);
    }
}

pub(super) enum WebSocketPoolSlot {
    Idle {
        connection: Box<PooledWebSocketConnection>,
    },
    Busy(WebSocketPoolReservation),
    Connecting(WebSocketPoolConnecting),
    Closing(Uuid),
}

impl WebSocketPoolSlot {
    pub(super) fn latest_response_id(&self) -> Option<&str> {
        match self {
            Self::Idle { connection, .. } => connection.continuation.latest_response_id(),
            Self::Busy(reservation) => reservation.latest_response_id.as_deref(),
            Self::Connecting(_) | Self::Closing(_) => None,
        }
    }
}

pub(super) struct WebSocketPoolConnecting {
    pub(super) id: Uuid,
    pub(super) started_at: Instant,
    pub(super) outcome: watch::Sender<WebSocketPoolConnectOutcome>,
    pub(super) cancellation: CancellationToken,
}

impl WebSocketPoolConnecting {
    pub(super) fn cancel(&self, error: Option<CodexEgressError>) {
        self.outcome.send_if_modified(|outcome| {
            if matches!(outcome, WebSocketPoolConnectOutcome::LocalEgress(_)) {
                return false;
            }
            *outcome = error.map_or(
                WebSocketPoolConnectOutcome::Failed,
                WebSocketPoolConnectOutcome::LocalEgress,
            );
            true
        });
        self.cancellation.cancel();
    }
}

#[derive(Clone)]
pub(super) struct WebSocketPoolReservation {
    pub(super) id: Uuid,
    pub(super) reserved_at: Instant,
    pub(super) latest_response_id: Option<String>,
    pub(super) retire_on_return: bool,
}

pub(super) async fn close_pooled_connection(connection: PooledWebSocketConnection) {
    connection.websocket.close().await;
}

pub(super) async fn close_pooled_connections(connections: Vec<PooledWebSocketConnection>) {
    for connection in connections {
        close_pooled_connection(connection).await;
    }
}

/// idle 连接是否应从池中摘除：被后台 pump 标记死亡，或已超过 `max_age`。
pub(super) fn should_close_idle_connection(
    connection: &PooledWebSocketConnection,
    now: Instant,
    max_age: Duration,
) -> bool {
    connection.websocket.is_closed() || now.duration_since(connection.created_at) >= max_age
}

fn short_sha256<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(b"\0");
    }
    hex::encode(hasher.finalize()).chars().take(12).collect()
}
