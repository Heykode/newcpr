use super::{candidate, candidate_with_concurrency, context};
use gateway_core::account::{
    AccountQuotaResetWindow, AccountSelector, QuotaResetPeriod, RotationStrategy,
    smart_scheduling::SmartSchedulingConfig,
};
use serde_json::json;
use std::time::Duration;

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
fn zero_or_absent_window_weights_disable_reset_scores_and_tolerance() {
    let mut context = context(RotationStrategy::Smart);
    let mut candidate = candidate_with_concurrency("acct_reset", 0, 2);
    candidate.signals.quota_reset_windows = Some(
        [AccountQuotaResetWindow {
            period: QuotaResetPeriod::Monthly,
            reset_at: context.now + Duration::from_secs(1),
        }]
        .into(),
    );
    let legacy = json!({"loadWeight":0,"quotaWeight":0,"healthWeight":0,"latencyWeight":0,"resetWeight":10,"queueWeight":0,"preferHigherWeight":false});
    let mut disabled = legacy.clone();
    disabled["resetWindowWeights"] = json!({"fiveHour":0,"weekly":0,"monthly":0});
    for wire in [legacy, disabled] {
        let config: SmartSchedulingConfig = serde_json::from_value(wire).unwrap();
        context.policy = context.policy.with_smart_scheduling(config);
        let trace = gateway_core::diagnostics::TraceContext::new("req_zero_windows");
        trace.account_selection(std::slice::from_ref(&candidate), &context, None);
        let events = trace.snapshot().unwrap();
        let data = &events["events"][0]["data"];
        assert_eq!(data["candidates"][0]["smartScore"], 0.0);
        assert_eq!(data["smartScoreTolerance"], 0.0);
        assert_eq!(
            serde_json::to_value(config).unwrap()["resetWindowWeights"],
            json!({"fiveHour":0.0,"weekly":0.0,"monthly":0.0})
        );
    }
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
fn reset_weight_rewards_only_future_resets_and_preserves_capacity_checks() {
    let mut context = context(RotationStrategy::Smart);
    context.policy = context.policy.with_smart_scheduling(serde_json::from_value(json!({
        "loadWeight":0, "quotaWeight":0, "healthWeight":0, "latencyWeight":0, "resetWeight":1, "queueWeight":0,"preferHigherWeight":false,
        "resetWindowWeights":{"fiveHour":0,"weekly":1,"monthly":0}
    })).unwrap());
    let mut candidates = [
        candidate_with_concurrency("acct_unknown", 0, 2),
        candidate_with_concurrency("acct_past", 0, 2),
        candidate_with_concurrency("acct_now", 0, 2),
        candidate_with_concurrency("acct_near", 0, 2),
        candidate_with_concurrency("acct_far", 0, 2),
    ];
    for (candidate, reset_at) in candidates[1..].iter_mut().zip([
        context.now - Duration::from_secs(1),
        context.now,
        context.now + Duration::from_secs(3600),
        context.now + Duration::from_secs(86_400),
    ]) {
        candidate.signals.quota_reset_windows = Some(
            [AccountQuotaResetWindow {
                period: QuotaResetPeriod::Weekly,
                reset_at,
            }]
            .into(),
        );
    }
    assert_eq!(
        AccountSelector
            .select(&candidates, &context)
            .unwrap()
            .candidate()
            .account
            .id()
            .as_str(),
        "acct_near"
    );
    let trace = gateway_core::diagnostics::TraceContext::new("req_reset_scores");
    trace.account_selection(&candidates, &context, None);
    let snapshot = trace.snapshot().unwrap();
    let scores = snapshot["events"][0]["data"]["candidates"]
        .as_array()
        .unwrap();
    for (candidate, expected) in scores.iter().zip([0.0, 0.0, 0.0, 0.96, 0.5]) {
        assert!((candidate["smartScore"].as_f64().unwrap() - expected).abs() < 1e-10);
    }
    candidates[3].signals.in_flight = 2;
    assert_eq!(
        AccountSelector
            .select(&candidates, &context)
            .unwrap()
            .candidate()
            .account
            .id()
            .as_str(),
        "acct_far"
    );
    candidates[3].signals.in_flight = 0;
    context.now += Duration::from_secs(3600);
    assert_eq!(
        AccountSelector
            .select(&candidates, &context)
            .unwrap()
            .candidate()
            .account
            .id()
            .as_str(),
        "acct_far"
    );
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

#[test]
fn missing_expired_and_duplicate_periods_do_not_inflate_reset_score() {
    let mut context = context(RotationStrategy::Smart);
    let mut candidate = candidate_with_concurrency("acct_partial", 0, 2);
    candidate.signals.quota_reset_at = Some(context.now + Duration::from_secs(1));
    candidate.signals.quota_reset_windows = Some(
        [
            (QuotaResetPeriod::Weekly, 86400),
            (QuotaResetPeriod::Weekly, 90000),
            (QuotaResetPeriod::Weekly, 0),
            (QuotaResetPeriod::FiveHour, 0),
        ]
        .map(|(period, delta)| AccountQuotaResetWindow {
            period,
            reset_at: context.now + Duration::from_secs(delta),
        })
        .into(),
    );
    for scale in [1.0, 0.5] {
        let mut config = serde_json::to_value(
            SmartSchedulingConfig::new([0.0, 0.0, 0.0, 0.0, 1.0, 0.0], false).unwrap(),
        )
        .unwrap();
        config["resetWindowWeights"] = json!({
            "weekly": 2.0*scale, "fiveHour": 4.0*scale, "monthly": 4.0*scale
        });
        context.policy = context
            .policy
            .with_smart_scheduling(serde_json::from_value(config).unwrap());
        let trace = gateway_core::diagnostics::TraceContext::new("req_partial");
        trace.account_selection(std::slice::from_ref(&candidate), &context, None);
        assert_eq!(
            trace.snapshot().unwrap()["events"][0]["data"]["candidates"][0]["smartScore"],
            0.1
        );
    }
}
