//! OpenAI Provider 向 Host 贡献的后台 worker。

use super::*;
use crate::transport::profile::cli_release::CliReleaseService;
use crate::transport::profile::platform_release::PlatformDesktopReleaseService;

pub(crate) struct ClientReleaseServices {
    pub desktop: Arc<CodexDesktopReleaseService>,
    pub cli: Arc<CliReleaseService>,
    pub platforms: Arc<PlatformDesktopReleaseService>,
}

use gateway_core::time::DeploymentTimeZone;

pub(super) const WORKER_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
pub(super) const WORKER_MAXIMUM_BACKOFF: Duration = Duration::from_secs(60);
pub(super) const WORKER_LEASE_TTL: Duration = Duration::from_secs(15 * 60);
pub(super) const WORKER_LEASE_RENEWAL: Duration = Duration::from_secs(5 * 60);
pub(super) const OAUTH_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
pub(super) const QUOTA_CHECK_INTERVAL: Duration = Duration::from_secs(30);
pub(super) const DESKTOP_RELEASE_WORKER_OWNER: &str = "openai-desktop-release";
pub(super) const MODEL_ETAG_WORKER_OWNER: &str = "openai-model-etag";
pub(super) const MODEL_CATALOG_WORKER_OWNER: &str = "openai-model-catalog";
pub(super) const WARMUP_WORKER_OWNER: &str = "openai-account-warmup";
pub(super) const WARMUP_CHECK_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) fn worker_contributions(
    refresh: Arc<CodexCredentialRefreshService>,
    quota: Arc<CodexCredentialQuotaService>,
    catalog: Arc<CodexCredentialCatalogService>,
    quota_refresh_policy: CodexQuotaRefreshPolicy,
    oauth_refresh_enabled: bool,
    releases: ClientReleaseServices,
    timezone: DeploymentTimeZone,
) -> Result<Vec<WorkerContribution>, WorkerDefinitionError> {
    let refresh_id = WorkerId::try_new(WorkerKind::OAuthRefresh, PROVIDER_NAME)?;
    let quota_id = WorkerId::try_new(WorkerKind::QuotaCatalogHealth, PROVIDER_NAME)?;
    let catalog_id = WorkerId::try_new(WorkerKind::QuotaCatalogHealth, MODEL_CATALOG_WORKER_OWNER)?;
    let etag_id = WorkerId::try_new(WorkerKind::QuotaCatalogHealth, MODEL_ETAG_WORKER_OWNER)?;
    let desktop_release_id =
        WorkerId::try_new(WorkerKind::QuotaCatalogHealth, DESKTOP_RELEASE_WORKER_OWNER)?;
    let warmup_id = WorkerId::try_new(WorkerKind::QuotaCatalogHealth, WARMUP_WORKER_OWNER)?;
    let mut contributions = Vec::new();
    if oauth_refresh_enabled {
        contributions.push(WorkerContribution::Registration(scheduled_registration(
            refresh_id,
            OAUTH_REFRESH_INTERVAL,
            Box::new(OpenAiOAuthRefreshTask {
                service: refresh,
                catalog: Arc::clone(&catalog),
            }),
        )?));
    }
    contributions.extend([
        WorkerContribution::Registration(profile_registration(
            WorkerId::try_new(
                WorkerKind::QuotaCatalogHealth,
                "openai-platform-desktop-release",
            )?,
            APPCAST_POLL_INTERVAL,
            Box::new(OpenAiPlatformDesktopReleaseTask {
                service: releases.platforms,
            }),
        )?),
        WorkerContribution::Registration(profile_registration(
            WorkerId::try_new(WorkerKind::QuotaCatalogHealth, "openai-cli-release")?,
            APPCAST_POLL_INTERVAL,
            Box::new(OpenAiCliReleaseTask {
                service: releases.cli,
            }),
        )?),
        WorkerContribution::Registration(scheduled_registration(
            quota_id,
            QUOTA_CHECK_INTERVAL,
            Box::new(OpenAiQuotaTask {
                quota: Arc::clone(&quota),
            }),
        )?),
        WorkerContribution::Registration(scheduled_registration(
            warmup_id,
            WARMUP_CHECK_INTERVAL,
            Box::new(OpenAiWarmupTask { quota, timezone }),
        )?),
        WorkerContribution::Registration(scheduled_registration(
            catalog_id,
            quota_refresh_policy.interval(),
            Box::new(OpenAiCatalogTask {
                catalog: Arc::clone(&catalog),
                success_tail_jitter: Duration::from_millis(
                    u64::try_from(quota_refresh_policy.interval().as_millis() / 5)
                        .unwrap_or_default(),
                ),
            }),
        )?),
        WorkerContribution::Registration(WorkerRegistration::try_new(
            etag_id,
            WorkerRunnable::Daemon {
                restart: DaemonRestartPolicy::try_new(
                    WORKER_INITIAL_BACKOFF,
                    WORKER_MAXIMUM_BACKOFF,
                )?,
                task: Box::new(OpenAiCatalogEtagTask { catalog }),
            },
        )?),
        WorkerContribution::Registration(scheduled_registration(
            desktop_release_id,
            APPCAST_POLL_INTERVAL,
            Box::new(OpenAiDesktopReleaseTask {
                service: releases.desktop,
            }),
        )?),
    ]);
    Ok(contributions)
}

pub(super) fn scheduled_registration(
    id: WorkerId,
    interval: Duration,
    task: Box<dyn ScheduledTask>,
) -> Result<WorkerRegistration, WorkerDefinitionError> {
    let schedule = WorkerSchedule::try_new(
        interval,
        WORKER_INITIAL_BACKOFF,
        WORKER_MAXIMUM_BACKOFF,
        WORKER_LEASE_TTL,
        WORKER_LEASE_RENEWAL,
    )?;
    let lease = WorkerLeaseRequest::try_new(id.clone(), WORKER_LEASE_TTL)?;
    WorkerRegistration::try_new(
        id,
        WorkerRunnable::Scheduled {
            schedule,
            lease: Some(lease),
            task,
        },
    )
}

fn profile_registration(
    id: WorkerId,
    interval: Duration,
    task: Box<dyn ScheduledTask>,
) -> Result<WorkerRegistration, WorkerDefinitionError> {
    let mut registration = scheduled_registration(id, interval, task)?;
    if let WorkerRunnable::Scheduled { lease, .. } = &mut registration.runnable {
        *lease = None;
    }
    Ok(registration)
}

pub(super) struct OpenAiOAuthRefreshTask {
    catalog: Arc<CodexCredentialCatalogService>,
    service: Arc<CodexCredentialRefreshService>,
}

impl ScheduledTask for OpenAiOAuthRefreshTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            if context.cancellation().is_cancelled() {
                return Ok(());
            }
            let outcomes = self.service.refresh_due().await.map_err(|error| {
                tracing::error!(error = %error, "OpenAI OAuth refresh cycle failed");
                WorkerTaskError::safe("OpenAI OAuth refresh failed").with_source(error)
            })?;
            let mut changed = Vec::new();
            let mut refreshed = 0_u64;
            let mut invalidated = 0_u64;
            let mut banned = 0_u64;
            let mut transient = 0_u64;
            let mut lease_unavailable = 0_u64;
            let mut stale = 0_u64;
            let mut failed = 0_u64;
            let mut transient_accounts = Vec::new();
            let mut failed_accounts = Vec::new();
            for outcome in &outcomes {
                match outcome {
                    CodexCredentialRefreshOutcome::Refreshed { account_id, .. } => {
                        refreshed += 1;
                        changed.push(account_id);
                    }
                    CodexCredentialRefreshOutcome::Invalidated { account_id } => {
                        invalidated += 1;
                        changed.push(account_id);
                    }
                    CodexCredentialRefreshOutcome::Banned { account_id } => {
                        banned += 1;
                        changed.push(account_id);
                    }
                    CodexCredentialRefreshOutcome::Transient { account_id } => {
                        transient += 1;
                        transient_accounts.push(account_id);
                    }
                    CodexCredentialRefreshOutcome::LeaseUnavailable { .. } => {
                        lease_unavailable += 1;
                    }
                    CodexCredentialRefreshOutcome::Stale { .. } => stale += 1,
                    CodexCredentialRefreshOutcome::Failed { account_id } => {
                        failed += 1;
                        failed_accounts.push(account_id);
                    }
                }
            }
            if !changed.is_empty() {
                let ids = changed
                    .into_iter()
                    .map(|id| gateway_core::account::ProviderAccountId::new(id.as_str()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        WorkerTaskError::safe("OpenAI 账号 ID 无效").with_source(error)
                    })?;
                self.catalog
                    .invalidate_accounts(&ids)
                    .await
                    .map_err(|error| {
                        WorkerTaskError::safe("OpenAI 凭据变更后目录失效失败").with_source(error)
                    })?;
            }
            if !outcomes.is_empty() {
                tracing::info!(
                    refreshed,
                    invalidated,
                    banned,
                    transient,
                    lease_unavailable,
                    stale,
                    failed,
                    "OpenAI OAuth refresh cycle completed"
                );
            }
            if transient > 0 || failed > 0 {
                tracing::warn!(
                    refreshed,
                    invalidated,
                    banned,
                    transient,
                    lease_unavailable,
                    stale,
                    failed,
                    transient_accounts = ?transient_accounts,
                    failed_accounts = ?failed_accounts,
                    "OpenAI OAuth refresh cycle contained operational failures"
                );
            }
            Ok(())
        })
    }
}

pub(super) struct OpenAiQuotaTask {
    quota: Arc<CodexCredentialQuotaService>,
}

pub(super) struct OpenAiWarmupTask {
    quota: Arc<CodexCredentialQuotaService>,
    timezone: DeploymentTimeZone,
}

pub(super) struct OpenAiCatalogTask {
    catalog: Arc<CodexCredentialCatalogService>,
    success_tail_jitter: Duration,
}

pub(super) struct OpenAiCatalogEtagTask {
    catalog: Arc<CodexCredentialCatalogService>,
}

pub(super) struct OpenAiDesktopReleaseTask {
    service: Arc<CodexDesktopReleaseService>,
}

impl ScheduledTask for OpenAiDesktopReleaseTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let refresh = self.service.refresh();
            tokio::pin!(refresh);
            let result = tokio::select! {
                () = context.cancellation().cancelled() => return Ok(()),
                result = &mut refresh => result,
            };
            if let Err(error) = result {
                // 上游检查失败已经作为 Provider 观察事实保存；本周期本身正常完成，
                // 避免 Host 的短退避持续请求固定官方 appcast。
                tracing::warn!(error = %error, "OpenAI Desktop release check failed");
            }
            Ok(())
        })
    }
}

impl ScheduledTask for OpenAiQuotaTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            if context.cancellation().is_cancelled() {
                return Ok(());
            }
            match self.quota.synchronize().await {
                Ok(summary) if summary.has_operational_failures() => {
                    tracing::warn!(
                        updated = summary.updated,
                        exhausted = summary.exhausted,
                        banned = summary.banned,
                        transient = summary.transient,
                        stale = summary.stale,
                        "OpenAI quota cycle contained operational failures"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "OpenAI quota synchronization failed"
                    );
                    return Err(WorkerTaskError::safe("OpenAI quota synchronization failed")
                        .with_source(error));
                }
            }
            Ok(())
        })
    }
}

impl ScheduledTask for OpenAiWarmupTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let cron_result = self.run_cron_cycle(&context).await;
            let bucket_result = tokio::select! {
                () = context.cancellation().cancelled() => return Ok(()),
                result = self.quota.execute_bucket_warmup_at(chrono::Utc::now()) => result,
            };
            if let Err(error) = bucket_result {
                tracing::warn!(error = %error, "OpenAI quota bucket warmup cycle failed");
            }
            cron_result
        })
    }
}

impl OpenAiWarmupTask {
    async fn run_cron_cycle(&self, context: &WorkerCycleContext) -> Result<(), WorkerTaskError> {
        if context.cancellation().is_cancelled() {
            return Ok(());
        }
        let Some(runtime_policy) = self.quota.runtime_policy() else {
            return Ok(());
        };
        let policy = runtime_policy.load_warmup_policy().await.map_err(|error| {
            WorkerTaskError::safe("OpenAI warmup policy load failed").with_source(error)
        })?;
        if !policy.enabled() {
            return Ok(());
        }
        use chrono::Timelike as _;
        let now = chrono::Utc::now();
        let local_now = self.timezone.local(now);
        // 回拨产生的第二个相同时刻不能再次执行
        if self
            .timezone
            .resolve_local(local_now.naive_local())
            .is_none_or(|first| first != now)
        {
            return Ok(());
        }
        let slot = local_now
            .naive_local()
            .with_second(0)
            .and_then(|value| value.with_nanosecond(0))
            .ok_or_else(|| WorkerTaskError::safe("invalid warmup slot"))?;
        if !policy.matches_slot(slot) {
            return Ok(());
        }
        let Some(model) = policy.model() else {
            return Err(WorkerTaskError::safe("OpenAI warmup model is missing"));
        };
        if !runtime_policy
            .claim_warmup_slot(self.timezone, slot)
            .await
            .map_err(|source| {
                WorkerTaskError::safe("warmup slot is unavailable").with_source(source)
            })?
        {
            return Ok(());
        }
        tracing::info!(%slot, model, "OpenAI account warmup cycle started");
        let outcome = tokio::select! {
            () = context.cancellation().cancelled() => return Ok(()),
            outcome = self.quota.execute_warmup(model) => outcome,
        };
        match outcome {
            Ok(summary) => {
                tracing::info!(
                    warmed_up = summary.warmed_up,
                    skipped_active = summary.skipped_active,
                    skipped_exhausted = summary.skipped_exhausted,
                    failed = summary.failed,
                    "OpenAI account warmup cycle completed"
                );
            }
            Err(error) => {
                tracing::warn!(error = %error, "OpenAI account warmup cycle failed");
            }
        }
        Ok(())
    }
}

impl ScheduledTask for OpenAiCatalogTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            if context.cancellation().is_cancelled() {
                return Ok(());
            }
            match self.catalog.refresh_catalogs().await {
                Ok(_) => {
                    let delay = random_duration(self.success_tail_jitter);
                    tokio::select! {
                        () = context.cancellation().cancelled() => {}
                        () = tokio::time::sleep(delay) => {}
                    }
                    Ok(())
                }
                Err(CodexCredentialCatalogError::NoEligibleCredential) => Ok(()),
                Err(error) => {
                    tracing::warn!(error = %error, "OpenAI model catalog refresh failed");
                    Err(
                        WorkerTaskError::safe("OpenAI model catalog synchronization failed")
                            .with_source(error),
                    )
                }
            }
        })
    }
}

fn random_duration(max_exclusive: Duration) -> Duration {
    let max_millis = u64::try_from(max_exclusive.as_millis()).unwrap_or(u64::MAX);
    if max_millis == 0 {
        return Duration::ZERO;
    }
    let mut random = [0_u8; 8];
    let offset = if getrandom::fill(&mut random).is_ok() {
        u64::from_le_bytes(random) % max_millis
    } else {
        0
    };
    Duration::from_millis(offset)
}

impl DaemonTask for OpenAiCatalogEtagTask {
    fn run(
        &self,
        context: gateway_core::task::WorkerDaemonContext,
    ) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let cancellation = context.cancellation();
            loop {
                tokio::select! {
                    () = cancellation.cancelled() => return Ok(()),
                    () = self.catalog.wait_for_refresh() => {},
                };
                let result = tokio::select! {
                    () = cancellation.cancelled() => return Ok(()),
                    result = self.catalog.refresh_catalogs() => result.map(|_| ()),
                };
                match result {
                    Ok(_) | Err(CodexCredentialCatalogError::NoEligibleCredential) => {}
                    Err(error) => {
                        self.catalog.schedule_refresh();
                        return Err(
                            WorkerTaskError::safe("OpenAI 模型目录刷新失败").with_source(error)
                        );
                    }
                }
                context.succeeded();
            }
        })
    }
}

struct OpenAiCliReleaseTask {
    service: Arc<CliReleaseService>,
}

impl ScheduledTask for OpenAiCliReleaseTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            tokio::select! {
                () = context.cancellation().cancelled() => {},
                result = async { self.service.restore().await; self.service.refresh().await } => {
                    if let Err(error) = result { tracing::warn!(error = %error, "OpenAI CLI release check failed"); }
                }
            }
            Ok(())
        })
    }
}

struct OpenAiPlatformDesktopReleaseTask {
    service: Arc<PlatformDesktopReleaseService>,
}
impl ScheduledTask for OpenAiPlatformDesktopReleaseTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            tokio::select! {
                () = context.cancellation().cancelled() => {},
                () = async { self.service.restore().await; self.service.refresh().await } => {},
            }
            Ok(())
        })
    }
}
