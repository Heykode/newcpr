//! Opt-in source routing; account identity and protocol projection stay elsewhere.

use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use gateway_core::{
    account::{ProviderAccount, ProviderAccountId},
    provider_ports::egress::{EgressMode, ProviderEgressConfig, ProviderEgressStorePort},
};
use reqwest::Client;
use tokio::net::{TcpSocket, TcpStream, lookup_host};

use super::{
    profile::CodexWireProfile,
    tls::{build_reqwest_native_client_with_custom_ca, custom_ca_env_cache_key},
};

const MAX_CACHED_HTTP_CLIENTS: usize = 128;
const MAX_ACCOUNT_HTTP_CLIENTS: usize = 8;
const SOURCE_HEALTH_TTL: Duration = Duration::from_secs(30);
const SOURCE_FAILURE_COOLDOWN: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CodexEgressError {
    #[error("IPv6 configuration is unavailable")]
    Unavailable,
    #[error("explicit IPv6 and an account proxy cannot be used together")]
    ProxyConflict,
    #[error("no enabled IPv6 address is available")]
    EmptyPool,
    #[error("the account's fixed IPv6 binding is unavailable")]
    BindingUnavailable,
    #[error("the selected IPv6 address is no longer enabled")]
    SourceDisabled,
    #[error("the account's egress configuration changed before dispatch")]
    ConfigurationChanged,
    #[error("the selected IPv6 address cannot be bound on this server")]
    SourceUnavailable,
    #[error("the upstream has no usable IPv6 destination")]
    DestinationUnavailable,
    #[error("the source-bound connection failed before sending a request")]
    ConnectFailed,
    #[error("the source-bound HTTP client could not be configured")]
    ClientConfiguration,
}

/// Immutable choice shared by a WS attempt and its permitted HTTP fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexEgressRoute {
    pub(crate) account_id: ProviderAccountId,
    pub(crate) source: Ipv6Addr,
    pub(crate) mode: EgressMode,
    pub(crate) revision: u64,
    pub(crate) continuation: bool,
}

impl CodexEgressRoute {
    pub(crate) fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.account_id.as_str(),
            self.source,
            self.revision
        )
    }
}

struct CachedClient {
    account_id: ProviderAccountId,
    key: String,
    client: Client,
}

#[derive(Debug, Clone, Copy)]
struct SourceHealth {
    available_until: Option<Instant>,
    blocked_until: Option<Instant>,
}

/// One process-local owner; writes reload it only after committing their state.
pub struct CodexEgressRuntime {
    store: Arc<dyn ProviderEgressStorePort>,
    reload_lock: tokio::sync::Mutex<()>,
    state: RwLock<Option<Arc<ProviderEgressConfig>>>,
    clients: Mutex<VecDeque<CachedClient>>,
    source_health: Mutex<HashMap<Ipv6Addr, SourceHealth>>,
    probe_cursor: Mutex<usize>,
}

impl CodexEgressRuntime {
    /// Request-level round robin shared by every maintenance account and model.
    pub(crate) fn next_probe_source(&self) -> Result<Ipv6Addr, CodexEgressError> {
        let mut cursor = self.probe_cursor.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.snapshot()?;
        let count = state.addresses.len();
        for _ in 0..count {
            let index = *cursor % count;
            *cursor = (index + 1) % count;
            let item = &state.addresses[index];
            if item.enabled && !self.source_temporarily_blocked(item.address) {
                return Ok(item.address);
            }
        }
        Err(CodexEgressError::EmptyPool)
    }

    pub(crate) fn probe_http_client(&self, source: Ipv6Addr) -> Result<Client, CodexEgressError> {
        let state = self.snapshot()?;
        if !state
            .addresses
            .iter()
            .any(|item| item.enabled && item.address == source)
        {
            return Err(CodexEgressError::SourceDisabled);
        }
        self.ensure_source_ready(source)?;
        build_reqwest_native_client_with_custom_ca(
            Client::builder()
                .no_proxy()
                .local_address(IpAddr::V6(source))
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .tcp_keepalive(Duration::from_secs(30))
                .http2_keep_alive_interval(Duration::from_secs(30))
                .http2_keep_alive_timeout(Duration::from_secs(5))
                .http2_keep_alive_while_idle(true)
                .pool_max_idle_per_host(0),
        )
        .map_err(|_| CodexEgressError::ClientConfiguration)
    }

    pub async fn load(
        store: Arc<dyn ProviderEgressStorePort>,
    ) -> Result<Arc<Self>, CodexEgressError> {
        let state = store
            .load()
            .await
            .map_err(|_| CodexEgressError::Unavailable)?;
        Ok(Arc::new(Self {
            store,
            reload_lock: tokio::sync::Mutex::new(()),
            state: RwLock::new(Some(state)),
            clients: Mutex::new(VecDeque::new()),
            source_health: Mutex::new(HashMap::new()),
            probe_cursor: Mutex::new(0),
        }))
    }

    pub async fn reload(&self) -> Result<(), CodexEgressError> {
        // Order reads as well as publication: a delayed load must not undo a
        // newer failure fence or let an older failure erase a recovered state.
        let _reload = self.reload_lock.lock().await;
        let loaded = self
            .store
            .load()
            .await
            .map_err(|_| CodexEgressError::Unavailable);
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match loaded {
            Ok(next) => {
                if state
                    .as_ref()
                    .is_none_or(|current| next.revision >= current.revision)
                {
                    *state = Some(next);
                    self.clients
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clear();
                    self.source_health
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clear();
                }
                Ok(())
            }
            Err(error) => {
                // Never dispatch stale enabled policy after failed post-commit publication.
                *state = None;
                Err(error)
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Result<Arc<ProviderEgressConfig>, CodexEgressError> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(CodexEgressError::Unavailable)
    }

    pub(crate) fn active(&self, account: &ProviderAccount) -> bool {
        self.snapshot().is_ok_and(|state| {
            state
                .account_overrides
                .get(account.id())
                .copied()
                .flatten()
                .unwrap_or(state.default_mode)
                .is_active()
        })
    }

    pub(crate) fn check_account(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<(), CodexEgressError> {
        if self.snapshot()?.account_overrides.contains_key(account_id) {
            Ok(())
        } else {
            Err(CodexEgressError::ConfigurationChanged)
        }
    }

    pub(crate) fn select(
        &self,
        account: &ProviderAccount,
    ) -> Result<Option<CodexEgressRoute>, CodexEgressError> {
        let state = self.snapshot()?;
        let mode = state
            .account_overrides
            .get(account.id())
            .copied()
            .ok_or(CodexEgressError::ConfigurationChanged)?
            .unwrap_or(state.default_mode);
        if !mode.is_active() {
            return Ok(None);
        }
        if account.outbound_proxy().is_some() {
            return Err(CodexEgressError::ProxyConflict);
        }
        let source = if mode.is_random() {
            let available = state
                .addresses
                .iter()
                .filter(|item| item.enabled && !self.source_temporarily_blocked(item.address))
                .collect::<Vec<_>>();
            let available = if available.is_empty() {
                // A complete temporary block must not be reported as an empty
                // configured pool. Let the next connection attempt re-check a
                // candidate after the cooldown expires.
                state
                    .addresses
                    .iter()
                    .filter(|item| item.enabled)
                    .collect::<Vec<_>>()
            } else {
                available
            };
            if available.is_empty() {
                return Err(CodexEgressError::EmptyPool);
            }
            let mut bytes = [0; 8];
            getrandom::fill(&mut bytes).map_err(|_| CodexEgressError::Unavailable)?;
            let index = u64::from_ne_bytes(bytes) % available.len() as u64;
            available[index as usize].address
        } else {
            *state
                .fixed_bindings
                .get(account.id())
                .ok_or(CodexEgressError::BindingUnavailable)?
        };
        let route = CodexEgressRoute {
            account_id: account.id().clone(),
            source,
            mode,
            revision: state.revision,
            continuation: false,
        };
        self.check(&route)?;
        Ok(Some(route))
    }

    pub(crate) fn check(&self, route: &CodexEgressRoute) -> Result<(), CodexEgressError> {
        let state = self.snapshot()?;
        let Some(override_mode) = state.account_overrides.get(&route.account_id) else {
            return Err(CodexEgressError::ConfigurationChanged);
        };
        if !state
            .addresses
            .iter()
            .any(|item| item.enabled && item.address == route.source)
        {
            return Err(CodexEgressError::SourceDisabled);
        }
        // Exact continuations keep their physical owner across a mode change,
        // but never across an account deletion or a disabled source address.
        if !route.continuation
            && (override_mode.unwrap_or(state.default_mode) != route.mode
                || (!route.mode.is_random()
                    && state.fixed_bindings.get(&route.account_id) != Some(&route.source)))
        {
            return Err(CodexEgressError::ConfigurationChanged);
        }
        Ok(())
    }

    pub(crate) fn http_client(
        &self,
        route: &CodexEgressRoute,
        profile: &CodexWireProfile,
    ) -> Result<Client, CodexEgressError> {
        self.check(route)?;
        let key = format!(
            "{}:{}:{:?}",
            route.key(),
            profile.user_agent(),
            custom_ca_env_cache_key()
        );
        if !route.mode.is_fresh() {
            let mut cache = self
                .clients
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(index) = cache.iter().position(|item| item.key == key)
                && let Some(item) = cache.remove(index)
            {
                let client = item.client.clone();
                cache.push_back(item);
                return Ok(client);
            }
        }
        // Only a new source-bound client needs a local bind check. Reused
        // clients already own a connection pool for this source and must stay
        // off the request's hot path.
        self.ensure_source_ready(route.source)?;
        let client = build_reqwest_native_client_with_custom_ca(
            Client::builder()
                .no_proxy()
                .local_address(IpAddr::V6(route.source))
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .tcp_keepalive(Duration::from_secs(30))
                .http2_keep_alive_interval(Duration::from_secs(30))
                .http2_keep_alive_timeout(Duration::from_secs(5))
                .http2_keep_alive_while_idle(true)
                .pool_idle_timeout(Duration::from_secs(60))
                .pool_max_idle_per_host(1),
        )
        .map_err(|_| CodexEgressError::ClientConfiguration)?;
        if !route.mode.is_fresh() {
            let mut cache = self
                .clients
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(existing) = cache.iter().find(|item| item.key == key) {
                return Ok(existing.client.clone());
            }
            while cache
                .iter()
                .filter(|item| item.account_id == route.account_id)
                .count()
                >= MAX_ACCOUNT_HTTP_CLIENTS
            {
                if let Some(index) = cache
                    .iter()
                    .position(|item| item.account_id == route.account_id)
                {
                    cache.remove(index);
                }
            }
            while cache.len() >= MAX_CACHED_HTTP_CLIENTS {
                cache.pop_front();
            }
            cache.push_back(CachedClient {
                account_id: route.account_id.clone(),
                key,
                client: client.clone(),
            });
        }
        Ok(client)
    }

    /// Check a source only when a new connection/client may use it.
    ///
    /// Successful checks are cached briefly to avoid repeating a local socket
    /// bind for every request. Failed checks are cooled down so random mode
    /// does not hammer the same unavailable address.
    pub(crate) fn ensure_source_ready(&self, source: Ipv6Addr) -> Result<(), CodexEgressError> {
        let now = Instant::now();
        {
            let mut health = self
                .source_health
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = health.get_mut(&source) {
                if entry.blocked_until.is_some_and(|until| until > now) {
                    return Err(CodexEgressError::SourceUnavailable);
                }
                if entry.available_until.is_some_and(|until| until > now) {
                    return Ok(());
                }
                health.remove(&source);
            }
        }

        match validate_local_source(source) {
            Ok(()) => {
                self.source_health
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        source,
                        SourceHealth {
                            available_until: Some(now + SOURCE_HEALTH_TTL),
                            blocked_until: None,
                        },
                    );
                Ok(())
            }
            Err(error) => {
                self.source_health
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        source,
                        SourceHealth {
                            available_until: None,
                            blocked_until: Some(now + SOURCE_FAILURE_COOLDOWN),
                        },
                    );
                Err(error)
            }
        }
    }

    fn source_temporarily_blocked(&self, source: Ipv6Addr) -> bool {
        let now = Instant::now();
        let mut health = self
            .source_health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = health.get(&source).copied() else {
            return false;
        };
        if entry.blocked_until.is_some_and(|until| until > now) {
            return true;
        }
        if entry.available_until.is_none_or(|until| until <= now)
            && entry.blocked_until.is_none_or(|until| until <= now)
        {
            health.remove(&source);
        }
        false
    }
}

/// Bind-only validation proves local availability, not upstream reachability.
pub fn validate_local_source(source: Ipv6Addr) -> Result<(), CodexEgressError> {
    let socket = TcpSocket::new_v6().map_err(|_| CodexEgressError::SourceUnavailable)?;
    socket
        .bind(SocketAddr::new(IpAddr::V6(source), 0))
        .map_err(|_| CodexEgressError::SourceUnavailable)
}

pub(crate) async fn connect_source_bound(
    host: &str,
    port: u16,
    source: Ipv6Addr,
) -> Result<TcpStream, CodexEgressError> {
    let addresses = lookup_host((host, port))
        .await
        .map_err(|_| CodexEgressError::DestinationUnavailable)?;
    let mut has_destination = false;
    for destination in addresses.filter(SocketAddr::is_ipv6) {
        has_destination = true;
        let socket = TcpSocket::new_v6().map_err(|_| CodexEgressError::SourceUnavailable)?;
        socket
            .bind(SocketAddr::new(IpAddr::V6(source), 0))
            .map_err(|_| CodexEgressError::SourceUnavailable)?;
        if let Ok(stream) = socket.connect(destination).await {
            stream
                .set_nodelay(true)
                .map_err(|_| CodexEgressError::ConnectFailed)?;
            return Ok(stream);
        }
    }
    Err(if has_destination {
        CodexEgressError::ConnectFailed
    } else {
        CodexEgressError::DestinationUnavailable
    })
}
