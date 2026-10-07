//! OpenAI Provider 向 Host 贡献的后台 worker。

use super::*;
use chrono::{Timelike as _, Utc};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EtagRefreshBackoff {
    initial: Duration,
    current: Duration,
    maximum: Duration,
}

impl EtagRefreshBackoff {
    const fn new(initial: Duration, maximum: Duration) -> Self {
        Self {
            initial,
            current: initial,
            maximum,
        }
    }

    fn take_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = self
            .current
            .checked_mul(2)
            .unwrap_or(self.maximum)
            .min(self.maximum);
        delay
    }

    const fn reset(&mut self) {
        self.current = self.initial;
    }
}

pub(crate) fn worker_contributions(
    refresh: Arc<CodexCredentialRefreshService>,
    quota: Arc<CodexCredentialQuotaService>,
    catalog: Arc<CodexCredentialCatalogService>,
    quota_refresh_policy: CodexQuotaRefreshPolicy,
    oauth_refresh_enabled: bool,
    desktop_release: Arc<CodexDesktopReleaseService>,
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
            Box::new(OpenAiOAuthRefreshTask { service: refresh }),
        )?));
    }
    contributions.extend([
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
                service: desktop_release,
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

pub(super) struct OpenAiOAuthRefreshTask {
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
                    CodexCredentialRefreshOutcome::Refreshed { .. } => refreshed += 1,
                    CodexCredentialRefreshOutcome::Invalidated { .. } => invalidated += 1,
                    CodexCredentialRefreshOutcome::Banned { .. } => banned += 1,
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
            if context.cancellation().is_cancelled() {
                return Ok(());
            }
            let Some(runtime_policy) = self.quota.runtime_policy() else {
                return Ok(());
            };
            let policy = runtime_policy.load_warmup_policy().await.map_err(|error| {
                tracing::warn!(error = %error, "OpenAI warmup policy load failed");
                WorkerTaskError::safe("OpenAI warmup policy load failed")
            })?;
            if !policy.enabled() {
                return Ok(());
            }
            let local_now = self.timezone.local(Utc::now());
            let local_slot = local_now
                .naive_local()
                .with_second(0)
                .and_then(|value| value.with_nanosecond(0));
            let Some(local_slot) = local_slot else {
                return Err(WorkerTaskError::safe("OpenAI warmup local time is invalid"));
            };
            let scheduled = policy
                .scheduled_times()
                .into_iter()
                .any(|(hour, minute)| hour == local_slot.hour() && minute == local_slot.minute());
            if !scheduled {
                return Ok(());
            }
            let claimed = runtime_policy
                .claim_warmup_slot(self.timezone, local_slot)
                .await
                .map_err(|error| {
                    tracing::warn!(error = %error, "OpenAI warmup slot claim failed");
                    WorkerTaskError::safe("OpenAI warmup slot claim failed")
                })?;
            if !claimed {
                return Ok(());
            }
            let Some(model) = policy.model() else {
                return Err(WorkerTaskError::safe("OpenAI warmup model is missing"));
            };
            tracing::info!(
                timezone = self.timezone.name(),
                model,
                "OpenAI account warmup cycle started"
            );
            tokio::select! {
                () = context.cancellation().cancelled() => Ok(()),
                result = self.quota.execute_warmup(model) => {
                    match result {
                        Ok(summary) => {
                            tracing::info!(
                                warmed_up = summary.warmed_up,
                                skipped_active = summary.skipped_active,
                                skipped_exhausted = summary.skipped_exhausted,
                                failed = summary.failed,
                                "OpenAI account warmup cycle completed"
                            );
                            Ok(())
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, "OpenAI account warmup cycle failed");
                            Err(WorkerTaskError::safe("OpenAI account warmup cycle failed"))
                        }
                    }
                }
            }
        })
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
        cancellation: gateway_core::lifecycle::CancellationToken,
    ) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let mut backoff =
                EtagRefreshBackoff::new(WORKER_INITIAL_BACKOFF, WORKER_MAXIMUM_BACKOFF);
            let mut consecutive_failures = 0_u32;
            loop {
                tokio::select! {
                    () = cancellation.cancelled() => return Ok(()),
                    () = self.catalog.wait_for_etag_refresh() => {},
                };
                let result = tokio::select! {
                    () = cancellation.cancelled() => return Ok(()),
                    result = self.catalog.refresh() => result.map(|_| ()),
                };
                match result {
                    Ok(()) => {
                        if consecutive_failures > 0 {
                            tracing::info!(
                                failures = consecutive_failures,
                                "OpenAI model catalog ETag refresh recovered"
                            );
                        }
                        consecutive_failures = 0;
                        backoff.reset();
                    }
                    Err(error) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        let delay = backoff.take_delay();
                        tracing::warn!(
                            error = %error,
                            failures = consecutive_failures,
                            retry_after_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                            "OpenAI model catalog ETag refresh failed; retrying after backoff"
                        );
                        tokio::select! {
                            () = cancellation.cancelled() => return Ok(()),
                            () = tokio::time::sleep(delay) => {},
                        }
                    }
                }
            }
        })
    }
}
