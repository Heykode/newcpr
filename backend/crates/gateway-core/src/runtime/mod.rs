//! 运行时快照的原子发布、健康状态与跨进程版本收敛。

mod account_concurrency;

pub use account_concurrency::{AccountConcurrencyHandle, AccountConcurrencySnapshot};

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex as SyncMutex, RwLock};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use futures::lock::Mutex;
use futures::{FutureExt as _, Stream, StreamExt as _, pin_mut, select_biased};
use futures_timer::Delay;

use crate::health::{HealthProbe, HealthState};
use crate::identity::ProviderKind;
use crate::lifecycle::CancellationToken;
use crate::routing::snapshot::{
    RuntimeSnapshot, RuntimeSnapshotCompileError, RuntimeSnapshotCompiler,
};
use crate::routing::{
    ConfigRevision, OpenAiTurnStatePolicy, ProviderCatalogGeneration, RequestTuning,
};
use crate::task::{
    DaemonRestartPolicy, DaemonTask, ScheduledTask, WorkerContribution, WorkerCycleContext,
    WorkerDefinitionError, WorkerId, WorkerKind, WorkerRegistration, WorkerRunnable,
    WorkerSchedule, WorkerTaskError,
};

const RECONCILIATION_INTERVAL: Duration = Duration::from_secs(5);
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAXIMUM_BACKOFF: Duration = Duration::from_secs(30);
const UNUSED_LEASE_TTL: Duration = Duration::from_secs(30);
const UNUSED_LEASE_RENEWAL: Duration = Duration::from_secs(10);
/// Temporary grace period for serving the last known-good runtime snapshot.
///
/// This is intentionally shorter than QX's 60-second settings cache. Account
/// limits affect new upstream capacity, so stale values should converge sooner.
pub const RUNTIME_SNAPSHOT_STALE_GRACE: Duration = Duration::from_secs(30);

/// 不泄漏订阅基础设施细节的通知错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("runtime snapshot notification is unavailable")]
pub struct SnapshotSubscriptionError;

impl SnapshotSubscriptionError {
    #[must_use]
    pub const fn unavailable() -> Self {
        Self
    }
}

/// 可丢失的配置 revision 通知流；权威 revision 始终由 Store 端口读取。
pub type SnapshotRevisionStream =
    Pin<Box<dyn Stream<Item = Result<ConfigRevision, SnapshotSubscriptionError>> + Send + 'static>>;

/// 跨进程 revision 通知的基础设施中立端口。
pub trait SnapshotSubscriptionPort: Send + Sync {
    fn publish_snapshot_revision(
        &self,
        revision: ConfigRevision,
    ) -> BoxFuture<'_, Result<(), SnapshotSubscriptionError>>;

    fn subscribe_snapshot_revisions(
        &self,
    ) -> BoxFuture<'_, Result<SnapshotRevisionStream, SnapshotSubscriptionError>>;
}

/// 请求级冻结失败；此状态必须 fail closed。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("runtime snapshot is unavailable")]
pub struct RuntimeSnapshotUnavailable;

/// RuntimeSnapshot 原子发布和请求级冻结句柄。
#[derive(Clone, Default)]
pub struct RuntimeSnapshotHandle {
    state: Arc<RwLock<RuntimeSnapshotState>>,
}

#[derive(Default)]
struct RuntimeSnapshotState {
    current: Option<Arc<RuntimeSnapshot>>,
    suspended_at: Option<Instant>,
}

/// Process-shared request tuning. Each request copies the value once.
#[derive(Clone, Default)]
pub struct RequestTuningHandle {
    current: Arc<RwLock<RequestTuning>>,
    openai_turn_state_policy: Arc<RwLock<OpenAiTurnStatePolicy>>,
    account_concurrency: AccountConcurrencyHandle,
}

impl RequestTuningHandle {
    #[must_use]
    pub fn new(initial: RequestTuning) -> Self {
        Self {
            current: Arc::new(RwLock::new(initial)),
            openai_turn_state_policy: Arc::new(RwLock::new(OpenAiTurnStatePolicy::default())),
            account_concurrency: AccountConcurrencyHandle::default(),
        }
    }

    #[must_use]
    pub fn load(&self) -> RequestTuning {
        *read_unpoisoned(&self.current)
    }

    pub fn publish(&self, tuning: RequestTuning) {
        *write_unpoisoned(&self.current) = tuning;
    }

    #[must_use]
    pub fn openai_turn_state_policy(&self) -> OpenAiTurnStatePolicy {
        read_unpoisoned(&self.openai_turn_state_policy).clone()
    }

    pub fn publish_openai_turn_state_policy(&self, policy: OpenAiTurnStatePolicy) {
        *write_unpoisoned(&self.openai_turn_state_policy) = policy;
    }

    #[must_use]
    pub fn account_concurrency(&self) -> AccountConcurrencyHandle {
        self.account_concurrency.clone()
    }
}

impl RuntimeSnapshotHandle {
    #[must_use]
    pub fn new(initial: RuntimeSnapshot) -> Self {
        Self {
            state: Arc::new(RwLock::new(RuntimeSnapshotState {
                current: Some(Arc::new(initial)),
                suspended_at: None,
            })),
        }
    }

    pub fn publish(&self, snapshot: RuntimeSnapshot) {
        let mut state = write_unpoisoned(&self.state);
        state.current = Some(Arc::new(snapshot));
        state.suspended_at = None;
    }

    pub fn suspend(&self) {
        let mut state = write_unpoisoned(&self.state);
        if state.current.is_some() {
            state.suspended_at.get_or_insert_with(Instant::now);
        }
    }

    fn is_suspended(&self) -> bool {
        read_unpoisoned(&self.state).suspended_at.is_some()
    }

    #[must_use]
    pub fn revision(&self) -> Option<ConfigRevision> {
        let state = read_unpoisoned(&self.state);
        state
            .is_available_at(Instant::now())
            .then(|| state.current.as_ref().map(|snapshot| snapshot.revision()))
            .flatten()
    }

    #[must_use]
    pub fn provider_catalog_generations(
        &self,
    ) -> Option<BTreeMap<ProviderKind, ProviderCatalogGeneration>> {
        let state = read_unpoisoned(&self.state);
        state
            .is_available_at(Instant::now())
            .then(|| {
                state
                    .current
                    .as_ref()
                    .map(|snapshot| snapshot.provider_catalog_generations().clone())
            })
            .flatten()
    }

    /// 冻结当前 Arc；后续发布不改变已经开始的请求。
    pub fn acquire(&self) -> Result<Arc<RuntimeSnapshot>, RuntimeSnapshotUnavailable> {
        let state = read_unpoisoned(&self.state);
        if !state.is_available_at(Instant::now()) {
            return Err(RuntimeSnapshotUnavailable);
        }
        state.current.clone().ok_or(RuntimeSnapshotUnavailable)
    }
}

impl RuntimeSnapshotState {
    fn is_available_at(&self, now: Instant) -> bool {
        self.current.is_some()
            && self
                .suspended_at
                .is_none_or(|started| now.duration_since(started) < RUNTIME_SNAPSHOT_STALE_GRACE)
    }
}

impl HealthProbe for RuntimeSnapshotHandle {
    fn name(&self) -> &'static str {
        "runtime_snapshot"
    }

    fn check(&self) -> BoxFuture<'_, HealthState> {
        Box::pin(async move {
            if self.revision().is_some() {
                HealthState::Healthy
            } else {
                HealthState::Unhealthy("Runtime snapshot is unavailable".to_owned())
            }
        })
    }
}

/// Admin 提交配置后触发本进程刷新与跨进程通知的对象安全端口。
pub trait SnapshotControl: Send + Sync {
    fn publish_committed(&self, committed_revision: ConfigRevision) -> BoxFuture<'_, ()>;
}

#[derive(Default)]
struct RefreshPriorityState {
    pending_commits: usize,
    active_background: Option<CancellationToken>,
}

struct CommittedRefreshGuard(Arc<SyncMutex<RefreshPriorityState>>);

impl CommittedRefreshGuard {
    fn begin(priority: Arc<SyncMutex<RefreshPriorityState>>) -> Self {
        let active = {
            let mut state = lock_unpoisoned(&priority);
            state.pending_commits += 1;
            state.active_background.clone()
        };
        if let Some(active) = active {
            active.cancel();
        }
        Self(priority)
    }
}

impl Drop for CommittedRefreshGuard {
    fn drop(&mut self) {
        lock_unpoisoned(&self.0).pending_commits -= 1;
    }
}

struct BackgroundRefreshGuard {
    priority: Arc<SyncMutex<RefreshPriorityState>>,
    cancellation: CancellationToken,
}

impl BackgroundRefreshGuard {
    fn begin(priority: Arc<SyncMutex<RefreshPriorityState>>) -> Option<Self> {
        let mut state = lock_unpoisoned(&priority);
        if state.pending_commits != 0 {
            return None;
        }
        let cancellation = CancellationToken::new();
        state.active_background = Some(cancellation.clone());
        drop(state);
        Some(Self {
            priority,
            cancellation,
        })
    }
}

impl Drop for BackgroundRefreshGuard {
    fn drop(&mut self) {
        lock_unpoisoned(&self.priority).active_background = None;
    }
}

/// 配置提交后的本进程快照发布与跨进程失效通知。
#[derive(Clone)]
pub struct RuntimeSnapshotPublisher {
    compiler: Arc<RuntimeSnapshotCompiler>,
    snapshots: RuntimeSnapshotHandle,
    request_tuning: RequestTuningHandle,
    subscriptions: Arc<dyn SnapshotSubscriptionPort>,
    refresh_lock: Arc<Mutex<()>>,
    refresh_priority: Arc<SyncMutex<RefreshPriorityState>>,
}

impl RuntimeSnapshotPublisher {
    #[must_use]
    pub fn new(
        compiler: Arc<RuntimeSnapshotCompiler>,
        snapshots: RuntimeSnapshotHandle,
        subscriptions: Arc<dyn SnapshotSubscriptionPort>,
    ) -> Self {
        Self::new_with_request_tuning(
            compiler,
            snapshots,
            RequestTuningHandle::default(),
            subscriptions,
        )
    }

    #[must_use]
    pub fn new_with_request_tuning(
        compiler: Arc<RuntimeSnapshotCompiler>,
        snapshots: RuntimeSnapshotHandle,
        request_tuning: RequestTuningHandle,
        subscriptions: Arc<dyn SnapshotSubscriptionPort>,
    ) -> Self {
        Self {
            compiler,
            snapshots,
            request_tuning,
            subscriptions,
            refresh_lock: Arc::new(Mutex::new(())),
            refresh_priority: Arc::new(SyncMutex::new(RefreshPriorityState::default())),
        }
    }

    /// 串行重编译并替换本进程快照；无法确认配置时暂停新请求。
    pub async fn refresh(&self) -> Result<ConfigRevision, RuntimeSnapshotCompileError> {
        // Serialize facts reads through publication or suspension; request reads skip this lock.
        let _refresh = self.refresh_lock.lock().await;
        let background = BackgroundRefreshGuard::begin(Arc::clone(&self.refresh_priority))
            .ok_or(RuntimeSnapshotCompileError::RevisionChanged)?;
        self.refresh_locked(true, Some(&background.cancellation))
            .await
    }

    async fn refresh_committed(&self) -> Result<ConfigRevision, RuntimeSnapshotCompileError> {
        let _committed = CommittedRefreshGuard::begin(Arc::clone(&self.refresh_priority));
        let _refresh = self.refresh_lock.lock().await;
        self.refresh_locked(true, None).await
    }

    async fn refresh_locked(
        &self,
        suspend_on_error: bool,
        cancellation: Option<&CancellationToken>,
    ) -> Result<ConfigRevision, RuntimeSnapshotCompileError> {
        let concurrency = &self.request_tuning.account_concurrency;
        let fence = Arc::clone(&read_unpoisoned(&concurrency.state).fence);
        let compiled = if let Some(cancellation) = cancellation {
            let cancelled = cancellation.cancelled().fuse();
            let compiled = self.compiler.compile().fuse();
            pin_mut!(cancelled, compiled);
            // Preemption is not a load failure and must not suspend published capacity.
            select_biased! {
                _ = cancelled => return Err(RuntimeSnapshotCompileError::RevisionChanged),
                result = compiled => result,
            }
        } else {
            let previous = read_unpoisoned(&self.snapshots.state).current.clone();
            match previous {
                Some(previous) => self.compiler.compile_with_cached_catalog(&previous).await,
                None => self.compiler.compile().await,
            }
        };
        // Keep publication and explicit suspension ordered without holding a sync lock over I/O.
        let mut state = write_unpoisoned(&concurrency.state);
        if !Arc::ptr_eq(&fence, &state.fence) {
            return Err(RuntimeSnapshotCompileError::RevisionChanged);
        }
        let compiled = compiled.and_then(|snapshot| {
            if state.publish(snapshot.account_concurrency()) {
                Ok(snapshot)
            } else {
                Err(RuntimeSnapshotCompileError::RevisionChanged)
            }
        });
        let snapshot = match compiled {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if suspend_on_error {
                    state.suspend();
                    self.snapshots.suspend();
                }
                return Err(error);
            }
        };
        let revision = snapshot.revision();
        self.request_tuning.publish(snapshot.request_tuning());
        self.request_tuning
            .publish_openai_turn_state_policy(snapshot.openai_turn_state_policy().clone());
        self.snapshots.publish(snapshot);
        Ok(revision)
    }

    pub fn suspend(&self) {
        let mut state = write_unpoisoned(&self.request_tuning.account_concurrency.state);
        state.suspend();
        self.snapshots.suspend();
    }

    #[must_use]
    pub fn published_revision(&self) -> Option<ConfigRevision> {
        self.snapshots.revision()
    }

    #[must_use]
    fn provider_catalogs_need_refresh(&self) -> bool {
        self.snapshots.provider_catalog_generations().as_ref()
            != Some(&self.compiler.provider_catalog_generations())
    }

    /// 数据库提交不能被目录或通知基础设施的暂时故障伪装成回滚。
    async fn publish_committed_inner(&self, committed_revision: ConfigRevision) {
        let _ = self.refresh_committed().await;
        let _ = self
            .subscriptions
            .publish_snapshot_revision(committed_revision)
            .await;
    }

    /// 交给 Host 的周期对账与长驻订阅任务。
    pub fn worker_contributions(&self) -> Result<Vec<WorkerContribution>, WorkerDefinitionError> {
        let reconciliation_id = WorkerId::try_new(
            WorkerKind::RuntimeSnapshotReconciliation,
            "runtime_snapshot",
        )?;
        let schedule = WorkerSchedule::try_new(
            RECONCILIATION_INTERVAL,
            INITIAL_BACKOFF,
            MAXIMUM_BACKOFF,
            UNUSED_LEASE_TTL,
            UNUSED_LEASE_RENEWAL,
        )?;
        let reconciliation = WorkerRegistration::try_new(
            reconciliation_id,
            WorkerRunnable::Scheduled {
                schedule,
                lease: None,
                task: Box::new(RuntimeSnapshotReconciliationTask {
                    publisher: self.clone(),
                }),
            },
        )?;
        let subscription_id =
            WorkerId::try_new(WorkerKind::RuntimeChangeSubscription, "runtime_snapshot")?;
        let restart = DaemonRestartPolicy::try_new(INITIAL_BACKOFF, MAXIMUM_BACKOFF)?;
        let subscription = WorkerRegistration::try_new(
            subscription_id,
            WorkerRunnable::Daemon {
                restart,
                task: Box::new(RuntimeSnapshotSubscriptionTask {
                    subscriptions: Arc::clone(&self.subscriptions),
                    publisher: self.clone(),
                }),
            },
        )?;
        Ok(vec![
            WorkerContribution::Registration(reconciliation),
            WorkerContribution::Registration(subscription),
        ])
    }

    async fn reconcile(&self) -> Result<(), WorkerTaskError> {
        let _refresh = self.refresh_lock.lock().await;
        let background = BackgroundRefreshGuard::begin(Arc::clone(&self.refresh_priority))
            .ok_or_else(|| WorkerTaskError::safe("runtime snapshot commit is pending"))?;
        let persisted_revision = match self.compiler.store().current_config_revision().await {
            Ok(revision) => revision,
            Err(_) => {
                self.suspend();
                return Err(WorkerTaskError::safe(
                    "runtime snapshot revision is unavailable",
                ));
            }
        };
        let configuration_changed = runtime_revision_needs_refresh(
            self.published_revision().map(ConfigRevision::get),
            persisted_revision.get(),
        );
        // A readable stale snapshot still needs a successful reload to cancel its grace deadline.
        let needs_recovery = self.snapshots.is_suspended()
            || read_unpoisoned(&self.request_tuning.account_concurrency.state).is_suspended();
        if !configuration_changed && !needs_recovery && !self.provider_catalogs_need_refresh() {
            return Ok(());
        }
        // A catalog-only failure can retain the previous immutable request snapshot.
        self.refresh_locked(
            configuration_changed || needs_recovery,
            Some(&background.cancellation),
        )
        .await
        .map_err(|_| WorkerTaskError::safe("runtime snapshot reconciliation failed"))?;
        Ok(())
    }
}

impl SnapshotControl for RuntimeSnapshotPublisher {
    fn publish_committed(&self, committed_revision: ConfigRevision) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.publish_committed_inner(committed_revision).await;
        })
    }
}

struct RuntimeSnapshotReconciliationTask {
    publisher: RuntimeSnapshotPublisher,
}

impl ScheduledTask for RuntimeSnapshotReconciliationTask {
    fn run_cycle(
        &self,
        _context: WorkerCycleContext,
    ) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move { self.publisher.reconcile().await })
    }
}

struct RuntimeSnapshotSubscriptionTask {
    subscriptions: Arc<dyn SnapshotSubscriptionPort>,
    publisher: RuntimeSnapshotPublisher,
}

impl DaemonTask for RuntimeSnapshotSubscriptionTask {
    fn run(&self, cancellation: CancellationToken) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let mut retry_delay = INITIAL_BACKOFF;
            loop {
                if cancellation.is_cancelled() {
                    return Ok(());
                }
                let subscription = self.subscriptions.subscribe_snapshot_revisions().await;
                let mut subscription = match subscription {
                    Ok(subscription) => {
                        retry_delay = INITIAL_BACKOFF;
                        subscription
                    }
                    Err(_) => {
                        wait_or_cancel(&cancellation, retry_delay).await;
                        retry_delay = (retry_delay * 2).min(MAXIMUM_BACKOFF);
                        continue;
                    }
                };
                loop {
                    let cancelled = cancellation.cancelled().fuse();
                    let next = subscription.next().fuse();
                    pin_mut!(cancelled, next);
                    let notified = select_biased! {
                        _ = cancelled => return Ok(()),
                        next = next => next,
                    };
                    match notified {
                        Some(Ok(_)) => {
                            let _ = self.publisher.refresh_committed().await;
                        }
                        Some(Err(_)) | None => break,
                    }
                }
            }
        })
    }
}

async fn wait_or_cancel(cancellation: &CancellationToken, duration: Duration) {
    let cancelled = cancellation.cancelled().fuse();
    let delay = Delay::new(duration).fuse();
    pin_mut!(cancelled, delay);
    select_biased! {
        _ = cancelled => {},
        _ = delay => {},
    }
}

/// 当前发布版本与持久版本不一致时必须重载；缺失和回退同样 fail closed。
#[must_use]
pub fn runtime_revision_needs_refresh(
    published_revision: Option<u64>,
    persisted_revision: u64,
) -> bool {
    published_revision != Some(persisted_revision)
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_unpoisoned<T>(lock: &SyncMutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
