//! Opt-in account admission. Store owns leases; Core owns ordering and the shared clock.

use std::collections::BTreeSet;
use std::future::Future;
use std::num::NonZeroU32;

use gateway_core::account::AccountSchedulingAvailability;
use gateway_core::engine::{AccountWaitDeadline, AccountWaitMode};
use gateway_core::provider_ports::{
    ProviderWaitLeaseAcquisition, ProviderWaitLeaseRequest, ProviderWaitPromotion,
};
use gateway_core::runtime::AccountConcurrencySnapshot;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AccountWaitFailure {
    #[error("queue_full")]
    QueueFull,
    #[error("wait_timeout")]
    Timeout,
    #[error("wait_token_expired")]
    TokenExpired,
}

impl AccountWaitFailure {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::QueueFull => "queue_full",
            Self::Timeout => "wait_timeout",
            Self::TokenExpired => "wait_token_expired",
        }
    }
}

struct WaitControl<'a> {
    attempt: &'a AttemptContext,
    deadline: tokio::time::Instant,
}

impl<'a> WaitControl<'a> {
    fn new(attempt: &'a AttemptContext) -> Result<Self, CredentialSelectionError> {
        let budget = attempt
            .account_wait_budget()
            .ok_or(CredentialSelectionError::Coordinator)?;
        if budget.is_exhausted() {
            return Err(CredentialSelectionError::AccountWait(
                AccountWaitFailure::Timeout,
            ));
        }
        Ok(Self {
            attempt,
            deadline: tokio::time::Instant::now()
                + attempt
                    .deadline()
                    .duration_since(SystemTime::now())
                    .unwrap_or_default(),
        })
    }

    fn enter(
        &mut self,
        mode: AccountWaitMode,
    ) -> Result<AccountWaitDeadline, CredentialSelectionError> {
        let budget = self
            .attempt
            .account_wait_budget()
            .ok_or(CredentialSelectionError::Coordinator)?;
        let deadline = budget.enter(mode).map_err(|error| match error {
            gateway_core::engine::AccountWaitBudgetError::Expired => {
                CredentialSelectionError::AccountWait(AccountWaitFailure::Timeout)
            }
            _ => CredentialSelectionError::Coordinator,
        })?;
        self.deadline = self.deadline.min(deadline.monotonic_deadline().into());
        Ok(deadline)
    }

    async fn run<T>(
        &self,
        future: impl Future<Output = Result<T, CredentialSelectionError>>,
    ) -> Result<T, CredentialSelectionError> {
        tokio::select! {
            biased;
            () = self.attempt.cancellation().cancelled() => Err(CredentialSelectionError::Cancelled),
            () = tokio::time::sleep_until(self.deadline) => {
                if let Some(budget) = self.attempt.account_wait_budget() {
                    budget.expire();
                }
                Err(CredentialSelectionError::AccountWait(AccountWaitFailure::Timeout))
            }
            result = future => result,
        }
    }
}

struct WaitingSelection {
    universe: BTreeSet<ProviderAccountId>,
    context: AccountSelectionContext,
    pinned: Option<ProviderAccountId>,
    affinity: AffinitySelection,
    observed_affinity: Option<ProviderAccountId>,
    cyber_policy_scope: Option<CodexCyberPolicyScope>,
}

enum WaitOutcome {
    Acquired(Box<CodexCredentialLease>),
    Full,
    Changed,
    Retry,
}

// Interval waiting is a soft-affinity exception, not execution-capacity Busy.
fn sole_interval_deadline(
    candidate: &AccountCandidate,
    context: &AccountSelectionContext,
    limits: &AccountConcurrencySnapshot,
) -> Option<SystemTime> {
    if AccountSelector.availability(candidate, context, limits)
        != AccountSchedulingAvailability::Blocked(AccountSchedulingBlocker::RequestInterval)
    {
        return None;
    }
    let mut without_interval = candidate.clone();
    without_interval.signals.last_started_at = None;
    if AccountSelector.availability(&without_interval, context, limits)
        != AccountSchedulingAvailability::Ready
    {
        return None;
    }
    candidate
        .signals
        .last_started_at?
        .checked_add(context.policy.request_interval())
}

impl CodexCredentialSelector {
    pub(super) async fn select_with_capacity_wait(
        &self,
        request: &CredentialSelectionInput<'_>,
        cyber_policy_key: Option<&ProviderSessionAffinityKey>,
    ) -> Result<CodexCredentialLease, CredentialSelectionError> {
        let pinned = request.attempt.required_account().is_some()
            || matches!(
                request.attempt.continuation_attempt(),
                ContinuationAttempt::Native | ContinuationAttempt::ReplayOwner
            );
        let result = self.select_waiting_inner(request, cyber_policy_key).await;
        result.map_err(|source| {
            if pinned {
                let owner_lost = matches!(source, CredentialSelectionError::NoEligibleCredential);
                CredentialSelectionError::PinnedAccount {
                    source: Box::new(source),
                    owner_lost,
                }
            } else {
                source
            }
        })
    }

    async fn select_waiting_inner(
        &self,
        request: &CredentialSelectionInput<'_>,
        cyber_policy_key: Option<&ProviderSessionAffinityKey>,
    ) -> Result<CodexCredentialLease, CredentialSelectionError> {
        let mut control = WaitControl::new(request.attempt)?;
        let accounts = control
            .run(async {
                self.repository
                    .list_for_provider()
                    .await
                    .map_err(Into::into)
            })
            .await?;
        let ids = accounts
            .iter()
            .map(|account| account.id().clone())
            .collect::<Vec<_>>();
        let scheduling = control
            .run(async {
                self.leases
                    .load_state(
                        request.attempt.client_api_key_ref(),
                        &self.provider_kind,
                        &ids,
                    )
                    .await
                    .map_err(Into::into)
            })
            .await?;
        let mut candidates = control
            .run(self.wait_candidates(accounts, scheduling.signals(), request))
            .await?;
        let continuation = match request.attempt.continuation_attempt() {
            ContinuationAttempt::Native => request
                .attempt
                .continuation()
                .and_then(gateway_core::engine::continuation::ContinuationBinding::pinned)
                .map(|pin| pin.account().clone()),
            ContinuationAttempt::ReplayOwner => request
                .attempt
                .account_state_owner()
                .filter(|owner| owner.provider() == &self.provider_kind)
                .map(|owner| owner.account().clone()),
            ContinuationAttempt::None | ContinuationAttempt::ReplayAny => None,
        };
        if request
            .attempt
            .required_account()
            .zip(continuation.as_ref())
            .is_some_and(|(required, owner)| required != owner)
        {
            return Err(CredentialSelectionError::NoEligibleCredential);
        }
        let pinned = request.attempt.required_account().cloned().or(continuation);
        let mut affinity = control
            .run(async {
                Ok(self
                    .resolve_session_affinity(
                        request.session_affinity_key,
                        request
                            .session_affinity_observation
                            .and_then(CodexSessionAffinity::root_key),
                        request
                            .session_affinity_observation
                            .and_then(CodexSessionAffinity::migration_key),
                        &candidates,
                        SystemTime::now(),
                    )
                    .await)
            })
            .await?;
        if pinned
            .as_ref()
            .zip(affinity.bound_account())
            .is_some_and(|(pin, bound)| pin != bound)
        {
            affinity.escape(AffinityEscapeReason::PinnedAccount);
        }
        let cyber_policy_scope = control
            .run(async {
                match cyber_policy_key {
                    Some(key) => Ok(Some(CodexCyberPolicyScope {
                        key: key.clone(),
                        state: self
                            .session_exclusions
                            .load(&self.provider_kind, key)
                            .await?,
                    })),
                    None => Ok(None),
                }
            })
            .await?;
        let mut excluded = request.attempt.excluded_accounts().clone();
        if let Some(state) = cyber_policy_scope
            .as_ref()
            .and_then(|scope| scope.state.as_ref())
        {
            excluded.extend(state.excluded_accounts().iter().cloned());
        }
        let observed_affinity = if affinity.inherited {
            None
        } else {
            affinity.bound_account().cloned()
        };
        let mut state = WaitingSelection {
            universe: ids.into_iter().collect(),
            context: AccountSelectionContext {
                policy: request.attempt.account_selection_policy(),
                now: SystemTime::now(),
                excluded_accounts: excluded,
                preferred_account: pinned
                    .clone()
                    .or_else(|| affinity.preferred_account().cloned()),
                preferred_account_overrides_weight: true,
                round_robin_cursor: scheduling.round_robin_cursor(),
                eligibility: AccountEligibilityPolicy::Enforce,
                account_scope: request.attempt.account_scope().cloned(),
            },
            pinned,
            affinity,
            observed_affinity,
            cyber_policy_scope,
        };
        let mut sticky_tried = BTreeSet::new();

        // A changed account/credential/publication gets a bounded fresh selection, never an upstream retry.
        'rescan: for scan in 0..3 {
            control
                .run(self.reload_wait_exclusions(&mut state, request.attempt))
                .await?;
            if scan != 0 {
                candidates = control
                    .run(self.reload_wait_pool(request, &state.universe))
                    .await?;
            }
            if let Some(pin) = &state.pinned {
                candidates.retain(|candidate| candidate.account.id() == pin);
            }
            state.context.now = SystemTime::now();
            state.context.preferred_account = state
                .pinned
                .clone()
                .or_else(|| state.affinity.preferred_account().cloned());
            let limits = self.wait_limits()?;
            let original = state
                .pinned
                .clone()
                .or_else(|| state.affinity.bound_account().cloned());
            if let Some(original) = original.as_ref()
                && !sticky_tried.contains(original)
                && candidates.iter().any(|candidate| {
                    candidate.account.id() == original
                        && (AccountSelector.availability(candidate, &state.context, &limits)
                            == AccountSchedulingAvailability::Busy
                            || (state.pinned.is_none()
                                && sole_interval_deadline(candidate, &state.context, &limits)
                                    .is_some()))
                })
            {
                sticky_tried.insert(original.clone());
                match self
                    .wait_for_account(
                        original,
                        AccountWaitMode::Sticky,
                        &mut control,
                        request,
                        &mut state,
                    )
                    .await?
                {
                    WaitOutcome::Acquired(lease) => return Ok(*lease),
                    WaitOutcome::Retry => continue 'rescan,
                    WaitOutcome::Changed => {
                        if state.pinned.is_some() {
                            return Err(CredentialSelectionError::NoEligibleCredential);
                        }
                        continue 'rescan;
                    }
                    WaitOutcome::Full => {
                        if state.pinned.is_some() {
                            return Err(CredentialSelectionError::AccountWait(
                                AccountWaitFailure::QueueFull,
                            ));
                        }
                        if candidates.iter().any(|candidate| {
                            candidate.account.id() == original
                                && sole_interval_deadline(candidate, &state.context, &limits)
                                    .is_some()
                        }) {
                            state.affinity.observe_preferred_selection(
                                PreferredAccountSelection::Blocked(
                                    AccountSchedulingBlocker::RequestInterval,
                                ),
                            );
                        } else {
                            state.affinity.observe_lease_busy(original);
                        }
                    }
                }
            }
            let mut raced = BTreeSet::new();
            loop {
                let mut context = state.context.clone();
                context.now = SystemTime::now();
                context.excluded_accounts.extend(raced.iter().cloned());
                let limits = self.wait_limits()?;
                let selection =
                    AccountSelector.select_with_live_capacity(&candidates, &context, &limits);
                request.attempt.trace().account_selection(
                    &candidates,
                    &context,
                    selection.as_ref(),
                );
                let Some(selection) = selection else {
                    break;
                };
                let id = selection.candidate().account.id().clone();
                let Some(candidate) = control.run(self.reload_wait_target(&id, request)).await?
                else {
                    raced.insert(id);
                    continue;
                };
                state.context.now = SystemTime::now();
                control
                    .run(self.reload_wait_exclusions(&mut state, request.attempt))
                    .await?;
                let limits = self.wait_limits()?;
                if AccountSelector.availability(&candidate, &state.context, &limits)
                    != AccountSchedulingAvailability::Ready
                {
                    if let Some(old) = candidates.iter_mut().find(|old| old.account.id() == &id) {
                        *old = candidate;
                    }
                    raced.insert(id);
                    continue;
                }
                let lease_request =
                    self.wait_execution_request(&candidate.account, &limits, request.attempt)?;
                match control
                    .run(async {
                        self.leases
                            .try_acquire_scheduling(lease_request)
                            .await
                            .map_err(Into::into)
                    })
                    .await?
                {
                    ProviderLeaseAcquisition::Acquired(guard) => {
                        if let Some(lease) = control
                            .run(self.finish_wait_selection(
                                guard,
                                limits.revision(),
                                &candidate.account,
                                request,
                                &mut state,
                            ))
                            .await?
                        {
                            return Ok(lease);
                        }
                        continue 'rescan;
                    }
                    ProviderLeaseAcquisition::Busy { .. } => {
                        raced.insert(id.clone());
                        if original.as_ref() == Some(&id) && sticky_tried.insert(id.clone()) {
                            match self
                                .wait_for_account(
                                    &id,
                                    AccountWaitMode::Sticky,
                                    &mut control,
                                    request,
                                    &mut state,
                                )
                                .await?
                            {
                                WaitOutcome::Acquired(lease) => return Ok(*lease),
                                WaitOutcome::Retry => continue 'rescan,
                                WaitOutcome::Changed => {
                                    if state.pinned.is_some() {
                                        return Err(CredentialSelectionError::NoEligibleCredential);
                                    }
                                    continue 'rescan;
                                }
                                WaitOutcome::Full if state.pinned.is_some() => {
                                    return Err(CredentialSelectionError::AccountWait(
                                        AccountWaitFailure::QueueFull,
                                    ));
                                }
                                WaitOutcome::Full => {}
                            }
                        }
                        state.affinity.observe_lease_busy(&id);
                    }
                }
            }
            // Refresh facts without consuming another RR cursor. Lease-race Busy remains a
            // separate observation, not a forged in_flight signal or a business exclusion.
            candidates = control
                .run(self.reload_wait_pool(request, &state.universe))
                .await?;
            if let Some(pin) = &state.pinned {
                candidates.retain(|candidate| candidate.account.id() == pin);
            }
            let mut tried = BTreeSet::new();
            let mut saw_full = false;
            loop {
                let limits = self.wait_limits()?;
                let mut context = state.context.clone();
                context.now = SystemTime::now();
                context.excluded_accounts.extend(tried.iter().cloned());
                let fresh_ready = candidates
                    .iter()
                    .filter(|candidate| !raced.contains(candidate.account.id()))
                    .cloned()
                    .collect::<Vec<_>>();
                if AccountSelector
                    .select_with_live_capacity(&fresh_ready, &context, &limits)
                    .is_some()
                {
                    continue 'rescan;
                }
                let race_candidates = candidates
                    .iter()
                    .filter(|candidate| raced.contains(candidate.account.id()))
                    .cloned()
                    .collect::<Vec<_>>();
                let selected = AccountSelector
                    .select_for_capacity_wait(&candidates, &context, &limits)
                    .or_else(|| {
                        AccountSelector.select_with_live_capacity(
                            &race_candidates,
                            &context,
                            &limits,
                        )
                    });
                let Some(selected) = selected else {
                    return Err(if saw_full {
                        CredentialSelectionError::AccountWait(AccountWaitFailure::QueueFull)
                    } else {
                        CredentialSelectionError::NoEligibleCredential
                    });
                };
                let id = selected.candidate().account.id().clone();
                tried.insert(id.clone());
                match self
                    .wait_for_account(
                        &id,
                        AccountWaitMode::Fallback,
                        &mut control,
                        request,
                        &mut state,
                    )
                    .await?
                {
                    WaitOutcome::Acquired(lease) => return Ok(*lease),
                    WaitOutcome::Retry => continue 'rescan,
                    WaitOutcome::Full => saw_full = true,
                    WaitOutcome::Changed => {
                        if state.pinned.is_some() {
                            return Err(CredentialSelectionError::NoEligibleCredential);
                        }
                        continue 'rescan;
                    }
                }
            }
        }
        Err(CredentialSelectionError::Coordinator)
    }

    fn wait_limits(&self) -> Result<Arc<AccountConcurrencySnapshot>, CredentialSelectionError> {
        self.account_concurrency
            .load()
            .map_err(|_| CredentialSelectionError::Coordinator)?
            .ok_or(CredentialSelectionError::Coordinator)
    }

    async fn reload_wait_exclusions(
        &self,
        state: &mut WaitingSelection,
        attempt: &AttemptContext,
    ) -> Result<(), CredentialSelectionError> {
        state.context.excluded_accounts = attempt.excluded_accounts().clone();
        if let Some(scope) = state.cyber_policy_scope.as_mut() {
            scope.state = self
                .session_exclusions
                .load(&self.provider_kind, &scope.key)
                .await?;
            if let Some(current) = scope.state.as_ref() {
                state
                    .context
                    .excluded_accounts
                    .extend(current.excluded_accounts().iter().cloned());
            }
        }
        Ok(())
    }

    async fn wait_candidates(
        &self,
        accounts: Vec<ProviderAccount>,
        signals: &std::collections::BTreeMap<ProviderAccountId, AccountRuntimeSignals>,
        request: &CredentialSelectionInput<'_>,
    ) -> Result<Vec<AccountCandidate>, CredentialSelectionError> {
        let mut candidates = Vec::new();
        let accounts = self.state_ready_accounts(accounts, request).await;
        self.quota.prepare_scheduling(&accounts).await;
        for account in accounts {
            if account.provider() != &self.provider_kind
                || !request
                    .attempt
                    .account_scope()
                    .is_some_and(|scope| scope.allows(account.id()))
            {
                continue;
            }
            let cooldown = self
                .quota
                .rate_limited_until(account.id())
                .await
                .map_err(|_| CredentialSelectionError::Store)?;
            let health = self
                .account_feedback
                .scheduling_signals(&self.provider_kind, account.id());
            let signals = signals
                .get(account.id())
                .cloned()
                .ok_or(CredentialSelectionError::Coordinator)?
                .with_provider_quota(self.quota.scheduling_signals(&account))
                .with_rate_limit(cooldown)
                .with_runtime_health(health.0, health.1);
            candidates.push(AccountCandidate { account, signals });
        }
        Ok(candidates)
    }

    async fn reload_wait_pool(
        &self,
        request: &CredentialSelectionInput<'_>,
        universe: &BTreeSet<ProviderAccountId>,
    ) -> Result<Vec<AccountCandidate>, CredentialSelectionError> {
        let accounts = self
            .repository
            .list_for_provider()
            .await?
            .into_iter()
            .filter(|account| universe.contains(account.id()))
            .collect::<Vec<_>>();
        let ids = accounts
            .iter()
            .map(|account| account.id().clone())
            .collect::<Vec<_>>();
        let signals = self.leases.load_signals(&self.provider_kind, &ids).await?;
        self.wait_candidates(accounts, &signals, request).await
    }

    async fn reload_wait_target(
        &self,
        id: &ProviderAccountId,
        request: &CredentialSelectionInput<'_>,
    ) -> Result<Option<AccountCandidate>, CredentialSelectionError> {
        let Some(account) = self
            .repository
            .store()
            .get_account(id)
            .await
            .map_err(|_| CredentialSelectionError::Store)?
        else {
            return Ok(None);
        };
        let signals = self
            .leases
            .load_signals(&self.provider_kind, std::slice::from_ref(id))
            .await?;
        Ok(self
            .wait_candidates(vec![account], &signals, request)
            .await?
            .pop())
    }

    fn wait_execution_request(
        &self,
        account: &ProviderAccount,
        limits: &AccountConcurrencySnapshot,
        attempt: &AttemptContext,
    ) -> Result<ProviderSchedulingLeaseRequest, CredentialSelectionError> {
        Ok(ProviderSchedulingLeaseRequest::new(
            self.provider_kind.clone(),
            account.id().clone(),
            account.revision(),
            limits
                .limit_for(account.id().as_str())
                .ok_or(CredentialSelectionError::NoEligibleCredential)?,
            attempt.account_selection_policy().request_interval(),
            attempt.deadline(),
        ))
    }

    async fn wait_for_account(
        &self,
        id: &ProviderAccountId,
        mode: AccountWaitMode,
        control: &mut WaitControl<'_>,
        request: &CredentialSelectionInput<'_>,
        state: &mut WaitingSelection,
    ) -> Result<WaitOutcome, CredentialSelectionError> {
        let Some(candidate) = control.run(self.reload_wait_target(id, request)).await? else {
            return Ok(WaitOutcome::Changed);
        };
        state.context.now = SystemTime::now();
        control
            .run(self.reload_wait_exclusions(state, request.attempt))
            .await?;
        let limits = self.wait_limits()?;
        let interval_end = (mode == AccountWaitMode::Sticky && state.pinned.is_none())
            .then(|| sole_interval_deadline(&candidate, &state.context, &limits))
            .flatten();
        match AccountSelector.availability(&candidate, &state.context, &limits) {
            AccountSchedulingAvailability::Blocked(AccountSchedulingBlocker::RequestInterval)
                if interval_end.is_some() => {}
            AccountSchedulingAvailability::Blocked(_) => return Ok(WaitOutcome::Changed),
            AccountSchedulingAvailability::Busy => {}
            AccountSchedulingAvailability::Ready => {
                let acquired = control
                    .run(async {
                        self.leases
                            .try_acquire_scheduling(self.wait_execution_request(
                                &candidate.account,
                                &limits,
                                request.attempt,
                            )?)
                            .await
                            .map_err(Into::into)
                    })
                    .await?;
                if let ProviderLeaseAcquisition::Acquired(guard) = acquired {
                    return Ok(
                        match control
                            .run(self.finish_wait_selection(
                                guard,
                                limits.revision(),
                                &candidate.account,
                                request,
                                state,
                            ))
                            .await?
                        {
                            Some(lease) => WaitOutcome::Acquired(Box::new(lease)),
                            None => WaitOutcome::Retry,
                        },
                    );
                }
                let Some(current) = control.run(self.reload_wait_target(id, request)).await? else {
                    return Ok(WaitOutcome::Changed);
                };
                state.context.now = SystemTime::now();
                control
                    .run(self.reload_wait_exclusions(state, request.attempt))
                    .await?;
                let limits = self.wait_limits()?;
                if matches!(
                    AccountSelector.availability(&current, &state.context, &limits),
                    AccountSchedulingAvailability::Blocked(_)
                ) {
                    return Ok(WaitOutcome::Changed);
                }
            }
        }
        let tuning = request.attempt.request_tuning();
        // A skipped interval must not start a new sticky window for later retries.
        if interval_end.is_some_and(|end| {
            end >= request.attempt.deadline()
                || end.duration_since(SystemTime::now()).unwrap_or_default()
                    >= Duration::from_secs(tuning.account_busy_wait_sticky_timeout_seconds)
        }) {
            return Ok(WaitOutcome::Changed);
        }
        let deadline = control.enter(mode)?;
        if interval_end.is_some_and(|end| end >= deadline.deadline()) {
            return Ok(WaitOutcome::Changed);
        }
        let max_waiting = NonZeroU32::new(match mode {
            AccountWaitMode::Sticky => tuning.account_busy_wait_sticky_max_waiting,
            AccountWaitMode::Fallback => tuning.account_busy_wait_fallback_max_waiting,
        })
        .ok_or(CredentialSelectionError::Coordinator)?;
        let acquisition = control
            .run(async {
                self.leases
                    .try_acquire_wait(ProviderWaitLeaseRequest::new(
                        self.provider_kind.clone(),
                        id.clone(),
                        request.attempt.request_id().clone(),
                        mode,
                        max_waiting,
                        deadline.deadline(),
                    ))
                    .await
                    .map_err(Into::into)
            })
            .await?;
        let ProviderWaitLeaseAcquisition::Acquired(mut waiting) = acquisition else {
            return Ok(WaitOutcome::Full);
        };
        let result = control
            .run(async {
                let mut poll = Duration::from_millis(100);
                loop {
                    let Some(candidate) = self.reload_wait_target(id, request).await? else {
                        return Ok(WaitOutcome::Changed);
                    };
                    let limits = self.wait_limits()?;
                    state.context.now = SystemTime::now();
                    self.reload_wait_exclusions(state, request.attempt).await?;
                    match AccountSelector.availability(&candidate, &state.context, &limits) {
                        AccountSchedulingAvailability::Blocked(
                            AccountSchedulingBlocker::RequestInterval,
                        ) if interval_end.is_some_and(|end| {
                            sole_interval_deadline(&candidate, &state.context, &limits)
                                .is_some_and(|current| current <= end && state.context.now < end)
                        }) =>
                        {
                            // Do not promote before the interval ends or chase a later start.
                            let remaining = interval_end
                                .and_then(|end| end.duration_since(SystemTime::now()).ok())
                                .unwrap_or_default();
                            tokio::time::sleep(poll.min(remaining)).await;
                            poll = (poll * 2).min(Duration::from_secs(1));
                            continue;
                        }
                        AccountSchedulingAvailability::Blocked(_) => {
                            return Ok(WaitOutcome::Changed);
                        }
                        AccountSchedulingAvailability::Busy if interval_end.is_some() => {
                            return Ok(WaitOutcome::Changed);
                        }
                        _ => {}
                    }
                    let promotion = waiting
                        .try_promote(self.wait_execution_request(
                            &candidate.account,
                            &limits,
                            request.attempt,
                        )?)
                        .await?;
                    match promotion {
                        ProviderWaitPromotion::Acquired(guard) => {
                            return Ok(
                                match self
                                    .finish_wait_selection(
                                        guard,
                                        limits.revision(),
                                        &candidate.account,
                                        request,
                                        state,
                                    )
                                    .await?
                                {
                                    Some(lease) => WaitOutcome::Acquired(Box::new(lease)),
                                    None => WaitOutcome::Retry,
                                },
                            );
                        }
                        ProviderWaitPromotion::Expired => {
                            if let Some(budget) = request.attempt.account_wait_budget() {
                                budget.expire();
                            }
                            return Err(CredentialSelectionError::AccountWait(
                                AccountWaitFailure::TokenExpired,
                            ));
                        }
                        ProviderWaitPromotion::Busy { .. } if interval_end.is_some() => {
                            return Ok(WaitOutcome::Changed);
                        }
                        ProviderWaitPromotion::Busy { .. } => {}
                    }
                    let jitter =
                        Duration::from_millis(u64::from(uuid::Uuid::new_v4().as_bytes()[0] % 25));
                    tokio::time::sleep(poll + jitter).await;
                    poll = (poll * 2).min(Duration::from_secs(1));
                }
            })
            .await;
        if let Ok(WaitOutcome::Acquired(lease)) = result {
            // Promotion has already removed/disarmed the wait ownership atomically.
            drop(waiting);
            return Ok(WaitOutcome::Acquired(lease));
        }
        // Await normal cleanup. Cancellation drops this future into Store's owned cleanup path.
        let released = control
            .run(async { waiting.release().await.map_err(Into::into) })
            .await;
        match result {
            Err(error) => Err(error),
            Ok(outcome) => {
                released?;
                Ok(outcome)
            }
        }
    }

    async fn finish_wait_selection(
        &self,
        guard: Box<dyn ProviderLeaseGuard>,
        revision: gateway_core::routing::ConfigRevision,
        acquired_account: &ProviderAccount,
        request: &CredentialSelectionInput<'_>,
        state: &mut WaitingSelection,
    ) -> Result<Option<CodexCredentialLease>, CredentialSelectionError> {
        let id = acquired_account.id();
        let Some(candidate) = self.reload_wait_target(id, request).await? else {
            return Ok(None);
        };
        let limits = self.wait_limits()?;
        state.context.now = SystemTime::now();
        self.reload_wait_exclusions(state, request.attempt).await?;
        // The acquired lease has just written last_started_at. Its own interval
        // is not a reason to reject it; the atomic acquisition checked the prior start.
        let mut acquired_context = state.context.clone();
        acquired_context.policy = gateway_core::account::AccountSelectionPolicy::new(
            state.context.policy.strategy(),
            state.context.policy.max_concurrent_per_account(),
            Duration::ZERO,
        );
        if candidate.account != *acquired_account
            || limits.revision() != revision
            || matches!(
                AccountSelector.availability(&candidate, &acquired_context, &limits),
                AccountSchedulingAvailability::Blocked(_)
            )
            || limits
                .limit_for(id.as_str())
                .is_none_or(|limit| candidate.signals.in_flight > limit.get())
        {
            drop(guard);
            return Ok(None);
        }
        let runtime = match self
            .repository
            .load_runtime_credential(&candidate.account)
            .await
        {
            Ok(runtime) => runtime,
            Err(CredentialRepositoryError::RevisionConflict) => {
                drop(guard);
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        if self.wait_limits()?.revision() != revision
            || self
                .repository
                .store()
                .get_account(id)
                .await
                .map_err(|_| CredentialSelectionError::Store)?
                .as_ref()
                != Some(&candidate.account)
        {
            drop(guard);
            return Ok(None);
        }
        let expected = if state
            .pinned
            .as_ref()
            .zip(state.observed_affinity.as_ref())
            .is_some_and(|(pin, bound)| pin != bound)
        {
            id.clone()
        } else {
            state
                .observed_affinity
                .clone()
                .unwrap_or_else(|| id.clone())
        };
        let cookies = runtime
            .cookies
            .into_iter()
            .filter(|cookie| {
                cookie
                    .expires_at
                    .is_none_or(|expires| expires > chrono::Utc::now())
                    && self.cookie_policy.may_replay(
                        request.request_url,
                        &cookie.domain,
                        &cookie.path,
                        cookie.host_only,
                        cookie.secure,
                    )
            })
            .collect();
        let current = self.reload_wait_pool(request, &state.universe).await?;
        self.reload_wait_exclusions(state, request.attempt).await?;
        let current_limits = self.wait_limits()?;
        acquired_context.now = SystemTime::now();
        acquired_context.excluded_accounts = state.context.excluded_accounts.clone();
        if current_limits.revision() != revision
            || !current.iter().any(|latest| {
                latest.account == candidate.account
                    && !matches!(
                        AccountSelector.availability(latest, &acquired_context, &current_limits),
                        AccountSchedulingAvailability::Blocked(_)
                    )
                    && current_limits
                        .limit_for(id.as_str())
                        .is_some_and(|limit| latest.signals.in_flight <= limit.get())
            })
        {
            drop(guard);
            return Ok(None);
        }
        let capacity = AccountSelector.capacity_snapshot_with_live_limits(
            &current,
            &state.context,
            &current_limits,
        );
        // All fallible credential/capacity validation precedes affinity mutation.
        // A losing initial CAS only observes the existing winner; it cannot bind this account.
        if state.observed_affinity.is_none()
            && let Some(key) = request.session_affinity_key
            && let Some(effective) = self.claim_initial_session_affinity(key, id).await
            && state.pinned.is_none()
            && &effective != id
        {
            drop(guard);
            state.observed_affinity = Some(effective.clone());
            state.affinity = AffinitySelection::preferred(effective);
            return Ok(None);
        }
        if state.observed_affinity.as_ref() == Some(id)
            && let Some(key) = request.session_affinity_key
        {
            self.update_session_affinity(key, id, id).await;
        }
        Ok(Some(CodexCredentialLease {
            installation_id: runtime.installation_id,
            account: candidate.account,
            authentication: runtime.authentication,
            cookies,
            cyber_policy_scope: state.cyber_policy_scope.clone(),
            diagnostic: false,
            allows_account_state_mutation: true,
            affinity_telemetry: state.affinity.telemetry(id),
            affinity_expected_account_id: expected,
            capacity,
            _guard: guard,
        }))
    }
}
