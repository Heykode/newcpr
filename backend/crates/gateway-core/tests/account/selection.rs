use std::time::{Duration, Instant};

use gateway_core::account::{
    AccountAttemptFeedback, AccountCandidate, AccountFeedbackStats, AccountSelector,
    ProviderAccountId, RotationStrategy,
};
use gateway_core::routing::ProviderKind;

use super::{candidate, candidate_with_concurrency, context};

const FAILURE_RATE_HALF_LIFE: Duration = Duration::from_secs(15 * 60);

fn live_limits(values: &[(&str, u32)]) -> gateway_core::runtime::AccountConcurrencySnapshot {
    gateway_core::runtime::AccountConcurrencySnapshot::new(
        gateway_core::routing::ConfigRevision::new(1).unwrap(),
        values
            .iter()
            .map(|(id, limit)| ((*id).to_owned(), std::num::NonZeroU32::new(*limit).unwrap()))
            .collect(),
    )
}

#[test]
fn account_wait_limits_accept_shared_snapshots_and_neutral_implementations() {
    use std::num::NonZeroU32;
    use std::sync::Arc;

    use gateway_core::account::{
        AccountConcurrencyLimits, AccountSchedulingAvailability as Availability,
        AccountSchedulingBlocker,
    };

    struct MissingLimits;
    impl AccountConcurrencyLimits for MissingLimits {
        fn limit_for(&self, _: &str) -> Option<NonZeroU32> {
            None
        }
    }

    let candidates = vec![candidate("acct_busy", 1, None)];
    let ctx = context(RotationStrategy::Smart);
    let shared = Arc::new(live_limits(&[("acct_busy", 1)]));
    let erased: Arc<dyn AccountConcurrencyLimits> = shared.clone();
    for limits in [&shared as &dyn AccountConcurrencyLimits, &erased] {
        assert_eq!(
            AccountSelector.availability(&candidates[0], &ctx, limits),
            Availability::Busy
        );
        assert!(
            AccountSelector
                .select_with_live_capacity(&candidates, &ctx, limits)
                .is_none()
        );
        assert!(
            AccountSelector
                .select_for_capacity_wait(&candidates, &ctx, limits)
                .is_some()
        );
        assert_eq!(
            AccountSelector
                .capacity_snapshot_with_live_limits(&candidates, &ctx, limits)
                .unwrap()
                .total_slots(),
            1
        );
    }
    assert_eq!(
        AccountSelector.availability(&candidates[0], &ctx, &MissingLimits),
        Availability::Blocked(AccountSchedulingBlocker::LocalAvailability)
    );
}

#[test]
fn account_wait_classification_distinguishes_busy_unhealthy_and_live_capacity() {
    use gateway_core::account::{
        AccountSchedulingAvailability as Availability, AccountSchedulingBlocker,
    };
    let candidates = vec![
        candidate_with_concurrency("acct_busy", 2, 9),
        candidate("acct_ready", 0, None),
    ];
    let ctx = context(RotationStrategy::Smart);
    let limits = live_limits(&[("acct_busy", 2), ("acct_ready", 1)]);
    assert_eq!(
        AccountSelector.availability(&candidates[0], &ctx, &limits),
        Availability::Busy
    );
    assert_eq!(
        AccountSelector.availability(&candidates[1], &ctx, &limits),
        Availability::Ready
    );
    assert_eq!(
        AccountSelector
            .select_for_capacity_wait(&candidates, &ctx, &limits)
            .unwrap()
            .candidate()
            .account
            .id()
            .as_str(),
        "acct_busy"
    );
    assert_eq!(
        AccountSelector
            .select_with_live_capacity(&candidates, &ctx, &limits)
            .unwrap()
            .candidate()
            .account
            .id()
            .as_str(),
        "acct_ready"
    );
    assert_eq!(
        AccountSelector.availability(&candidates[0], &ctx, &live_limits(&[])),
        Availability::Blocked(AccountSchedulingBlocker::LocalAvailability)
    );
    assert_eq!(
        AccountSelector.availability(&candidates[0], &ctx, &live_limits(&[("acct_busy", 3)])),
        Availability::Ready
    );
    assert_eq!(
        candidates[0].account.concurrency_limit().unwrap().get(),
        9,
        "live selection must not rewrite credential facts"
    );
    assert_eq!(candidates[0].signals.in_flight, 2);
}

#[test]
fn account_wait_never_masks_non_capacity_blockers() {
    use gateway_core::account::{
        AccountSchedulingAvailability as Availability, AccountSchedulingBlocker,
        AccountSelectionPolicy, CredentialState, QuotaEvidence, QuotaState,
    };
    let limits = live_limits(&[("acct_busy", 1)]);
    let mut ctx = context(RotationStrategy::Smart);
    let busy = candidate("acct_busy", 1, None);
    for account in [
        busy.account.clone().with_account_facts(
            false,
            CredentialState::Ready,
            QuotaState::unknown(),
            None,
            None,
        ),
        busy.account.clone().with_account_facts(
            true,
            CredentialState::Ready,
            QuotaState::exhausted(QuotaEvidence::UsageLimitReached, ctx.now, None),
            None,
            None,
        ),
    ] {
        let rejected = AccountCandidate {
            account,
            signals: busy.signals.clone(),
        };
        assert_eq!(
            AccountSelector.availability(&rejected, &ctx, &limits),
            Availability::Blocked(AccountSchedulingBlocker::LocalAvailability)
        );
    }
    ctx.excluded_accounts.insert(busy.account.id().clone());
    assert_eq!(
        AccountSelector.availability(&busy, &ctx, &limits),
        Availability::Blocked(AccountSchedulingBlocker::Excluded)
    );
    ctx.excluded_accounts.clear();
    ctx.policy = AccountSelectionPolicy::new(
        RotationStrategy::Smart,
        std::num::NonZeroU32::new(1).unwrap(),
        Duration::from_secs(5),
    );
    let mut interval_busy = busy.clone();
    interval_busy.signals.last_started_at = Some(ctx.now);
    assert_eq!(
        AccountSelector.availability(&interval_busy, &ctx, &limits),
        Availability::Blocked(AccountSchedulingBlocker::RequestInterval)
    );
    assert!(
        AccountSelector
            .select_for_capacity_wait(&[interval_busy], &ctx, &limits)
            .is_none()
    );
    let mut cooled = busy;
    cooled.signals.rate_limited_until = Some(ctx.now + Duration::from_secs(60));
    assert_eq!(
        AccountSelector.availability(&cooled, &ctx, &limits),
        Availability::Blocked(AccountSchedulingBlocker::LocalAvailability)
    );
}

#[test]
fn account_wait_live_selection_keeps_existing_strategy_weight_and_affinity() {
    for strategy in [
        RotationStrategy::Smart,
        RotationStrategy::Sticky,
        RotationStrategy::RoundRobin,
        RotationStrategy::QuotaResetPriority,
    ] {
        let candidates = vec![
            super::weighted_candidate("acct_one", 10, 1),
            super::weighted_candidate("acct_two", 80, 1),
        ];
        let limits = live_limits(&[("acct_one", 3), ("acct_two", 3)]);
        let mut ctx = context(strategy);
        for cursor in 0..4 {
            ctx.round_robin_cursor = cursor;
            assert_eq!(
                AccountSelector
                    .select(&candidates, &ctx)
                    .unwrap()
                    .candidate()
                    .account
                    .id(),
                AccountSelector
                    .select_with_live_capacity(&candidates, &ctx, &limits)
                    .unwrap()
                    .candidate()
                    .account
                    .id()
            );
        }
        let full = live_limits(&[("acct_one", 1), ("acct_two", 1)]);
        assert_eq!(
            AccountSelector
                .select_for_capacity_wait(&candidates, &ctx, &full)
                .unwrap()
                .candidate()
                .account
                .id()
                .as_str(),
            "acct_two"
        );
        ctx.preferred_account = Some(candidates[0].account.id().clone());
        ctx.preferred_account_overrides_weight = true;
        assert_eq!(
            AccountSelector
                .select_for_capacity_wait(&candidates, &ctx, &full)
                .unwrap()
                .candidate()
                .account
                .id()
                .as_str(),
            "acct_one"
        );
    }
}

#[test]
fn account_wait_capacity_uses_live_slots_without_counting_waiters() {
    let candidates = vec![
        candidate_with_concurrency("acct_one", 2, 90),
        candidate("acct_two", 1, None),
    ];
    let limits = live_limits(&[("acct_one", 2), ("acct_two", 4)]);
    let ctx = context(RotationStrategy::Smart);
    let capacity = AccountSelector
        .capacity_snapshot_with_live_limits(&candidates, &ctx, &limits)
        .unwrap();
    assert_eq!(capacity.used_slots(), 3);
    assert_eq!(capacity.total_slots(), 6);
    assert!(
        AccountSelector
            .capacity_snapshot_with_live_limits(&candidates, &ctx, &live_limits(&[]))
            .is_none()
    );
}

fn feedback_subject() -> (AccountFeedbackStats, ProviderKind, ProviderAccountId) {
    (
        AccountFeedbackStats::default(),
        ProviderKind::new("openai").expect("valid provider"),
        ProviderAccountId::new("acct_decay").expect("valid account"),
    )
}

fn report_failure(
    feedback: &AccountFeedbackStats,
    provider: &ProviderKind,
    account: &ProviderAccountId,
    observed_at: Instant,
) {
    feedback.report_at(
        provider,
        account,
        AccountAttemptFeedback::Failed {
            first_output_ms: None,
        },
        observed_at,
    );
}

#[test]
fn account_failure_rate_should_halve_after_one_half_life() {
    let (feedback, provider, account) = feedback_subject();
    let observed_at = Instant::now();
    report_failure(&feedback, &provider, &account, observed_at);

    let failure_rate = feedback
        .scheduling_signals_at(&provider, &account, observed_at + FAILURE_RATE_HALF_LIFE)
        .0;

    assert_eq!(failure_rate, Some(1_000));
}

#[test]
fn account_failure_rate_should_quarter_after_two_half_lives() {
    let (feedback, provider, account) = feedback_subject();
    let observed_at = Instant::now();
    report_failure(&feedback, &provider, &account, observed_at);

    let failure_rate = feedback
        .scheduling_signals_at(
            &provider,
            &account,
            observed_at + FAILURE_RATE_HALF_LIFE * 2,
        )
        .0;

    assert_eq!(failure_rate, Some(500));
}

#[test]
fn account_failure_rate_should_decay_before_applying_a_new_sample() {
    let (feedback, provider, account) = feedback_subject();
    let observed_at = Instant::now();
    report_failure(&feedback, &provider, &account, observed_at);
    feedback.report_at(
        &provider,
        &account,
        AccountAttemptFeedback::Succeeded {
            first_output_ms: None,
        },
        observed_at + FAILURE_RATE_HALF_LIFE,
    );

    let failure_rate = feedback
        .scheduling_signals_at(&provider, &account, observed_at + FAILURE_RATE_HALF_LIFE)
        .0;

    assert_eq!(failure_rate, Some(800));
}

#[test]
fn concurrent_account_failures_should_not_lose_samples() {
    let (feedback, provider, account) = feedback_subject();
    let observed_at = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| report_failure(&feedback, &provider, &account, observed_at));
        }
    });

    let failure_rate = feedback
        .scheduling_signals_at(&provider, &account, observed_at)
        .0;

    assert_eq!(failure_rate, Some(8_322));
}
fn smart_selection_ids(candidates: &[AccountCandidate]) -> Vec<&str> {
    let mut selection = context(RotationStrategy::Smart);
    (0..20)
        .map(|cursor| {
            selection.round_robin_cursor = cursor;
            AccountSelector
                .select(candidates, &selection)
                .expect("candidate available")
                .candidate()
                .account
                .id()
                .as_str()
        })
        .collect()
}

#[test]
fn smart_selector_should_rotate_despite_small_signal_differences() {
    for signal in ["quota", "latency", "load", "failure"] {
        let mut candidates = [
            candidate_with_concurrency("acct_a", 0, 100),
            candidate_with_concurrency("acct_b", 0, 100),
        ];
        match signal {
            "quota" => {
                candidates[0].signals.quota_remaining_rank = Some(600);
                candidates[1].signals.quota_remaining_rank = Some(601);
            }
            "latency" => {
                candidates[0].signals.first_output_latency_ms = Some(2_500);
                candidates[1].signals.first_output_latency_ms = Some(2_501);
            }
            "load" => candidates[0].signals.in_flight = 1,
            "failure" => candidates[0].signals.failure_rate_basis_points = Some(1),
            _ => unreachable!(),
        }
        let selected = smart_selection_ids(&candidates);

        assert_eq!(selected, ["acct_a", "acct_b"].repeat(10), "{signal}");
    }
}

#[test]
fn smart_selector_should_keep_rotation_order_when_nearby_scores_cross() {
    let mut candidates = [
        candidate("acct_a", 0, Some(8_000)),
        candidate("acct_b", 0, Some(8_001)),
        candidate("acct_worse", 0, Some(2_000)),
    ];
    let mut selection = context(RotationStrategy::Smart);
    let mut selected = Vec::new();
    for cursor in 0..20 {
        selection.round_robin_cursor = cursor;
        for candidate in &mut candidates {
            match candidate.account.id().as_str() {
                "acct_a" => candidate.signals.quota_remaining_rank = Some(8_000 + cursor % 2),
                "acct_b" => candidate.signals.quota_remaining_rank = Some(8_001 - cursor % 2),
                _ => {}
            }
        }
        candidates.reverse();
        selected.push(
            AccountSelector
                .select(&candidates, &selection)
                .expect("candidate available")
                .candidate()
                .account
                .id()
                .as_str()
                .to_owned(),
        );
    }

    assert_eq!(selected, ["acct_a", "acct_b"].repeat(10));
}

#[test]
fn smart_selector_should_only_rotate_among_candidates_close_to_the_best() {
    let candidates = [
        candidate("acct_a", 0, Some(10_000)),
        candidate("acct_b", 0, Some(9_500)),
        candidate("acct_c", 0, Some(9_000)),
    ];

    assert_eq!(
        smart_selection_ids(&candidates),
        ["acct_a", "acct_b"].repeat(10)
    );
}

#[test]
fn smart_selector_should_preserve_material_signal_advantages() {
    for signal in ["quota", "latency", "load", "failure"] {
        let mut candidates = [
            candidate_with_concurrency("acct_a", 0, 10),
            candidate_with_concurrency("acct_b", 0, 10),
        ];
        match signal {
            "quota" => {
                candidates[0].signals.quota_remaining_rank = Some(600);
                candidates[1].signals.quota_remaining_rank = Some(2_600);
            }
            "latency" => {
                candidates[0].signals.first_output_latency_ms = Some(20_000);
                candidates[1].signals.first_output_latency_ms = Some(2_500);
            }
            "load" => candidates[0].signals.in_flight = 1,
            "failure" => candidates[0].signals.failure_rate_basis_points = Some(2_000),
            _ => unreachable!(),
        }

        assert_eq!(smart_selection_ids(&candidates), ["acct_b"; 20], "{signal}");
    }
}

#[test]
fn smart_selector_should_balance_actual_load_against_remaining_quota() {
    let mut candidates = [
        candidate_with_concurrency("acct_a", 0, 10),
        candidate_with_concurrency("acct_b", 1, 10),
    ];
    candidates[0].signals.quota_remaining_rank = Some(600);
    candidates[1].signals.quota_remaining_rank = Some(2_600);

    assert_eq!(smart_selection_ids(&candidates), ["acct_b"; 20]);
}

#[test]
fn smart_selector_should_treat_unknown_quota_as_neutral() {
    let candidates = [
        candidate("acct_known_low", 0, Some(600)),
        candidate("acct_unknown", 0, None),
    ];

    assert_eq!(smart_selection_ids(&candidates), ["acct_unknown"; 20]);
}
