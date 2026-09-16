//! Shared background samples. Page reads never initiate prediction or upstream work.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chrono::Utc;
use futures::{StreamExt as _, stream};
use gateway_core::{
    account::{AccountStatus, ProviderAccountId, resolve_account_status},
    routing::AccountGroupId,
};
use tokio::sync::{Mutex, Semaphore};

use crate::{
    model::{
        AdminError,
        group_monitor::{GroupMonitorReport, MonitorAccountEstimate, project_group_monitor},
        provider_credentials::AccountUsagePeriod,
        quota_forecast::current_window_estimate,
    },
    ports::store::{AccountGroupStore, AccountRuntimeStore},
};

use super::{accounts::AccountsService, map_store_error};

const SAMPLE_TIMEOUT: Duration = Duration::from_secs(60);

#[async_trait]
pub trait GroupMonitorService: Send + Sync {
    async fn sample(&self) -> Result<(), AdminError>;

    async fn read(
        &self,
        group_ids: Vec<AccountGroupId>,
        refresh_forecasts: bool,
    ) -> Result<GroupMonitorReport, AdminError>;
}

pub(crate) struct DefaultGroupMonitorService {
    groups: Arc<dyn AccountGroupStore>,
    runtime: Arc<dyn AccountRuntimeStore>,
    accounts: Arc<dyn AccountsService>,
    last_sample: Mutex<Option<(Instant, Result<(), AdminError>)>>,
    readers: Semaphore,
}

impl DefaultGroupMonitorService {
    pub(crate) fn new(
        groups: Arc<dyn AccountGroupStore>,
        runtime: Arc<dyn AccountRuntimeStore>,
        accounts: Arc<dyn AccountsService>,
    ) -> Self {
        Self {
            groups,
            runtime,
            accounts,
            last_sample: Mutex::new(None),
            readers: Semaphore::new(2),
        }
    }

    async fn collect_and_save(&self) -> Result<(), AdminError> {
        let now = Utc::now();
        let facts = self
            .groups
            .load_group_monitor(now)
            .await
            .map_err(|error| map_store_error(error, "group monitor"))?;
        let ids = facts
            .members
            .iter()
            .map(|fact| fact.member.account_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let runtime = self
            .runtime
            .account_runtime(&ids)
            .await
            .map_err(|error| map_store_error(error, "group monitor runtime"))?;
        let mut estimates = BTreeMap::new();
        let mut eligible_ids = BTreeSet::new();
        for fact in &facts.members {
            let id = &fact.member.account_id;
            let mut status = fact.member.status.clone();
            status.rate_limited_until = runtime.rate_limited_until.get(id).copied().map(Into::into);
            let eligible =
                resolve_account_status(&status, now.into()).status == AccountStatus::Normal;
            let lifespan = fact.plan.as_deref().and_then(|plan| {
                facts.lifespans.iter().find(|life| {
                    life.provider == fact.provider && life.plan.eq_ignore_ascii_case(plan)
                })
            });
            let remaining_life = lifespan
                .map(|life| life.minutes - (now - fact.first_seen_at).num_seconds() as f64 / 60.0);
            estimates
                .entry(id.clone())
                .or_insert_with(|| MonitorAccountEstimate {
                    id: id.clone(),
                    eligible,
                    used_slots: runtime
                        .in_flight
                        .as_ref()
                        .map(|slots| slots.get(id).copied().unwrap_or(0)),
                    total_slots: fact.member.total_slots,
                    remaining_usd: None,
                    incomplete: false,
                    unavailable: false,
                    low_sample: lifespan.is_some_and(|life| life.samples < 5),
                    reset_at: None,
                    consumption: facts.account_usage.get(id).cloned().unwrap_or_default(),
                    remaining_life_minutes: remaining_life,
                });
            if eligible {
                eligible_ids.insert(id.clone());
            }
        }
        // Deduplicate across the entire pool, not just one browser page.
        let loaded = stream::iter(eligible_ids)
            .map(|id| async move {
                let account_id = ProviderAccountId::new(id.clone())
                    .map_err(|_| AdminError::internal("监控账号 ID 不合法"))?;
                self.accounts
                    .current_quota(&account_id)
                    .await
                    .map(|account| (id, account))
            })
            .buffer_unordered(2)
            .collect::<Vec<_>>()
            .await;
        let generated_at = Utc::now();
        for result in loaded {
            let (id, account) = result?;
            if let Some(estimate) = estimates.get_mut(&id)
                && let Some((window, AccountUsagePeriod::Weekly)) = account.usage_window()
            {
                let current = current_window_estimate(window, generated_at);
                estimate.remaining_usd = current.map(|value| value.remaining_usd);
                estimate.incomplete = current.is_some_and(|value| value.incomplete_cost);
                estimate.reset_at = window.reset_at;
            }
        }
        let mut items = Vec::new();
        for group in facts.groups {
            let accounts = facts
                .members
                .iter()
                .filter(|fact| fact.member.group_id == group.id)
                .filter_map(|fact| estimates.get(&fact.member.account_id))
                .cloned()
                .collect::<Vec<_>>();
            let item = project_group_monitor(
                group.clone(),
                &accounts,
                &facts
                    .group_usage
                    .get(group.id.as_str())
                    .cloned()
                    .unwrap_or_default(),
            );
            items.push(item);
        }
        self.groups
            .save_group_monitor(
                &GroupMonitorReport {
                    generated_at: now,
                    items,
                },
                facts.config_revision,
            )
            .await
            .map_err(|error| map_store_error(error, "group monitor snapshot"))
    }

    async fn sample_requested(&self, requested: Instant) -> Result<(), AdminError> {
        let mut last = self.last_sample.lock().await;
        // Callers already waiting for this cycle share its success or failure.
        if let Some((finished, result)) = last.as_ref()
            && *finished >= requested
        {
            return result.clone();
        }
        let result = tokio::time::timeout(SAMPLE_TIMEOUT, self.collect_and_save())
            .await
            .unwrap_or_else(|_| Err(AdminError::unavailable("分组监控采样超时")));
        *last = Some((Instant::now(), result.clone()));
        result
    }

    async fn read_snapshot(
        &self,
        ids: &[AccountGroupId],
    ) -> Result<Option<GroupMonitorReport>, AdminError> {
        let _reader = self
            .readers
            .try_acquire()
            .map_err(|_| AdminError::unavailable("分组监控繁忙，请稍后重试"))?;
        self.groups
            .read_group_monitor(ids)
            .await
            .map_err(|error| map_store_error(error, "group monitor snapshot"))
    }
}

#[async_trait]
impl GroupMonitorService for DefaultGroupMonitorService {
    async fn sample(&self) -> Result<(), AdminError> {
        self.sample_requested(Instant::now()).await
    }

    async fn read(
        &self,
        group_ids: Vec<AccountGroupId>,
        refresh_forecasts: bool,
    ) -> Result<GroupMonitorReport, AdminError> {
        let requested = Instant::now();
        if group_ids.is_empty()
            || group_ids.len() > 3
            || group_ids.iter().collect::<BTreeSet<_>>().len() != group_ids.len()
        {
            return Err(AdminError::invalid("每次只能读取一至三个不同的分组"));
        }
        // Validate current group IDs even before a manual full-pool sample.
        let mut report = self.read_snapshot(&group_ids).await?;
        if refresh_forecasts {
            self.sample_requested(requested).await?;
            report = self.read_snapshot(&group_ids).await?;
        }
        report.ok_or_else(|| AdminError::unavailable("分组监控正在等待新采样，请稍后刷新"))
    }
}
