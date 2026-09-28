//! Host-owned managed Mihomo lifecycle. Adapted from Sub2API 030c1fd7 (LGPL-3.0).

mod diagnostics;
mod dynamic;
mod files;
mod kernel;
mod pool;
mod sources;
mod state;
mod warm;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use futures::{StreamExt, stream};
use gateway_admin::{
    model::{AdminError, AdminErrorKind, MutationContext, mihomo::*},
    ports::mihomo::MihomoManagement,
};
use gateway_core::lifecycle::CancellationToken;
use state::{Saved, digest, node_id, node_name};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{Mutex, RwLock, Semaphore};

type ClientBuilder =
    dyn Fn(reqwest::ClientBuilder) -> Result<reqwest::Client, AdminError> + Send + Sync;

struct View {
    busy: bool,
    phase: String,
    error: Option<String>,
    checks: BTreeMap<String, MihomoNodeCheck>,
}

/// Management serialization is independent of the request-side exit pool.
#[derive(Clone)]
pub struct ManagedMihomo {
    dir: PathBuf,
    saved: Arc<RwLock<Saved>>,
    view: Arc<RwLock<View>>,
    kernel: Arc<Mutex<kernel::Kernel>>,
    mutation: Arc<Mutex<()>>,
    checks: Arc<Semaphore>,
    cancellation: CancellationToken,
    operation: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    direct: reqwest::Client,
    build_client: Arc<ClientBuilder>,
    pub pools: Arc<pool::ExitPools>,
    running: Arc<std::sync::atomic::AtomicBool>,
    warmer: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl ManagedMihomo {
    pub async fn open<E>(
        dir: PathBuf,
        cancellation: CancellationToken,
        build: impl Fn(reqwest::ClientBuilder) -> Result<reqwest::Client, E> + Send + Sync + 'static,
    ) -> Result<Self, AdminError> {
        let build_client: Arc<ClientBuilder> = Arc::new(move |builder| {
            build(builder)
                .map_err(|_| AdminError::internal("受管代理客户端创建失败，请检查证书信任配置"))
        });
        let direct = build_client(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(120))
                .connect_timeout(Duration::from_secs(10)),
        )?;
        let saved: Saved = match std::fs::read(dir.join("settings.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| AdminError::internal("受管代理配置损坏，未覆盖原文件"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved::default(),
            Err(_) => return Err(AdminError::internal("无法读取受管代理配置")),
        };
        let service = Self {
            kernel: Arc::new(Mutex::new(kernel::Kernel::new(dir.clone()))),
            dir,
            saved: Arc::new(RwLock::new(saved)),
            view: Arc::new(RwLock::new(View {
                busy: false,
                phase: "idle".into(),
                error: None,
                checks: BTreeMap::new(),
            })),
            mutation: Arc::new(Mutex::new(())),
            checks: Arc::new(Semaphore::new(4)),
            cancellation,
            operation: Arc::new(Mutex::new(None)),
            pools: Arc::default(),
            running: Arc::default(),
            warmer: Arc::new(Mutex::new(None)),
            direct,
            build_client,
        };
        if service.kernel.lock().await.installed()
            && !service.saved.read().await.nodes.is_empty()
            && kernel::Kernel::supported()
        {
            // Resume only previously installed state; never download a binary on startup.
            service
                .queue(MihomoCommand {
                    action: MihomoAction::Start,
                    target: String::new(),
                    name: String::new(),
                    subscriptions: Vec::new(),
                    dynamic_proxies: Vec::new(),
                    country_filter: None,
                    download_mode: None,
                })
                .await?;
        }
        Ok(service)
    }

    pub async fn shutdown(&self) {
        if let Some(task) = self.warmer.lock().await.take() {
            let _ = task.await;
        }
        if let Some(task) = self.operation.lock().await.take() {
            let _ = task.await;
        }
        self.kernel.lock().await.stop().await;
    }

    async fn queue(&self, command: MihomoCommand) -> Result<(), AdminError> {
        if self.cancellation.is_cancelled() {
            return Err(AdminError::new(AdminErrorKind::Unavailable, "服务正在退出"));
        }
        let guard = self
            .mutation
            .clone()
            .try_lock_owned()
            .map_err(|_| AdminError::new(AdminErrorKind::Conflict, "代理管理任务仍在运行"))?;
        // Validate input before acknowledging a background mutation.
        let mut next = self.saved.read().await.clone();
        sources::prepare(&mut next, &command)?;
        if command.action == MihomoAction::CountryFilter {
            next.country_filter = state::normalize_country(
                command
                    .country_filter
                    .clone()
                    .ok_or_else(|| AdminError::invalid("缺少地区规则"))?,
            )?;
        }
        if command.action == MihomoAction::DownloadMode {
            next.download_mode = command
                .download_mode
                .ok_or_else(|| AdminError::invalid("缺少订阅下载模式"))?;
        }
        let phase = format!("{:?}", command.action);
        {
            let mut view = self.view.write().await;
            view.busy = true;
            view.phase = phase;
            view.error = None;
        }
        let service = self.clone();
        let task = tokio::spawn(async move {
            let _guard = guard;
            // Once configuration activation starts, timeout/cancellation must complete
            // rollback instead of dropping the future between reload and persistence.
            let result = service.run(command, next).await;
            service.publish_exits().await;
            let mut view = service.view.write().await;
            view.busy = false;
            view.phase = if result.is_ok() {
                "completed"
            } else {
                "failed"
            }
            .into();
            view.error = result.err().map(|e| e.to_string());
        });
        if let Some(previous) = self.operation.lock().await.replace(task) {
            let _ = previous.await;
        }
        Ok(())
    }

    async fn run(&self, command: MihomoCommand, mut next: Saved) -> Result<(), AdminError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        files::prepare_dir(&self.dir)?;
        let mut kernel = self.kernel.lock().await;
        if command.action == MihomoAction::Stop {
            kernel.stop().await;
            return Ok(());
        }
        if command.action == MihomoAction::Install {
            return self.preflight(deadline, kernel.install(&self.direct)).await;
        }
        if !kernel.installed() {
            return Err(AdminError::invalid("请先安装Mihomo内核"));
        }
        if matches!(
            command.action,
            MihomoAction::SubscriptionRename | MihomoAction::DownloadMode
        ) {
            self.persist(&next)?;
            *self.saved.write().await = next;
            return Ok(());
        }
        let running = kernel.running();
        if sources::source_action(command.action) {
            let proxy = if running {
                Some((self.build_client)(
                    reqwest::Client::builder()
                        .no_proxy()
                        .proxy(
                            reqwest::Proxy::all(kernel::ENDPOINT)
                                .map_err(|_| AdminError::internal("订阅代理配置失败"))?,
                        )
                        .timeout(Duration::from_secs(120)),
                )?)
            } else {
                None
            };
            self.preflight(
                deadline,
                sources::resolve(&mut next, &self.direct, proxy.as_ref()),
            )
            .await?;
        }
        match command.action {
            MihomoAction::CountryScan | MihomoAction::CountryProbe => {
                self.preflight(deadline, self.scan(&mut next, &command.target))
                    .await?;
            }
            MihomoAction::Disable | MihomoAction::Recover | MihomoAction::Probe => {
                let node = next
                    .nodes
                    .iter()
                    .find(|n| node_name(n) == command.target)
                    .ok_or_else(|| AdminError::invalid("节点不存在"))?;
                let disabled = if command.action == MihomoAction::Disable {
                    Some("disabled")
                } else if command.action == MihomoAction::Probe
                    && !self
                        .preflight(
                            deadline,
                            diagnostics::probe(&self.dir, node, &*self.build_client),
                        )
                        .await?
                {
                    Some("failed")
                } else {
                    None
                };
                if let Some(state) = disabled {
                    next.disabled.insert(command.target, state.into());
                } else {
                    next.disabled.remove(&command.target);
                }
            }
            _ => {}
        }
        if self.cancellation.is_cancelled() {
            return Err(AdminError::new(
                AdminErrorKind::Unavailable,
                "代理操作已取消",
            ));
        }
        if next.secret.is_empty() {
            next.secret = files::secret()?;
        }
        let old = self.saved.read().await.clone();
        let config = kernel.config(&next)?;
        // Reserve identities for this process, including failed candidates.
        // HTTP clients cannot survive a process restart, so neither should tombstones.
        let path = self.preflight(deadline, kernel.validate(&config)).await?;
        let activation = if running {
            kernel::reload(&self.direct, &config, &old.secret).await
        } else {
            kernel.start(&path, &next.secret, &self.direct).await
        };
        if let Err(error) = activation {
            if running {
                let previous = kernel.config(&old)?;
                if kernel::reload(&self.direct, &previous, &old.secret)
                    .await
                    .is_err()
                {
                    kernel.stop().await;
                }
            } else {
                kernel.stop().await;
            }
            return Err(error);
        }
        if let Err(error) = self.persist(&next) {
            if running {
                let previous = kernel.config(&old)?;
                if kernel::reload(&self.direct, &previous, &old.secret)
                    .await
                    .is_err()
                {
                    kernel.stop().await;
                }
            } else {
                kernel.stop().await;
            }
            return Err(error);
        }
        *self.saved.write().await = next;
        Ok(())
    }

    async fn preflight<T>(
        &self,
        deadline: tokio::time::Instant,
        future: impl std::future::Future<Output = Result<T, AdminError>>,
    ) -> Result<T, AdminError> {
        tokio::select! {
            () = self.cancellation.cancelled() => Err(AdminError::new(AdminErrorKind::Unavailable,"代理操作已取消")),
            result = tokio::time::timeout_at(deadline, future) => result.map_err(|_| AdminError::internal("代理管理任务超时"))?,
        }
    }

    fn persist(&self, saved: &Saved) -> Result<(), AdminError> {
        files::atomic_write(
            &self.dir.join("settings.json"),
            &serde_json::to_vec(saved).map_err(|_| AdminError::internal("代理配置编码失败"))?,
            false,
        )
    }

    async fn scan(&self, next: &mut Saved, target: &str) -> Result<(), AdminError> {
        let dynamic = next.dynamic_ids();
        let mut nodes: Vec<_> = next
            .nodes
            .iter()
            .filter(|n| {
                !dynamic.contains(&node_id(n)) && (target.is_empty() || node_name(n) == target)
            })
            .cloned()
            .collect();
        if !target.is_empty() && nodes.is_empty() {
            return Err(AdminError::invalid(
                "节点不存在，或属于由供应商管理地区的动态出口",
            ));
        }
        nodes.sort_by_key(|n| next.countries.get(node_name(n)).and_then(|o| o.checked_at));
        nodes.truncate(20);
        let observations: Vec<_> = stream::iter(nodes.into_iter().map(|node| async move {
            let value = diagnostics::country(&self.dir, &node, &*self.build_client).await;
            (node_name(&node).to_owned(), value)
        }))
        .buffer_unordered(2)
        .collect()
        .await;
        next.countries.extend(observations);
        Ok(())
    }
}

#[async_trait]
impl MihomoManagement for ManagedMihomo {
    async fn status(&self) -> Result<MihomoStatus, AdminError> {
        // Never queue the UI behind network downloads or candidate validation.
        let kernel = self.kernel.try_lock().ok();
        let installed = self.dir.join("mihomo").is_file();
        let running = kernel
            .map(|mut k| k.running())
            .unwrap_or_else(|| self.running.load(std::sync::atomic::Ordering::Acquire));
        let saved = self.saved.read().await;
        let view = self.view.read().await;
        let dynamic = saved.dynamic_ids();
        let nodes = saved
            .nodes
            .iter()
            .map(|node| {
                let name = node_name(node);
                let is_dynamic = dynamic.contains(&node_id(node));
                let country = saved.countries.get(name);
                MihomoNode {
                    name: name.into(),
                    display_name: saved
                        .names
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| name.to_owned()),
                    dynamic: is_dynamic,
                    subscription_ids: saved
                        .subscriptions
                        .iter()
                        .filter(|s| {
                            s.cache
                                .as_ref()
                                .is_some_and(|c| c.nodes.iter().any(|n| node_name(n) == name))
                        })
                        .map(|s| digest(s.url.as_bytes()))
                        .collect(),
                    state: saved
                        .disabled
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| "ready".into()),
                    country_code: country.and_then(|c| c.code.clone()),
                    country_checked_at: country.and_then(|c| c.checked_at),
                    country_error: country.and_then(|c| c.error.clone()),
                    country_blocked: !saved.country_allowed(node, is_dynamic),
                    check: view.checks.get(name).cloned(),
                }
            })
            .collect();
        Ok(MihomoStatus {
            version: kernel::VERSION.into(),
            installed,
            running,
            supported: kernel::Kernel::supported(),
            busy: view.busy,
            phase: view.phase.clone(),
            error: view.error.clone(),
            endpoint: kernel::ENDPOINT.into(),
            subscription_download_mode: saved.download_mode,
            subscription_items: saved.subscriptions_view(),
            dynamic_proxies: saved.dynamic_proxies.len(),
            node_states: nodes,
            country_filter: saved.country_filter.clone(),
            country_codes: state::COUNTRY_CODES
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            bps_warm_pool: pool::lock(&self.pools.managed).status(std::time::Instant::now()),
            bps_ip_warm_pool: pool::lock(&self.pools.regular).status(std::time::Instant::now()),
            codex_warm_pool: pool::lock(&self.pools.codex_managed)
                .status(std::time::Instant::now()),
            codex_ip_warm_pool: pool::lock(&self.pools.codex_regular)
                .status(std::time::Instant::now()),
        })
    }

    async fn submit(
        &self,
        command: MihomoCommand,
        context: &MutationContext,
    ) -> Result<MihomoStatus, AdminError> {
        tracing::info!(action=?command.action, actor=?context.actor, request_id=%context.request_id, "managed proxy operation requested");
        self.queue(command).await?;
        self.status().await
    }

    async fn test_node(
        &self,
        name: &str,
        quality: bool,
        _context: &MutationContext,
    ) -> Result<MihomoNodeCheck, AdminError> {
        let node = self
            .saved
            .read()
            .await
            .nodes
            .iter()
            .find(|n| node_name(n) == name)
            .cloned()
            .ok_or_else(|| AdminError::invalid("节点不存在"))?;
        let _slot = self
            .checks
            .acquire()
            .await
            .map_err(|_| AdminError::internal("节点检测已停止"))?;
        let mut result = self
            .preflight(
                tokio::time::Instant::now() + Duration::from_secs(300),
                diagnostics::check(&self.dir, &node, quality, &*self.build_client),
            )
            .await?;
        if self
            .saved
            .read()
            .await
            .nodes
            .iter()
            .any(|n| node_id(n) == node_id(&node))
        {
            let mut view = self.view.write().await;
            if result.quality.is_none() {
                result.quality = view.checks.get(name).and_then(|c| c.quality.clone());
            }
            view.checks.insert(name.into(), result.clone());
        }
        Ok(result)
    }
}
