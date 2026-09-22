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
        group_monitor_quota::monitor_quota_estimates,
        provider_credentials::{AccountUsagePeriod, explicit_plan_type},
    },
    ports::store::{AccountGroupStore, AccountRuntimeStore},
};

use super::notifications::NotificationsService;
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
    notifications: Arc<dyn NotificationsService>,
    last_sample: Mutex<Option<(Instant, Result<(), AdminError>)>>,
    readers: Semaphore,
}

impl DefaultGroupMonitorService {
    pub(crate) fn new(
        groups: Arc<dyn AccountGroupStore>,
        runtime: Arc<dyn AccountRuntimeStore>,
        accounts: Arc<dyn AccountsService>,
        notifications: Arc<dyn NotificationsService>,
    ) -> Self {
        Self {
            groups,
            runtime,
            accounts,
            notifications,
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
        let client_ids = facts
            .group_client_keys
            .values()
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let client_counts = self
            .runtime
            .client_in_flight(&client_ids)
            .await
            .map_err(|error| map_store_error(error, "group monitor concurrency"))?;
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
        let target_plans = facts
            .members
            .iter()
            .filter(|member| eligible_ids.contains(&member.member.account_id))
            .filter_map(|member| {
                explicit_plan_type(member.plan.as_deref())
                    .map(|plan| (member.provider.as_str(), plan.trim().to_ascii_lowercase()))
            })
            .collect::<BTreeSet<_>>();
        let mut peers = facts.quota_peers;
        peers.retain(|peer| {
            eligible_ids.contains(&peer.id)
                || explicit_plan_type(peer.plan.as_deref()).is_some_and(|plan| {
                    target_plans
                        .contains(&(peer.provider.as_str(), plan.trim().to_ascii_lowercase()))
                })
        });
        // Own and deduplicate IDs before crossing the async-trait boundary.
        let mut quota_ids = BTreeSet::new();
        quota_ids.extend(peers.iter().map(|peer| peer.id.clone()));
        let loaded = stream::iter(quota_ids)
            .map(|id| async move {
                let result = match ProviderAccountId::new(id.clone()) {
                    Ok(account_id) => self.accounts.current_quota(&account_id).await,
                    Err(_) => Err(AdminError::internal("监控账号 ID 不合法")),
                };
                (id, result)
            })
            .buffer_unordered(2)
            .collect::<Vec<_>>()
            .await;
        let generated_at = Utc::now();
        let mut quotas = BTreeMap::new();
        for (id, result) in loaded {
            match result {
                Ok(quota) => {
                    quotas.insert(id, quota);
                }
                Err(error) if eligible_ids.contains(&id) => return Err(error),
                // Optional references may disappear or become unreadable between samples.
                Err(_) => {}
            }
        }
        let current = monitor_quota_estimates(&peers, &quotas, generated_at);
        for (id, account) in &quotas {
            if let Some(estimate) = estimates.get_mut(id)
                && let Some((window, AccountUsagePeriod::Weekly)) = account.usage_window()
            {
                estimate.remaining_usd = current.get(id).map(|value| value.remaining_usd);
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
                if !group.enabled {
                    Some(0)
                } else {
                    facts
                        .group_client_keys
                        .get(group.id.as_str())
                        .map_or(Some(0), |keys| {
                            client_counts.as_ref().and_then(|counts| {
                                keys.iter().collect::<BTreeSet<_>>().into_iter().try_fold(
                                    0_u64,
                                    |sum, key| {
                                        counts.get(key).map(|value| sum.saturating_add(*value))
                                    },
                                )
                            })
                        })
                },
            );
            items.push(item);
        }
        let report = GroupMonitorReport {
            generated_at: now,
            items,
        };
        self.groups
            .save_group_monitor(&report, facts.config_revision)
            .await
            .map_err(|error| map_store_error(error, "group monitor snapshot"))?;
        match tokio::time::timeout(Duration::from_secs(3), self.notifications.observe(&report))
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "group monitor alert evaluation failed")
            }
            Err(_) => tracing::warn!("group monitor alert evaluation timed out"),
        }
        Ok(())
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
