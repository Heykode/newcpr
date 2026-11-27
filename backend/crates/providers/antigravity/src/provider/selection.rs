//! 协调 Antigravity 账号选择、并发排队和租约，取得租约后复核凭据

use super::{
    AntigravityProvider,
    candidates::{CandidatePool, Project},
};
use crate::{credential::Material, transport::unsent};
use gateway_core::{
    account::{
        AccountCapacitySnapshot, AccountSelector, PreferredAccountSelection, ProviderAccount,
        RotationStrategy,
    },
    concurrency::{CapacityWait, ConcurrencyQueuePolicy, QueueRejection},
    engine::AttemptContext,
    error::{ProviderError, ProviderErrorKind as Kind},
    provider_ports::{
        ProviderAccountAdmission, ProviderLeaseAcquisition, ProviderLeaseGuard,
        ProviderLeaseRequest, ProviderSchedulingLeaseRequest, ProviderSessionAffinityKey,
        ProviderSessionAffinityState,
    },
    routing::UpstreamModelId,
};
use std::time::Duration;

pub(super) struct SelectedAccount {
    pub(super) account: ProviderAccount,
    pub(super) observed_plan: Option<(Option<String>, Option<String>)>,
    pub(super) capacity: Option<AccountCapacitySnapshot>,
    pub(super) material: Material,
    pub(super) project: Project,
    pub(super) lease: Box<dyn ProviderLeaseGuard>,
    pub(super) account_switch: bool,
}

#[derive(Debug, thiserror::Error)]
enum SelectionError {
    #[error(transparent)]
    Queue(#[from] QueueRejection),
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

impl AntigravityProvider {
    pub(super) async fn select_account(
        &self,
        model: &UpstreamModelId,
        ctx: &AttemptContext,
        affinity_key: Option<&ProviderSessionAffinityKey>,
        search: bool,
    ) -> Result<SelectedAccount, ProviderError> {
        let policy = ctx.account_selection_policy();
        let select = |queue| self.select_with_queue(model, ctx, queue, affinity_key, search);
        let result = match select(policy.queue_policy()).await {
            Err(SelectionError::Queue(reason)) if ctx.required_account().is_none() => {
                policy
                    .reselect_after_queue_rejection(
                        reason,
                        ctx.deadline().remaining(),
                        select,
                        |error| match error {
                            SelectionError::Provider(error) => matches!(
                                error.kind(),
                                Kind::NoEligibleAccount | Kind::AccountCapacityUnavailable
                            ),
                            SelectionError::Queue(_) => false,
                        },
                    )
                    .await
            }
            result => result,
        };
        result.map_err(|error| match error {
            SelectionError::Queue(reason) => unsent(reason.provider_kind()),
            SelectionError::Provider(error) => error,
        })
    }

    async fn select_with_queue(
        &self,
        model: &UpstreamModelId,
        ctx: &AttemptContext,
        queue: ConcurrencyQueuePolicy,
        affinity_key: Option<&ProviderSessionAffinityKey>,
        search: bool,
    ) -> Result<SelectedAccount, SelectionError> {
        let budget = ctx
            .account_wait_budget()
            .map(|budget| budget.concurrency_wait_budget())
            .unwrap_or_default();
        let mut waiting = CapacityWait::new(&self.waiting, queue, ctx.deadline().at(), &budget)
            .with_periodic_recheck(ctx.required_account().is_none());
        'refresh: loop {
            if ctx.cancellation().is_cancelled() {
                return Err(unsent(Kind::Cancelled).into());
            }
            if ctx.deadline().is_elapsed() {
                return Err(unsent(Kind::Timeout).into());
            }
            let CandidatePool {
                accounts,
                mut projects,
                mut quotas,
                mut selection,
            } = self.load_candidates(model, ctx, search).await?;
            let affinity = match affinity_key.filter(|_| !ctx.is_diagnostic_required_account()) {
                Some(key) => Some(ProviderSessionAffinityState::new(
                    key,
                    self.session_affinity
                        .load_binding(&self.kind, key)
                        .await
                        .map_err(super::unavailable)?,
                    ctx.account_selection_policy(),
                )),
                None => None,
            };
            if let Some(affinity) = &affinity {
                affinity.apply_preference(&mut selection);
            }
            let binding_eligible = affinity
                .as_ref()
                .is_some_and(|affinity| affinity.binding_eligible(&accounts, &selection));
            // 分配游标只读，排队重查和亲和命中不消耗新分配的位置
            let assignment_cursor = if selection.policy.strategy() == RotationStrategy::RoundRobin
                && ctx.required_account().is_none()
            {
                let cursor = self
                    .session_affinity
                    .round_robin_cursor(&self.kind, ctx.client_api_key_ref())
                    .await
                    .map_err(super::unavailable)?;
                selection.round_robin_cursor = cursor;
                Some(cursor)
            } else {
                None
            };
            let mut available = accounts.clone();
            available.retain(|candidate| waiting.can_try(candidate.account.id()));
            let mut retry_after: Option<Duration> = None;
            while let Some(choice) = AccountSelector.select(&available, &selection) {
                if ctx.required_account().is_none()
                    && affinity
                        .as_ref()
                        .and_then(|affinity| {
                            affinity.wait_account(&accounts, &selection, queue.max_waiting > 0)
                        })
                        .is_some_and(|bound| bound != choice.candidate().account.id())
                {
                    break;
                }
                let candidate = choice.candidate();
                let account = &candidate.account;
                let acquire = ProviderSchedulingLeaseRequest::new(
                    self.kind.clone(),
                    account.id().clone(),
                    account.revision(),
                    selection.concurrency_limit(&candidate.account),
                    selection.policy.request_interval(),
                    ctx.deadline(),
                )
                .with_detail(ctx.lease_detail(Some(model.as_str())))
                .with_cancellation(ctx.cancellation().clone());
                match self
                    .leases
                    .try_acquire(ProviderLeaseRequest::Scheduling(acquire))
                    .await
                    .map_err(super::unavailable)?
                {
                    ProviderLeaseAcquisition::Acquired(lease) => {
                        let (_, project) = projects
                            .remove(account.id())
                            .ok_or_else(|| unsent(Kind::NoEligibleAccount))?;
                        let capacity = AccountSelector
                            .capacity_snapshot(&accounts, &selection)
                            .map(AccountCapacitySnapshot::with_acquired_request);
                        // 在认领迁移前保留原绑定事实，临时分流也按实际出站账号判断
                        let account_switch = affinity
                            .as_ref()
                            .and_then(ProviderSessionAffinityState::bound_account)
                            .is_some_and(|bound| bound != account.id())
                            || ctx
                                .account_state_owner()
                                .is_some_and(|owner| !owner.matches(&self.kind, account.id()));
                        let mut selected = self
                            .load_selected(account, project, lease, capacity, account_switch, ctx)
                            .await?;
                        selected.observed_plan = quotas
                            .remove(account.id())
                            .map(|quota| (quota.plan_type, quota.plan_display));
                        let binding = affinity.as_ref().map(|affinity| {
                            affinity.claim_binding(
                                account.id(),
                                binding_eligible,
                                ctx.required_account().is_some(),
                            )
                        });
                        let claim = affinity
                            .as_ref()
                            .zip(binding.as_ref())
                            .map(|(affinity, binding)| affinity.claim(binding));
                        // 临时分流保留主绑定，亲和命中不占轮询位置。
                        let admission = match assignment_cursor {
                            Some(cursor)
                                if choice.preferred() != PreferredAccountSelection::Hit
                                    && claim.as_ref().is_none_or(|claim| {
                                        claim.binding.account_id() == account.id()
                                    }) =>
                            {
                                Some(ProviderAccountAdmission::RoundRobin {
                                    client_key: ctx.client_api_key_ref(),
                                    cursor,
                                    session: claim,
                                })
                            }
                            _ => claim.map(ProviderAccountAdmission::Session),
                        };
                        if let Some(admission) = admission
                            && !self
                                .session_affinity
                                .admit_account(&self.kind, admission)
                                .await
                                .map_err(super::unavailable)?
                        {
                            selected.lease.release().await.map_err(super::unavailable)?;
                            tokio::task::yield_now().await;
                            continue 'refresh;
                        }
                        return Ok(selected);
                    }
                    ProviderLeaseAcquisition::Busy { retry_after: delay } => {
                        if let Some(delay) = delay {
                            retry_after =
                                Some(retry_after.map_or(delay, |current| current.min(delay)));
                        }
                        let busy_id = account.id().clone();
                        available.retain(|candidate| candidate.account.id() != &busy_id);
                    }
                }
            }
            let mut keys = AccountSelector.wait_candidates(&accounts, &selection);
            if ctx.required_account().is_none()
                && let Some(bound) = affinity.as_ref().and_then(|affinity| {
                    affinity.wait_account(&accounts, &selection, queue.max_waiting > 0)
                })
            {
                keys.retain(|account| account == bound);
            }
            if keys.is_empty() {
                return Err(unsent(Kind::NoEligibleAccount).into());
            }
            if queue.max_waiting == 0 || ctx.is_diagnostic_required_account() {
                let mut error = unsent(Kind::AccountCapacityUnavailable);
                if let Some(delay) = retry_after {
                    error = error.with_retry_after(delay);
                }
                return Err(error.into());
            }
            tokio::select! {
                biased;
                () = ctx.cancellation().cancelled() => return Err(unsent(Kind::Cancelled).into()),
                result = AccountSelector.wait_for_capacity(&mut waiting, &keys, &accounts, &selection) => result?,
            }
        }
    }

    async fn load_selected(
        &self,
        account: &ProviderAccount,
        project: Project,
        lease: Box<dyn ProviderLeaseGuard>,
        capacity: Option<AccountCapacitySnapshot>,
        account_switch: bool,
        ctx: &AttemptContext,
    ) -> Result<SelectedAccount, ProviderError> {
        let current = self
            .credentials
            .current(account.id())
            .await
            .map_err(super::unavailable)?;
        if current.account.revision() != account.revision()
            || ctx.excludes_proxy(current.account.outbound_proxy())
        {
            return Err(unsent(Kind::NoEligibleAccount));
        }
        let material = self.credentials.material(&current)?;
        Ok(SelectedAccount {
            account: current.account,
            observed_plan: None,
            capacity,
            material,
            project,
            lease,
            account_switch,
        })
    }
}
