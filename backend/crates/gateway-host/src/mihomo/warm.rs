use super::{
    ManagedMihomo,
    pool::{PoolState, lock},
    sources::read_limited,
    state::{digest, node_id},
};
use futures::{StreamExt, stream};
use gateway_admin::{
    model::{PageSize, proxies::ProxyListQuery},
    ports::proxy::ProxyStore,
};
use gateway_core::{
    account::{AccountStatus, OutboundProxy, ProviderAccountStore, ResponsesUpstream},
    provider_ports::session_proxy::RequestProxySource,
    runtime::AccountConcurrencyHandle,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

struct Flight {
    pool: Arc<Mutex<PoolState>>,
    id: String,
    owner: (u64, u64),
}
impl Drop for Flight {
    fn drop(&mut self) {
        if let Some(h) = lock(&self.pool).exits.get_mut(&self.id)
            && h.probe_owner == Some(self.owner)
        {
            h.probing = false;
            h.probe_owner = None;
        }
    }
}

async fn protocol(client: &reqwest::Client, route: ResponsesUpstream) -> Result<(), &'static str> {
    let endpoint = match route {
        ResponsesUpstream::Codex => "https://chatgpt.com/backend-api/codex/responses",
        ResponsesUpstream::Excel => "https://bps.openai.com/basispoints/api/responses",
    };
    let response = client
        .post(endpoint)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .map_err(|_| "network_error")?;
    let status = response.status().as_u16();
    if status == 401 {
        let json = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
            });
        if !json {
            return Err("unexpected_content_type");
        }
        let body = read_limited(response, 16 * 1024)
            .await
            .map_err(|_| "invalid_response")?;
        return auth_rejection(&body);
    }
    Err(match status {
        403 => "access_denied",
        407 => "proxy_auth_required",
        429 => "rate_limited",
        300..=399 => "redirect",
        500..=599 => "upstream_error",
        _ => "unexpected_status",
    })
}

pub(super) fn auth_rejection(body: &[u8]) -> Result<(), &'static str> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| "invalid_json")?;
    let nonempty = |v: &serde_json::Value| v.as_str().is_some_and(|s| !s.trim().is_empty());
    if nonempty(&value["error"])
        || nonempty(&value["error"]["message"])
        || nonempty(&value["error"]["code"])
        || nonempty(&value["detail"])
    {
        Ok(())
    } else {
        Err("missing_error")
    }
}

async fn candidate(
    pool: Arc<Mutex<PoolState>>,
    id: String,
    build: Arc<super::ClientBuilder>,
    route: ResponsesUpstream,
) -> bool {
    let (proxy, generation, revision, recover) = {
        let mut state = lock(&pool);
        let now = Instant::now();
        let Some(h) = state.exits.get_mut(&id) else {
            return false;
        };
        if h.probing || h.cooling(now) {
            return false;
        }
        h.probing = true;
        h.probe_owner = Some((h.generation, h.revision));
        (h.proxy.clone(), h.generation, h.revision, h.failures > 0)
    };
    let _flight = Flight {
        pool: pool.clone(),
        id: id.clone(),
        owner: (generation, revision),
    };
    let mut observations = Vec::with_capacity(3);
    let outcome = tokio::time::timeout(Duration::from_secs(4), async {
        let proxy = reqwest::Proxy::all(proxy.expose_url()).map_err(|_| "proxy_configuration")?;
        let client = build(
            reqwest::Client::builder()
                .no_proxy()
                .proxy(proxy)
                .pool_max_idle_per_host(0)
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .connect_timeout(Duration::from_secs(3)),
        )
        .map_err(|_| "proxy_configuration")?;
        let mut success = 0;
        let mut failures = 0;
        let mut needed = if recover { 2 } else { 1 };
        let mut last = "network_error";
        for _ in 0..3 {
            match protocol(&client, route).await {
                Ok(()) => {
                    observations.push(true);
                    success += 1;
                    if success >= needed {
                        return Ok(());
                    }
                }
                Err(reason) => {
                    observations.push(false);
                    success = 0;
                    failures += 1;
                    needed = 2;
                    last = reason;
                    if failures >= 2 || recover {
                        break;
                    }
                }
            }
        }
        Err(last)
    })
    .await
    .unwrap_or_else(|_| {
        observations.push(false);
        Err("probe_timeout")
    });
    let successful = outcome.is_ok();
    lock(&pool).qualify(
        &id,
        generation,
        revision,
        outcome,
        &observations,
        Instant::now(),
    );
    successful
}

async fn refill(
    pool: Arc<Mutex<PoolState>>,
    target: usize,
    build: Arc<super::ClientBuilder>,
    route: ResponsesUpstream,
) {
    lock(&pool).target = target.min(4096);
    if target == 0 {
        return;
    }
    let work = async {
        let refresh = {
            let mut state = lock(&pool);
            let now = Instant::now();
            state.status(now);
            if !state.available {
                return;
            }
            let loads = state.loads();
            state
                .rank(now)
                .into_iter()
                .enumerate()
                .filter_map(|(i, id)| {
                    let h = &state.exits[&id];
                    ((i < target || loads.get(&id).is_some_and(|(active, _)| *active > 0))
                        && h.ready(now)
                        && h.verified_until
                            .is_some_and(|t| t <= now + Duration::from_secs(30)))
                    .then_some(id)
                })
                .take(4)
                .collect::<Vec<_>>()
        };
        // Refresh does not withdraw a still-valid observation while awaiting I/O.
        stream::iter(
            refresh
                .into_iter()
                .map(|id| candidate(pool.clone(), id, build.clone(), route)),
        )
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
        loop {
            let ids = {
                let mut state = lock(&pool);
                let now = Instant::now();
                state.status(now);
                let eligible: Vec<_> = state
                    .rank(now)
                    .into_iter()
                    .filter(|id| !state.exits[id].cooling(now))
                    .collect();
                let ready = eligible
                    .iter()
                    .filter(|id| state.exits[*id].ready(now))
                    .count();
                let subscriptions = eligible
                    .iter()
                    .filter(|id| state.exits[*id].subscription)
                    .count();
                let ready_subscriptions = eligible
                    .iter()
                    .filter(|id| state.exits[*id].subscription && state.exits[*id].ready(now))
                    .count();
                let only_subscription = ready_subscriptions < target.min(subscriptions);
                if !state.available || (ready >= target && !only_subscription) {
                    break;
                }
                eligible
                    .into_iter()
                    .filter(|id| {
                        let h = &state.exits[id];
                        !h.ready(now) && !h.probing && (!only_subscription || h.subscription)
                    })
                    .take(32)
                    .collect::<Vec<_>>()
            };
            if ids.is_empty() {
                break;
            }
            let mut race = stream::iter(
                ids.into_iter()
                    .map(|id| candidate(pool.clone(), id, build.clone(), route)),
            )
            .buffer_unordered(4);
            let mut won = false;
            while let Some(success) = race.next().await {
                if success {
                    won = true;
                    break;
                }
            }
            drop(race);
            if !won {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(8), work).await;
}

impl ManagedMihomo {
    pub async fn start_warm(
        &self,
        accounts: Arc<dyn ProviderAccountStore>,
        proxies: Arc<dyn ProxyStore>,
        limits: AccountConcurrencyHandle,
    ) {
        let mut slot = self.warmer.lock().await;
        if slot.is_some() {
            return;
        }
        let service = self.clone();
        *slot = Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(5));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut static_read = None;
            loop {
                tokio::select! {()=service.cancellation.cancelled()=>break,_=ticker.tick()=>{}}
                let run = async {
                    let list = accounts.list_accounts().await.map_err(|_| ())?;
                    let limit_snapshot = limits.load().map_err(|_| ())?;
                    let mut targets = [[0_usize; 2]; 2];
                    for account in list {
                        if account.provider().as_str() != "openai"
                            || account.status_projection(SystemTime::now(), None).status
                                != AccountStatus::Normal
                        {
                            continue;
                        }
                        let count = limit_snapshot
                            .as_ref()
                            .and_then(|s| s.limit_for(account.id().as_str()))
                            .map_or(0, |n| n.get() as usize);
                        let source = match account.request_proxy_source() {
                            RequestProxySource::Mihomo => 0,
                            RequestProxySource::ProxyPool => 1,
                            RequestProxySource::Account => continue,
                        };
                        // Excel accounts may still use Codex for models outside their allowlist.
                        targets[0][source] = targets[0][source].saturating_add(count);
                        if account.responses_upstream() == ResponsesUpstream::Excel {
                            targets[1][source] = targets[1][source].saturating_add(count);
                        }
                    }
                    if (targets[0][1] > 0 || targets[1][1] > 0)
                        && static_read
                            .is_none_or(|t: Instant| t.elapsed() >= Duration::from_secs(15))
                    {
                        static_read = Some(Instant::now());
                        let refreshed = async {
                            let mut exits = Vec::new();
                            let mut page = 1;
                            loop {
                                let result = proxies
                                    .list(ProxyListQuery {
                                        page,
                                        page_size: PageSize::new(200).map_err(|_| ())?,
                                        search: String::new(),
                                    })
                                    .await
                                    .map_err(|_| ())?;
                                for item in result.items {
                                    let id = digest(item.proxy.expose_url().as_bytes());
                                    exits.push((id, item.proxy, false, false));
                                }
                                if u64::from(page) * 200 >= result.total {
                                    break;
                                }
                                page += 1;
                            }
                            Ok::<_, ()>(exits)
                        }
                        .await;
                        match refreshed {
                            Ok(exits) => {
                                lock(&service.pools.codex_regular).replace(
                                    exits.clone(),
                                    true,
                                    Instant::now(),
                                );
                                lock(&service.pools.regular).replace(exits, true, Instant::now());
                            }
                            Err(()) => tracing::warn!(
                                "managed proxy static membership refresh failed; retaining previous snapshot"
                            ),
                        }
                    }
                    service.publish_exits().await;
                    tokio::join!(
                        refill(
                            service.pools.managed.clone(),
                            targets[1][0],
                            service.build_client.clone(),
                            ResponsesUpstream::Excel
                        ),
                        refill(
                            service.pools.regular.clone(),
                            targets[1][1],
                            service.build_client.clone(),
                            ResponsesUpstream::Excel
                        ),
                        refill(
                            service.pools.codex_managed.clone(),
                            targets[0][0],
                            service.build_client.clone(),
                            ResponsesUpstream::Codex
                        ),
                        refill(
                            service.pools.codex_regular.clone(),
                            targets[0][1],
                            service.build_client.clone(),
                            ResponsesUpstream::Codex
                        )
                    );
                    Ok::<_, ()>(())
                };
                tokio::select! {()=service.cancellation.cancelled()=>break,result=tokio::time::timeout(Duration::from_secs(20),run)=>{
                    if !matches!(result,Ok(Ok(()))) {tracing::warn!("managed proxy warm configuration refresh failed");}
                }}
            }
        }));
    }

    pub(super) async fn publish_exits(&self) {
        let mut kernel = match self.kernel.try_lock() {
            Ok(kernel) => kernel,
            Err(_) => return,
        };
        let running = kernel.running();
        self.running
            .store(running, std::sync::atomic::Ordering::Release);
        let saved = self.saved.read().await;
        let dynamic = saved.dynamic_ids();
        let exits = saved
            .nodes
            .iter()
            .filter_map(|node| {
                let id = node_id(node);
                let is_dynamic = dynamic.contains(&id);
                if !saved.eligible(node, is_dynamic) {
                    return None;
                }
                let port = kernel.ports.get(&id)?;
                let proxy = OutboundProxy::parse(&format!("http://127.0.0.1:{port}")).ok()?;
                Some((id, proxy, is_dynamic, !is_dynamic))
            })
            .collect::<Vec<_>>();
        lock(&self.pools.codex_managed).replace(exits.clone(), running, Instant::now());
        lock(&self.pools.managed).replace(exits, running, Instant::now());
    }
}
