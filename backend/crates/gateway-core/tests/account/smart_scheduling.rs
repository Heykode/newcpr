use super::{candidate, context};
use gateway_core::account::{
    AccountSelector, RotationStrategy, smart_scheduling::SmartSchedulingConfig,
};

#[test]
fn weights_validate_and_roundtrip_without_float_drift() {
    let defaults = SmartSchedulingConfig::default();
    assert_eq!(defaults.weights(), [1.0, 0.8, 1.0, 0.5, 0.0, 0.0]);
    assert!(!defaults.prefer_higher_weight());
    assert_eq!(
        serde_json::from_value::<SmartSchedulingConfig>(serde_json::to_value(defaults).unwrap())
            .unwrap(),
        defaults
    );
    for invalid in [-0.1, 10.1, 0.01, f64::NAN, f64::INFINITY] {
        assert!(SmartSchedulingConfig::new([invalid, 1.0, 0.0, 0.0, 0.0, 0.0], false).is_err());
    }
    assert!(SmartSchedulingConfig::new([0.0; 6], false).is_err());
}

#[test]
fn smart_and_sticky_share_scores_but_preserve_original_preference() {
    let mut a = candidate("acct_a", 0, None);
    let mut b = candidate("acct_b", 1, None);
    a.signals.quota_remaining_rank = Some(0);
    b.signals.quota_remaining_rank = Some(10000);
    let candidates = [a, b];
    for strategy in [RotationStrategy::Smart, RotationStrategy::Sticky] {
        let mut ctx = context(strategy);
        ctx.policy = ctx.policy.with_smart_scheduling(
            SmartSchedulingConfig::new([0.0, 1.0, 0.0, 0.0, 0.0, 0.0], false).unwrap(),
        );
        assert_eq!(
            AccountSelector
                .select(&candidates, &ctx)
                .unwrap()
                .candidate()
                .account
                .id()
                .as_str(),
            "acct_b"
        );
        ctx.preferred_account = Some(candidates[0].account.id().clone());
        assert_eq!(
            AccountSelector
                .select(&candidates, &ctx)
                .unwrap()
                .candidate()
                .account
                .id()
                .as_str(),
            "acct_a"
        );
    }
    let mut rr = context(RotationStrategy::RoundRobin);
    rr.policy = rr.policy.with_smart_scheduling(
        SmartSchedulingConfig::new([0.0, 1.0, 0.0, 0.0, 0.0, 0.0], true).unwrap(),
    );
    assert_eq!(
        AccountSelector
            .select(&candidates, &rr)
            .unwrap()
            .candidate()
            .account
            .id()
            .as_str(),
        "acct_a"
    );
    assert!(rr.policy.preferred_account_overrides_weight());
}

#[test]
fn queue_pressure_breaks_ties_only_when_supplied_for_waiting() {
    let candidates = [candidate("acct_a", 0, None), candidate("acct_b", 0, None)];
    let mut ctx = context(RotationStrategy::Smart);
    ctx.policy = ctx.policy.with_smart_scheduling(
        SmartSchedulingConfig::new([0.0, 0.0, 0.0, 0.0, 0.0, 1.0], false).unwrap(),
    );
    assert_eq!(
        AccountSelector
            .select(&candidates, &ctx)
            .unwrap()
            .candidate()
            .account
            .id()
            .as_str(),
        "acct_a"
    );
    ctx.waiting_counts
        .insert(candidates[0].account.id().clone(), 3);
    assert_eq!(
        AccountSelector
            .select(&candidates, &ctx)
            .unwrap()
            .candidate()
            .account
            .id()
            .as_str(),
        "acct_b"
    );
}

#[test]
fn switchback_is_opt_in_and_never_overrides_a_hard_preference() {
    let candidates = [
        super::weighted_candidate("acct_a", 10, 0),
        super::weighted_candidate("acct_b", 90, 0),
    ];
    for strategy in [RotationStrategy::Smart, RotationStrategy::Sticky] {
        for (switchback, pinned, expected) in [
            (false, false, "acct_a"),
            (true, false, "acct_b"),
            (true, true, "acct_a"),
        ] {
            let mut ctx = context(strategy);
            ctx.policy = ctx.policy.with_smart_scheduling(
                SmartSchedulingConfig::new([1.0, 0.8, 1.0, 0.5, 0.0, 0.0], switchback).unwrap(),
            );
            ctx.preferred_account = Some(candidates[0].account.id().clone());
            ctx.preferred_account_overrides_weight =
                pinned || ctx.policy.preferred_account_overrides_weight();
            assert_eq!(
                AccountSelector
                    .select(&candidates, &ctx)
                    .unwrap()
                    .candidate()
                    .account
                    .id()
                    .as_str(),
                expected
            );
        }
    }
}

#[test]
fn expired_reset_does_not_score_as_an_imminent_reset() {
    let mut ctx = context(RotationStrategy::Smart);
    ctx.policy = ctx.policy.with_smart_scheduling(
        SmartSchedulingConfig::new([0.0, 0.0, 0.0, 0.0, 1.0, 0.0], false).unwrap(),
    );
    let mut expired = candidate("acct_a", 0, None);
    let mut future = candidate("acct_b", 0, None);
    expired.signals.quota_reset_at = Some(ctx.now - std::time::Duration::from_secs(1));
    future.signals.quota_reset_at = Some(ctx.now + std::time::Duration::from_secs(3600));
    let mut candidates = [expired, future];
    for expired_at in [candidates[0].signals.quota_reset_at, Some(ctx.now), None] {
        candidates[0].signals.quota_reset_at = expired_at;
        assert_eq!(
            AccountSelector
                .select(&candidates, &ctx)
                .unwrap()
                .candidate()
                .account
                .id()
                .as_str(),
            "acct_b"
        );
    }
}

#[test]
fn proportional_weights_preserve_near_score_rotation() {
    let candidates = [
        candidate("acct_a", 0, Some(5000)),
        candidate("acct_b", 0, Some(5100)),
    ];
    for scale in [1.0, 2.0, 4.0] {
        let mut ctx = context(RotationStrategy::Smart);
        ctx.policy = ctx.policy.with_smart_scheduling(
            SmartSchedulingConfig::new([scale, 0.8 * scale, scale, 0.5 * scale, 0.0, 0.0], false)
                .unwrap(),
        );
        for (cursor, expected) in [(0, "acct_a"), (1, "acct_b")] {
            ctx.round_robin_cursor = cursor;
            assert_eq!(
                AccountSelector
                    .select(&candidates, &ctx)
                    .unwrap()
                    .candidate()
                    .account
                    .id()
                    .as_str(),
                expected
            );
        }
    }
}
