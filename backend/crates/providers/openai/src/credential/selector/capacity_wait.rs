//! Capacity and strict-affinity admission. Store owns leases; Core owns ordering and the shared clock.

use std::collections::BTreeSet;
use std::future::Future;
use std::num::NonZeroU32;
use std::time::Duration;

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
    deadline: Option<tokio::time::Instant>,
    follows_root: bool,
}

impl<'a> WaitControl<'a> {
    fn new(
        attempt: &'a AttemptContext,
        follows_root: bool,
    ) -> Result<Self, CredentialSelectionError> {
        // An exhausted queue clock only prevents entering another wait. A new
        // attempt may still acquire an immediately available account upstream.
        attempt
            .account_wait_budget()
            .ok_or(CredentialSelectionError::Coordinator(None))?;
        Ok(Self {
            attempt,
            follows_root,
            deadline: attempt
                .deadline()
                .remaining()
                .map(|remaining| tokio::time::Instant::now() + remaining),
        })
    }

    fn enter(
        &mut self,
        mode: AccountWaitMode,
    ) -> Result<AccountWaitDeadline, CredentialSelectionError> {
        let budget = self
            .attempt
            .account_wait_budget()
            .ok_or(CredentialSelectionError::Coordinator(None))?;
        let deadline = if self.follows_root {
            budget.enter_session_affinity()
        } else {
            budget.enter(mode)
        }
        .map_err(|error| match error {
            gateway_core::engine::AccountWaitBudgetError::Expired => {
                CredentialSelectionError::AccountWait(AccountWaitFailure::Timeout)
            }
            _ => CredentialSelectionError::Coordinator(None),
        })?;
        let entered = deadline.monotonic_deadline().into();
        self.deadline = Some(
            self.deadline
                .map_or(entered, |current| current.min(entered)),
        );
        Ok(deadline)
    }

    async fn run<T>(
        &self,
        future: impl Future<Output = Result<T, CredentialSelectionError>>,
    ) -> Result<T, CredentialSelectionError> {
        tokio::select! {
            biased;
            () = self.attempt.cancellation().cancelled() => Err(CredentialSelectionError::Cancelled),
            () = async {
                match self.deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
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
    snapshot_retries: u32,
    assignment_cursor: Option<u64>,
    projection: projection::SelectionProjection,
    follows_root: bool,
    universe: BTreeSet<ProviderAccountId>,
    invalid_credentials: BTreeSet<ProviderAccountId>,
    context: AccountSelectionContext,
    pinned: Option<ProviderAccountId>,
    affinity: AffinitySelection,
    observed_affinity: Option<ProviderAccountId>,
    observed_affinity_binding: Option<ProviderSessionAffinityBinding>,
    cyber_policy_scope: Option<CodexCyberPolicyScope>,
}

enum WaitOutcome {
    Acquired(Box<CodexCredentialLease>),
    Full,
    Changed,
    Retry,
    RootChanged,
    // No business wait: quality uses direct selection; corrupt credentials skip this candidate.
    Skipped,
}

// An interval-only blocker can wait without being classified as capacity Busy.
fn sole_interval_deadline(
    candidate: &AccountCandidate,
    context: &AccountSelectionContext,
    limits: &AccountConcurrencySnapshot,
) -> Option<SystemTime> {
    interval_wait_deadline(candidate, context, limits, false)
}

// Pinned owners and fallback waiters can wait for both interval and capacity.
fn interval_wait_deadline(
    candidate: &AccountCandidate,
    context: &AccountSelectionContext,
    limits: &AccountConcurrencySnapshot,
    allow_busy: bool,
) -> Option<SystemTime> {
    if AccountSelector.availability(candidate, context, limits)
        != AccountSchedulingAvailability::Blocked(AccountSchedulingBlocker::RequestInterval)
    {
        return None;
    }
    let mut without_interval = candidate.clone();
    without_interval.signals.last_started_at = None;
    let available = AccountSelector.availability(&without_interval, context, limits);
    if available != AccountSchedulingAvailability::Ready
        && !(allow_busy && available == AccountSchedulingAvailability::Busy)
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
        universe: Option<&BTreeSet<ProviderAccountId>>,
    ) -> Result<CodexCredentialLease, CredentialSelectionError> {
        let pinned = request.attempt.required_account().is_some()
            || matches!(
                request.attempt.continuation_attempt(),
                ContinuationAttempt::Native | ContinuationAttempt::ReplayOwner
            );
        let result = self
            .select_waiting_inner(request, cyber_policy_key, universe)
            .await;
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
        initial_universe: Option<&BTreeSet<ProviderAccountId>>,
    ) -> Result<CodexCredentialLease, CredentialSelectionError> {
        let mut control = WaitControl::new(request.attempt, request.follows_root())?;
        let mut projection = control.run(self.read_projection(request)).await?;
        let mut accounts = if let Some(owner) = projection.owner() {
            control
                .run(async {
                    self.repository
                        .store()
                        .get_account(owner)
                        .await
                        .map(|account| account.into_iter().collect())
                        .map_err(|source| CredentialSelectionError::Store(Some(source.into())))
                })
                .await?
        } else {
            control.run(self.wait_accounts(request)).await?
        };
        if let Some(universe) = initial_universe {
            accounts.retain(|account| universe.contains(account.id()));
        }
        let ids = accounts
            .iter()
            .map(|account| account.id().clone())
            .collect::<Vec<_>>();
        let scheduling = control
            .run(async {
                self.leases
                    .load_state_for_pool(
                        request.attempt.client_api_key_ref(),
                        &self.provider_kind,
                        &ids,
                        request.concurrency_pool(),
                    )
                    .await
                    .map_err(Into::into)
            })
            .await?;
        let mut candidates = control
            .run(self.wait_candidates(accounts, scheduling.signals(), request))
            .await?;
        let mut projection_valid = false;
        if let Some(cached) = projection.candidates.as_ref() {
            if let Some(current) = candidates.pop()
                && cached.iter().any(|previous| {
                    previous.account.id() == current.account.id()
                        && only_quota_observation_advanced(&previous.account, &current.account)
                })
            {
                candidates = cached
                    .iter()
                    .map(|candidate| {
                        if candidate.account.id() == current.account.id() {
                            current.clone()
                        } else {
                            candidate.clone()
                        }
                    })
                    .collect();
                projection_valid = true;
            } else {
                self.invalidate_projection(&projection);
                projection.candidates = None;
                let accounts = control.run(self.wait_accounts(request)).await?;
                let ids = accounts
                    .iter()
                    .map(|account| account.id().clone())
                    .collect::<Vec<_>>();
                let signals = control
                    .run(async {
                        self.leases
                            .load_signals_for_pool(
                                &self.provider_kind,
                                &ids,
                                request.concurrency_pool(),
                            )
                            .await
                            .map_err(Into::into)
                    })
                    .await?;
                candidates = control
                    .run(self.wait_candidates(accounts, &signals, request))
                    .await?;
            }
        }
        if !projection_valid {
            self.publish_projection(&projection, &candidates);
        }
        drop(projection.fill.take());
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
        let mut pinned = request
            .attempt
            .required_account()
            .cloned()
            .or_else(|| continuation.clone());
        let follows_root = pinned.is_none() && request.follows_root();
        let mut affinity = control
            .run(async {
                self.resolve_session_affinity(
                    request,
                    request.affinity_key(),
                    request
                        .session_affinity_observation
                        .and_then(CodexSessionAffinity::root_key),
                    request
                        .session_affinity_observation
                        .and_then(CodexSessionAffinity::migration_key),
                    request
                        .session_affinity_observation
                        .filter(|_| pinned.is_none())
                        .and_then(CodexSessionAffinity::guardian_parent_preference_key),
                    &candidates,
                    SystemTime::now(),
                    request.attempt.codex_quota_overdraft_eligible(),
                    projection.key.as_ref().map(|_| projection.binding.as_ref()),
                )
                .await
            })
            .await?;
        if request
            .attempt
            .account_selection_policy()
            .openai_account_affinity()
            == gateway_core::account::AccountAffinity::Strict
            && affinity
                .bound_account()
                .zip(continuation.as_ref())
                .is_some_and(|(binding, owner)| binding != owner)
        {
            return Err(CredentialSelectionError::ContinuationOwnerChanged);
        }
        if follows_root {
            pinned = affinity.bound_account().cloned();
        }
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
                        state: tokio::time::timeout(
                            CYBER_POLICY_TIMEOUT,
                            self.session_exclusions.load(&self.provider_kind, key),
                        )
                        .await
                        .map_err(|source| CredentialSelectionError::Store(Some(source.into())))??,
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
        let observed_affinity_binding = if affinity.inherited {
            None
        } else {
            affinity.bound_binding().cloned()
        };
        let observed_affinity = observed_affinity_binding
            .as_ref()
            .map(|binding| binding.account_id().clone());
        let mut universe: BTreeSet<_> = initial_universe
            .cloned()
            .or_else(|| {
                request
                    .attempt
                    .account_scope()
                    .map(|scope| scope.allowed_account_ids().cloned().collect())
            })
            .unwrap_or_else(|| {
                candidates
                    .iter()
                    .map(|candidate| candidate.account.id().clone())
                    .collect()
            });
        if follows_root
            && let Some(owner) = pinned.as_ref()
            && !universe.contains(owner)
        {
            // Enabled-account listing omits a disabled root. Admit only this
            // already-bound, existing, authorized ID to the initial frozen set.
            let account = control
                .run(async {
                    self.repository
                        .store()
                        .get_account(owner)
                        .await
                        .map_err(|error| CredentialSelectionError::Store(Some(error.into())))
                })
                .await?;
            if account
                .as_ref()
                .is_some_and(|account| self.account_in_scope(account, request))
            {
                universe.insert(owner.clone());
            }
        }
        let mut state = WaitingSelection {
            snapshot_retries: 0,
            assignment_cursor: None,
            projection,
            follows_root,
            universe,
            invalid_credentials: BTreeSet::new(),
            context: AccountSelectionContext {
                waiting_counts: Default::default(),
                policy: request.attempt.account_selection_policy(),
                now: SystemTime::now(),
                excluded_accounts: excluded,
                preferred_account: pinned.clone().or_else(|| {
                    (request
                        .attempt
                        .account_selection_policy()
                        .openai_account_affinity()
                        != gateway_core::account::AccountAffinity::Relaxed)
                        .then(|| affinity.preferred_account().cloned())
                        .flatten()
                }),
                preferred_account_overrides_weight: true,
                round_robin_cursor: Self::session_round_robin_cursor(
                    request,
                    observed_affinity_binding.as_ref(),
                    scheduling.round_robin_cursor(),
                ),
                eligibility: if request.attempt.is_quality_retest() {
                    AccountEligibilityPolicy::BypassForQualityRetest
                } else {
                    AccountEligibilityPolicy::Enforce
                },
                allow_codex_quota_overdraft: request.attempt.codex_quota_overdraft_eligible(),
                account_scope: request.attempt.account_scope().cloned(),
                reserved_concurrency: request.reserved_concurrency(),
            },
            pinned,
            affinity,
            observed_affinity,
            observed_affinity_binding,
            cyber_policy_scope,
        };
        let mut sticky_tried = BTreeSet::new();

        // Capacity/cache/affinity rechecks share the request/queue clocks. Only
        // actual account snapshot conflicts consume the upstream retry bound.
        let mut reload = false;
        'rescan: loop {
            let refresh_affinity = reload;
            if reload {
                self.invalidate_projection(&state.projection);
                state.projection.candidates = None;
                candidates = control
                    .run(self.reload_wait_pool(request, &state.universe))
                    .await?;
            }
            reload = true;
            if request.is_cache_preference() && request.affinity_key().is_none() {
                state.observed_affinity = None;
                state.observed_affinity_binding = None;
                state.affinity = AffinitySelection::default();
            }
            if (state.follows_root || (refresh_affinity && state.pinned.is_none()))
                && let Some(key) = request.affinity_key()
            {
                match control
                    .run(async { Ok(self.lookup_request_affinity(request, key).await) })
                    .await?
                {
                    SessionAffinityLookup::Bound(binding) => {
                        if state.follows_root {
                            state.pinned = Some(binding.account_id().clone());
                        }
                        state.observed_affinity = Some(binding.account_id().clone());
                        state.observed_affinity_binding = Some(binding.clone());
                        state.affinity = affinity_selection_for_bound_account(
                            binding,
                            &candidates,
                            SystemTime::now(),
                            request.attempt.codex_quota_overdraft_eligible(),
                        );
                    }
                    SessionAffinityLookup::Unavailable => {
                        return Err(binding_unavailable());
                    }
                    SessionAffinityLookup::Missing => {
                        state.pinned = None;
                        state.observed_affinity = None;
                        state.observed_affinity_binding = None;
                        state.affinity = AffinitySelection::default();
                    }
                }
            }
            control
                .run(self.reload_wait_exclusions(&mut state, request))
                .await?;
            for candidate in &candidates {
                if !request.attempt.is_quality_retest() && candidate.account.quota_paused() {
                    if state.follows_root && state.pinned.as_ref() == Some(candidate.account.id()) {
                        return Err(CredentialSelectionError::QuotaPolicyFiltered);
                    }
                    state
                        .context
                        .excluded_accounts
                        .insert(candidate.account.id().clone());
                }
            }
            if let Some(pin) = &state.pinned {
                candidates.retain(|candidate| candidate.account.id() == pin);
            }
            state.context.now = SystemTime::now();
            state.context.preferred_account = state.pinned.clone().or_else(|| {
                (state.context.policy.openai_account_affinity()
                    != gateway_core::account::AccountAffinity::Relaxed)
                    .then(|| state.affinity.preferred_account().cloned())
                    .flatten()
            });
            state.assignment_cursor = control
                .run(self.assignment_cursor(
                    request,
                    state.observed_affinity_binding.as_ref(),
                    state.pinned.is_some(),
                ))
                .await?;
            if let Some(cursor) = state.assignment_cursor {
                state.context.round_robin_cursor = cursor;
            }
            if state.projection.candidates.is_some() {
                let limits = self.wait_limits()?;
                let ready =
                    AccountSelector.select_with_live_capacity(&candidates, &state.context, &limits);
                if ready.is_none_or(|selected| {
                    Some(selected.candidate().account.id()) != state.projection.owner()
                }) {
                    continue 'rescan;
                }
            }
            if state.follows_root
                && let Some(owner) = state.pinned.clone()
            {
                let limits = self.wait_limits()?;
                let selection =
                    AccountSelector.select_with_live_capacity(&candidates, &state.context, &limits);
                request.attempt.trace().account_selection(
                    &candidates,
                    &state.context,
                    selection.as_ref(),
                );
                match self
                    .wait_for_account(
                        &owner,
                        AccountWaitMode::Sticky,
                        &mut control,
                        request,
                        &mut state,
                    )
                    .await?
                {
                    WaitOutcome::Acquired(lease) => return Ok(*lease),
                    WaitOutcome::Full => {
                        return Err(CredentialSelectionError::AccountWait(
                            AccountWaitFailure::QueueFull,
                        ));
                    }
                    WaitOutcome::RootChanged => {
                        // Root migrations are bounded by the shared wait deadline, not
                        // the snapshot-conflict retry count. Reload the full frozen pool.
                        sticky_tried.clear();
                        continue 'rescan;
                    }
                    WaitOutcome::Retry | WaitOutcome::Changed | WaitOutcome::Skipped => {
                        continue 'rescan;
                    }
                }
            }
            let limits = self.wait_limits()?;
            let strict = state.context.policy.openai_account_affinity()
                == gateway_core::account::AccountAffinity::Strict;
            let original = state.pinned.clone().or_else(|| {
                strict
                    .then(|| state.affinity.bound_account().cloned())
                    .flatten()
            });
            if let Some(original) = original.as_ref()
                && (state.pinned.is_some()
                    || request
                        .attempt
                        .account_selection_policy()
                        .openai_account_affinity()
                        == gateway_core::account::AccountAffinity::Strict)
                && !sticky_tried.contains(original)
                && candidates.iter().any(|candidate| {
                    candidate.account.id() == original
                        && (AccountSelector.availability(candidate, &state.context, &limits)
                            == AccountSchedulingAvailability::Busy
                            || (state.pinned.is_some()
                                && interval_wait_deadline(
                                    candidate,
                                    &state.context,
                                    &limits,
                                    true,
                                )
                                .is_some())
                            || (strict && {
                                let mut without_interval = candidate.clone();
                                without_interval.signals.last_started_at = None;
                                AccountSelector.availability(
                                    &without_interval,
                                    &state.context,
                                    &limits,
                                ) == AccountSchedulingAvailability::Busy
                            }))
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
                    WaitOutcome::RootChanged => {
                        sticky_tried.clear();
                        continue 'rescan;
                    }
                    WaitOutcome::Retry => continue 'rescan,
                    WaitOutcome::Skipped => {}
                    WaitOutcome::Changed => {
                        if state.pinned.is_some() {
                            return Err(CredentialSelectionError::NoEligibleCredential);
                        }
                        continue 'rescan;
                    }
                    WaitOutcome::Full => {
                        if state.pinned.is_some() || strict {
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
                let limits = self.wait_limits()?;
                if request.attempt.is_quality_check()
                    && let Some((id, delay)) = candidates.iter().find_map(|candidate| {
                        quality_interval_delay(
                            request.attempt.is_quality_check(),
                            sole_interval_deadline(candidate, &context, &limits),
                            context.now,
                        )
                        .map(|delay| (candidate.account.id().clone(), delay))
                    })
                {
                    // Quality work is fixed-account administration, not soft affinity.
                    // Wait for the real interval without consuming a business queue slot.
                    control
                        .run(async {
                            tokio::time::sleep(delay).await;
                            Ok(())
                        })
                        .await?;
                    candidates = control
                        .run(self.reload_wait_pool(request, &state.universe))
                        .await?;
                    if let Some(pin) = &state.pinned {
                        candidates.retain(|candidate| candidate.account.id() == pin);
                    }
                    raced.remove(&id);
                    continue;
                }
                context.excluded_accounts.extend(raced.iter().cloned());
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
                    .run(self.reload_wait_exclusions(&mut state, request))
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
                    self.wait_execution_request(&candidate.account, &limits, request)?;
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
                                true,
                            ))
                            .await?
                        {
                            return Ok(lease);
                        }
                        if state.invalid_credentials.contains(&id) {
                            continue;
                        }
                        continue 'rescan;
                    }
                    ProviderLeaseAcquisition::Busy { .. } => {
                        if state.projection.candidates.is_some() {
                            continue 'rescan;
                        }
                        if request.attempt.is_quality_check()
                            && let Some(current) =
                                control.run(self.reload_wait_target(&id, request)).await?
                        {
                            state.context.now = SystemTime::now();
                            if AccountSelector.availability(&current, &state.context, &limits)
                                == AccountSchedulingAvailability::Ready
                                || sole_interval_deadline(&current, &state.context, &limits)
                                    .is_some()
                            {
                                if let Some(old) =
                                    candidates.iter_mut().find(|old| old.account.id() == &id)
                                {
                                    *old = current;
                                }
                                // Redis is authoritative; a concurrent start or clock
                                // boundary must not turn a quality interval into failure.
                                control
                                    .run(async {
                                        tokio::time::sleep(Duration::from_millis(10)).await;
                                        Ok(())
                                    })
                                    .await?;
                                continue;
                            }
                        }
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
                                WaitOutcome::RootChanged => {
                                    sticky_tried.clear();
                                    continue 'rescan;
                                }
                                WaitOutcome::Retry => continue 'rescan,
                                WaitOutcome::Skipped => continue,
                                WaitOutcome::Changed => {
                                    if state.pinned.is_some() {
                                        return Err(CredentialSelectionError::NoEligibleCredential);
                                    }
                                    continue 'rescan;
                                }
                                WaitOutcome::Full if state.pinned.is_some() || strict => {
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
            for candidate in &candidates {
                if !request.attempt.is_quality_retest() && candidate.account.quota_paused() {
                    if state.follows_root && state.pinned.as_ref() == Some(candidate.account.id()) {
                        return Err(CredentialSelectionError::QuotaPolicyFiltered);
                    }
                    state
                        .context
                        .excluded_accounts
                        .insert(candidate.account.id().clone());
                }
            }
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
                if matches!(
                    context.policy.strategy(),
                    gateway_core::account::RotationStrategy::Smart
                        | gateway_core::account::RotationStrategy::Sticky
                ) && context.policy.smart_scheduling().weights()[5] > 0.0
                {
                    let ids = candidates
                        .iter()
                        .map(|candidate| candidate.account.id().clone())
                        .collect::<Vec<_>>();
                    context.waiting_counts = control
                        .run(async {
                            self.leases
                                .load_waiting_counts_for_pool(
                                    &self.provider_kind,
                                    &ids,
                                    request.concurrency_pool(),
                                )
                                .await
                                .map_err(Into::into)
                        })
                        .await?;
                }
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
                    } else if session_policy_excludes_all(
                        state.cyber_policy_scope.as_ref(),
                        &candidates,
                        state.pinned.as_ref(),
                    ) {
                        CredentialSelectionError::SessionPolicyRestricted
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
                    WaitOutcome::RootChanged => {
                        sticky_tried.clear();
                        continue 'rescan;
                    }
                    WaitOutcome::Retry => continue 'rescan,
                    WaitOutcome::Skipped if request.attempt.is_quality_check() => {
                        // A refreshed interval belongs to direct quality waiting, not
                        // the bounded credential/publication conflict retry budget.
                        continue 'rescan;
                    }
                    WaitOutcome::Skipped => continue,
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
    }

    fn wait_limits(&self) -> Result<Arc<AccountConcurrencySnapshot>, CredentialSelectionError> {
        self.account_concurrency
            .load()
            .map_err(|source| CredentialSelectionError::Coordinator(Some(source.into())))?
            .ok_or(CredentialSelectionError::Coordinator(None))
    }

    async fn wait_accounts(
        &self,
        request: &CredentialSelectionInput<'_>,
    ) -> Result<Vec<ProviderAccount>, CredentialSelectionError> {
        let mut accounts = self.scoped_accounts(request).await?;
        if request.attempt.is_quality_retest()
            && let Some(required) = request.attempt.required_account()
            && !accounts.iter().any(|account| account.id() == required)
            && let Some(account) = self
                .repository
                .store()
                .get_account(required)
                .await
                .map_err(|source| CredentialSelectionError::Store(Some(source.into())))?
        {
            accounts.push(account);
        }
        self.retain_excel_auth_blocks(&accounts);
        Ok(accounts)
    }

    async fn reload_wait_exclusions(
        &self,
        state: &mut WaitingSelection,
        request: &CredentialSelectionInput<'_>,
    ) -> Result<(), CredentialSelectionError> {
        state.context.excluded_accounts = request.attempt.excluded_accounts().clone();
        state
            .context
            .excluded_accounts
            .extend(state.invalid_credentials.iter().cloned());
        if let Some(scope) = state.cyber_policy_scope.as_mut() {
            scope.state = tokio::time::timeout(
                CYBER_POLICY_TIMEOUT,
                self.session_exclusions
                    .load(&self.provider_kind, &scope.key),
            )
            .await
            .map_err(|source| CredentialSelectionError::Store(Some(source.into())))??;
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
        let mut accounts = accounts
            .into_iter()
            .filter_map(|account| Self::project_request_account(account, request))
            .filter(|account| self.account_in_scope(account, request))
            .collect::<Vec<_>>();
        self.retain_model_eligible(&mut accounts, request);
        self.prepare_route_scheduling(&accounts).await;
        let cooldowns = self.scheduling_cooldowns(&accounts, request).await;
        for account in accounts {
            let cooldown = cooldowns.get(account.id()).copied().flatten();
            let signals = signals
                .get(account.id())
                .cloned()
                .ok_or(CredentialSelectionError::Coordinator(None))?;
            let signals = self
                .route_scheduling_signals(&account, signals)
                .with_rate_limit(cooldown);
            let signals = quality_selection_signals(signals, request.attempt.is_quality_check());
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
            .wait_accounts(request)
            .await?
            .into_iter()
            .filter(|account| universe.contains(account.id()))
            .collect::<Vec<_>>();
        let ids = accounts
            .iter()
            .map(|account| account.id().clone())
            .collect::<Vec<_>>();
        let signals = self
            .leases
            .load_signals_for_pool(&self.provider_kind, &ids, request.concurrency_pool())
            .await?;
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
            .map_err(|source| CredentialSelectionError::Store(Some(source.into())))?
        else {
            return Ok(None);
        };
        let signals = self
            .leases
            .load_signals_for_pool(
                &self.provider_kind,
                std::slice::from_ref(id),
                request.concurrency_pool(),
            )
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
        request: &CredentialSelectionInput<'_>,
    ) -> Result<ProviderSchedulingLeaseRequest, CredentialSelectionError> {
        Ok(ProviderSchedulingLeaseRequest::new(
            self.provider_kind.clone(),
            account.id().clone(),
            account.revision(),
            quality_concurrency_limit(
                {
                    let ordinary_limit = limits
                        .limit_for(account.id().as_str())
                        .ok_or(CredentialSelectionError::NoEligibleCredential)?;
                    NonZeroU32::new(request.reserved_concurrency()).unwrap_or(ordinary_limit)
                },
                request.attempt.is_quality_check(),
            ),
            request
                .attempt
                .account_selection_policy()
                .request_interval(),
            request.attempt.deadline(),
        )
        .with_quality_check(request.attempt.is_quality_check())
        .with_detail(request.attempt.lease_detail(request.upstream_model))
        .with_cancellation(request.attempt.cancellation().clone())
        .with_concurrency_pool(request.concurrency_pool()))
    }

    async fn wait_for_account(
        &self,
        id: &ProviderAccountId,
        mode: AccountWaitMode,
        control: &mut WaitControl<'_>,
        request: &CredentialSelectionInput<'_>,
        state: &mut WaitingSelection,
    ) -> Result<WaitOutcome, CredentialSelectionError> {
        // Quality leases have separate ownership and cannot promote a business waiter.
        // Keep all callers on the direct-selection interval loop and its request deadline.
        if request.attempt.is_quality_check() {
            return Ok(WaitOutcome::Skipped);
        }
        let candidate = if state.universe.contains(id) {
            control.run(self.reload_wait_target(id, request)).await?
        } else {
            None
        };
        if !request.attempt.is_quality_retest()
            && candidate
                .as_ref()
                .is_some_and(|candidate| candidate.account.quota_paused())
        {
            return if state.follows_root {
                Err(CredentialSelectionError::QuotaPolicyFiltered)
            } else {
                Ok(WaitOutcome::Changed)
            };
        }
        state.context.now = SystemTime::now();
        control
            .run(self.reload_wait_exclusions(state, request))
            .await?;
        let limits = self.wait_limits()?;
        let fixed_wait = state.pinned.as_ref() == Some(id)
            || (state.follows_root
                || (mode == AccountWaitMode::Sticky
                    && state.context.policy.openai_account_affinity()
                        == gateway_core::account::AccountAffinity::Strict))
                && state.observed_affinity.as_ref() == Some(id);
        let retain_on_contention = fixed_wait || mode == AccountWaitMode::Fallback;
        let interval_end = candidate.as_ref().and_then(|candidate| {
            interval_wait_deadline(candidate, &state.context, &limits, retain_on_contention)
        });
        match candidate
            .as_ref()
            .map(|candidate| AccountSelector.availability(candidate, &state.context, &limits))
        {
            Some(AccountSchedulingAvailability::Blocked(
                AccountSchedulingBlocker::RequestInterval,
            )) if interval_end.is_some() => {}
            Some(AccountSchedulingAvailability::Busy) => {}
            Some(AccountSchedulingAvailability::Ready) => {
                let candidate = candidate.as_ref().expect("ready candidate");
                let acquired = control
                    .run(async {
                        self.leases
                            .try_acquire_scheduling(self.wait_execution_request(
                                &candidate.account,
                                &limits,
                                request,
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
                                true,
                            ))
                            .await?
                        {
                            Some(lease) => WaitOutcome::Acquired(Box::new(lease)),
                            None if state.invalid_credentials.contains(id) => WaitOutcome::Skipped,
                            None => WaitOutcome::Retry,
                        },
                    );
                }
                let Some(current) = control.run(self.reload_wait_target(id, request)).await? else {
                    return if state.follows_root {
                        Ok(WaitOutcome::Retry)
                    } else {
                        Ok(WaitOutcome::Changed)
                    };
                };
                state.context.now = SystemTime::now();
                control
                    .run(self.reload_wait_exclusions(state, request))
                    .await?;
                let limits = self.wait_limits()?;
                if matches!(
                    AccountSelector.availability(&current, &state.context, &limits),
                    AccountSchedulingAvailability::Blocked(_)
                ) && !state.follows_root
                    && !(retain_on_contention
                        && interval_wait_deadline(&current, &state.context, &limits, true)
                            .is_some())
                {
                    return Ok(WaitOutcome::Changed);
                }
            }
            _ if state.follows_root => {}
            _ => return Ok(WaitOutcome::Changed),
        }
        let tuning = request.attempt.request_tuning();
        // A skipped interval must not start a new sticky window for later retries.
        if mode == AccountWaitMode::Sticky
            && !fixed_wait
            && interval_end.is_some_and(|end| {
                request
                    .attempt
                    .deadline()
                    .at()
                    .is_some_and(|deadline| end >= deadline)
                    || end.duration_since(SystemTime::now()).unwrap_or_default()
                        >= Duration::from_secs(tuning.account_concurrency_wait_timeout_seconds)
            })
        {
            return Ok(WaitOutcome::Changed);
        }
        let deadline = control.enter(mode)?;
        if mode == AccountWaitMode::Sticky
            && !fixed_wait
            && interval_end.is_some_and(|end| end >= deadline.deadline())
        {
            return Ok(WaitOutcome::Changed);
        }
        let max_waiting = NonZeroU32::new(tuning.max_waiting_per_account)
            .ok_or(CredentialSelectionError::Coordinator(None))?;
        let acquisition = control
            .run(async {
                self.leases
                    .try_acquire_wait(
                        ProviderWaitLeaseRequest::new(
                            self.provider_kind.clone(),
                            id.clone(),
                            request.attempt.request_id().clone(),
                            mode,
                            max_waiting,
                            deadline.deadline(),
                        )
                        .with_concurrency_pool(request.concurrency_pool()),
                    )
                    .await
                    .map_err(Into::into)
            })
            .await?;
        let ProviderWaitLeaseAcquisition::Acquired(mut waiting) = acquisition else {
            return Ok(WaitOutcome::Full);
        };
        let result = control
            .run(async {
                let poll = Duration::from_millis(100);
                let mut next_pool_recheck = tokio::time::Instant::now();
                loop {
                    if let Some(key) = request.affinity_key() {
                        match self.lookup_request_affinity(request, key).await {
                            SessionAffinityLookup::Bound(binding)
                                if Some(&binding) == state.observed_affinity_binding.as_ref() => {}
                            SessionAffinityLookup::Missing
                                if state.observed_affinity_binding.is_none() => {}
                            SessionAffinityLookup::Bound(_) | SessionAffinityLookup::Missing => {
                                return Ok(WaitOutcome::RootChanged);
                            }
                            SessionAffinityLookup::Unavailable => {
                                return Err(binding_unavailable());
                            }
                        }
                    }
                    if !fixed_wait && tokio::time::Instant::now() >= next_pool_recheck {
                        next_pool_recheck =
                            tokio::time::Instant::now() + Duration::from_millis(500);
                        let candidates = self.reload_wait_pool(request, &state.universe).await?;
                        self.reload_wait_exclusions(state, request).await?;
                        let limits = self.wait_limits()?;
                        let mut context = state.context.clone();
                        context.now = SystemTime::now();
                        context.excluded_accounts.insert(id.clone());
                        while let Some(selected) = AccountSelector.select_with_live_capacity(
                            &candidates,
                            &context,
                            &limits,
                        ) {
                            let account = &selected.candidate().account;
                            match self
                                .leases
                                .try_acquire_scheduling(
                                    self.wait_execution_request(account, &limits, request)?,
                                )
                                .await?
                            {
                                ProviderLeaseAcquisition::Acquired(guard) => {
                                    if let Some(lease) = self
                                        .finish_wait_selection(
                                            guard,
                                            limits.revision(),
                                            account,
                                            request,
                                            state,
                                            false,
                                        )
                                        .await?
                                    {
                                        return Ok(WaitOutcome::Acquired(Box::new(lease)));
                                    }
                                    return Ok(WaitOutcome::Retry);
                                }
                                ProviderLeaseAcquisition::Busy { .. } => {
                                    context.excluded_accounts.insert(account.id().clone());
                                }
                            }
                        }
                    }
                    let candidate = if state.universe.contains(id) {
                        self.reload_wait_target(id, request).await?
                    } else {
                        None
                    };
                    let Some(candidate) = candidate else {
                        if state.follows_root {
                            tokio::time::sleep(poll).await;
                            continue;
                        }
                        return Ok(WaitOutcome::Changed);
                    };
                    let limits = self.wait_limits()?;
                    state.context.now = SystemTime::now();
                    self.reload_wait_exclusions(state, request).await?;
                    if !request.attempt.is_quality_retest() && candidate.account.quota_paused() {
                        return if state.follows_root {
                            Err(CredentialSelectionError::QuotaPolicyFiltered)
                        } else {
                            Ok(WaitOutcome::Changed)
                        };
                    }
                    let current_interval = interval_wait_deadline(
                        &candidate,
                        &state.context,
                        &limits,
                        retain_on_contention,
                    )
                    .filter(|current| {
                        retain_on_contention
                            || interval_end
                                .is_some_and(|end| *current <= end && state.context.now < end)
                    });
                    match AccountSelector.availability(&candidate, &state.context, &limits) {
                        AccountSchedulingAvailability::Blocked(
                            AccountSchedulingBlocker::RequestInterval,
                        ) if current_interval.is_some() => {
                            // Fixed owners and fallback waiters retain the same deadline on contention.
                            // Soft affinity still stops when another request moves its original end.
                            let remaining = current_interval
                                .and_then(|end| end.duration_since(SystemTime::now()).ok())
                                .unwrap_or_default();
                            tokio::time::sleep(poll.min(remaining)).await;
                            continue;
                        }
                        AccountSchedulingAvailability::Blocked(_) => {
                            if state.follows_root {
                                // Authentication, cooldown and exclusions cannot grant a
                                // lease. Keep waiting for recovery or a root migration.
                                tokio::time::sleep(poll).await;
                                continue;
                            }
                            return Ok(WaitOutcome::Changed);
                        }
                        AccountSchedulingAvailability::Busy
                            if interval_end.is_some() && !retain_on_contention =>
                        {
                            return Ok(WaitOutcome::Changed);
                        }
                        _ => {}
                    }
                    let promotion = waiting
                        .try_promote(self.wait_execution_request(
                            &candidate.account,
                            &limits,
                            request,
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
                                        false,
                                    )
                                    .await?
                                {
                                    Some(lease) => WaitOutcome::Acquired(Box::new(lease)),
                                    None if state.invalid_credentials.contains(id) => {
                                        WaitOutcome::Skipped
                                    }
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
                        ProviderWaitPromotion::Busy { .. }
                            if interval_end.is_some() && !retain_on_contention =>
                        {
                            return Ok(WaitOutcome::Changed);
                        }
                        ProviderWaitPromotion::Busy { .. } => {}
                    }
                    let jitter =
                        Duration::from_millis(u64::from(uuid::Uuid::new_v4().as_bytes()[0] % 25));
                    tokio::time::sleep(poll + jitter).await;
                }
            })
            .await;
        if let Ok(WaitOutcome::Acquired(lease)) = result {
            // Promotion disarms ownership; cross-account admission must release the original token.
            waiting.release().await?;
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
        immediate: bool,
    ) -> Result<Option<CodexCredentialLease>, CredentialSelectionError> {
        let id = acquired_account.id();
        let Some(mut candidate) = self.reload_wait_target(id, request).await? else {
            retry_account_snapshot(
                request.attempt,
                acquired_account,
                &mut state.snapshot_retries,
            )?;
            return Ok(None);
        };
        let limits = self.wait_limits()?;
        state.context.now = SystemTime::now();
        self.reload_wait_exclusions(state, request).await?;
        // The acquired lease has just written last_started_at. Its own interval
        // is not a reason to reject it; the atomic acquisition checked the prior start.
        let mut acquired_context = state.context.clone();
        acquired_context.policy = gateway_core::account::AccountSelectionPolicy::new(
            state.context.policy.strategy(),
            state.context.policy.max_concurrent_per_account(),
            Duration::ZERO,
        )
        .with_smart_scheduling(state.context.policy.smart_scheduling());
        if !only_quota_observation_advanced(acquired_account, &candidate.account) {
            drop(guard);
            retry_account_snapshot(
                request.attempt,
                acquired_account,
                &mut state.snapshot_retries,
            )?;
            return Ok(None);
        }
        if limits.revision() != revision
            || matches!(
                AccountSelector.availability(&candidate, &acquired_context, &limits),
                AccountSchedulingAvailability::Blocked(_)
            )
            || limits.limit_for(id.as_str()).is_none_or(|limit| {
                candidate.signals.in_flight > acquired_context.limit_for_pool(limit).get()
            })
        {
            drop(guard);
            return Ok(None);
        }
        let runtime = match self
            .repository
            .reload_selected_credential(&candidate.account)
            .await
        {
            Ok((account, runtime)) => {
                candidate.account = account;
                runtime
            }
            Err(CredentialRepositoryError::RevisionConflict(_)) => {
                drop(guard);
                retry_account_snapshot(
                    request.attempt,
                    &candidate.account,
                    &mut state.snapshot_retries,
                )?;
                return Ok(None);
            }
            Err(CredentialRepositoryError::InvalidCredentialData(_)) if state.pinned.is_none() => {
                drop(guard);
                state.invalid_credentials.insert(id.clone());
                state.context.excluded_accounts.insert(id.clone());
                if state.affinity.bound_account() == Some(id) {
                    state.affinity.escape(AffinityEscapeReason::HardUnavailable);
                    state.context.preferred_account = None;
                }
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        if !request.attempt.is_quality_retest() && candidate.account.quota_paused() {
            drop(guard);
            if state.follows_root {
                return Err(CredentialSelectionError::QuotaPolicyFiltered);
            }
            return Ok(None);
        }
        if self.wait_limits()?.revision() != revision {
            drop(guard);
            return Ok(None);
        }
        if self
            .repository
            .current_selected_account(&candidate.account)
            .await?
            .as_ref()
            .is_none_or(|current| !only_quota_observation_advanced(&candidate.account, current))
        {
            drop(guard);
            retry_account_snapshot(
                request.attempt,
                &candidate.account,
                &mut state.snapshot_retries,
            )?;
            return Ok(None);
        }
        if !request.attempt.is_quality_retest()
            && self.excel_auth_block(&candidate.account).is_some()
        {
            drop(guard);
            return Ok(None);
        }
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
        let current = if let Some(cached) = state.projection.candidates.as_ref() {
            cached
                .iter()
                .map(|old| {
                    if old.account.id() == id {
                        candidate.clone()
                    } else {
                        old.clone()
                    }
                })
                .collect()
        } else {
            self.reload_wait_pool(request, &state.universe).await?
        };
        self.reload_wait_exclusions(state, request).await?;
        let current_limits = self.wait_limits()?;
        acquired_context.now = SystemTime::now();
        acquired_context.excluded_accounts = state.context.excluded_accounts.clone();
        if !current
            .iter()
            .any(|latest| only_quota_observation_advanced(&candidate.account, &latest.account))
        {
            drop(guard);
            retry_account_snapshot(
                request.attempt,
                &candidate.account,
                &mut state.snapshot_retries,
            )?;
            return Ok(None);
        }
        if current_limits.revision() != revision
            || !current.iter().any(|latest| {
                only_quota_observation_advanced(&candidate.account, &latest.account)
                    && !matches!(
                        AccountSelector.availability(latest, &acquired_context, &current_limits),
                        AccountSchedulingAvailability::Blocked(_)
                    )
                    && current_limits.limit_for(id.as_str()).is_some_and(|limit| {
                        latest.signals.in_flight <= acquired_context.limit_for_pool(limit).get()
                    })
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
        let Some(expected) = self
            .admit_selected_session(
                request,
                state.observed_affinity_binding.as_ref(),
                id,
                state.pinned.is_some(),
                None,
                state
                    .assignment_cursor
                    .filter(|_| immediate && state.context.preferred_account.as_ref() != Some(id)),
            )
            .await?
        else {
            guard.release().await?;
            return Ok(None);
        };
        if state.follows_root
            && let Some(key) = request.affinity_key()
            && let Err(error) = self.verify_followed_root(key, &expected).await
        {
            if matches!(error, CredentialSelectionError::CapacityUnavailable { .. }) {
                drop(guard);
                return Ok(None);
            }
            return Err(error);
        }
        if let Some(affinity) = request.session_affinity_observation
            && !self
                .remember_turn_alias(
                    affinity,
                    request
                        .attempt
                        .account_selection_policy()
                        .openai_session_binding_ttl(),
                )
                .await?
        {
            return Err(binding_unavailable());
        }
        if !request.attempt.is_quality_retest()
            && self.excel_auth_block(&candidate.account).is_some()
        {
            drop(guard);
            return Ok(None);
        }
        Ok(Some(CodexCredentialLease {
            selected_route: candidate.account.credential_route(),
            session_binding_ttl: request
                .attempt
                .account_selection_policy()
                .openai_session_binding_ttl(),
            installation_id: runtime.installation_id,
            account: candidate.account,
            authentication: runtime.authentication,
            cookies,
            cyber_policy_scope: state.cyber_policy_scope.clone(),
            diagnostic: false,
            allows_account_state_mutation: true,
            affinity_telemetry: state.affinity.telemetry(id, request.is_cache_preference()),
            affinity_expected_binding: expected,
            guardian_parent_record_key: request
                .session_affinity_observation
                .and_then(CodexSessionAffinity::guardian_parent_record_key)
                .cloned(),
            capacity,
            _guard: guard,
        }))
    }
}

fn retry_account_snapshot(
    attempt: &AttemptContext,
    account: &ProviderAccount,
    retries: &mut u32,
) -> Result<(), CredentialSelectionError> {
    let retry = *retries < 3;
    if retry {
        *retries += 1;
    }
    attempt.trace().record(
        "account.snapshot_conflict",
        serde_json::json!({
            "accountId": account.id().as_str(),
            "credentialRevision": account.revision().get(),
            "retry": retry,
            "retryCount": *retries,
            "maxRetries": 3,
        }),
    );
    if retry {
        Ok(())
    } else {
        Err(CredentialSelectionError::AccountSnapshotChanged)
    }
}
