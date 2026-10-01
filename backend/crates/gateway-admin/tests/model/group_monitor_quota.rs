use chrono::Duration;
use gateway_admin::model::{
    group_monitor::monitor_learned_remaining,
    group_monitor_quota::{monitor_learned_windows, monitor_learning_observations},
    provider_credentials::{ProviderQuota, ProviderQuotaWindowRole, QuotaLocalUsageAttribution},
    quota_learning::{QuotaLearningEstimate, QuotaLearningObservation, QuotaLearningSource},
};

use super::quota_forecast::{now, quota, window};

fn observations(source: &ProviderQuota) -> Vec<QuotaLearningObservation> {
    monitor_learning_observations("acct_monitor", "openai", Some("Plus"), source, now())
}

fn estimate(observation: &QuotaLearningObservation, limit: f64) -> QuotaLearningEstimate {
    QuotaLearningEstimate {
        account_id: observation.account_id.clone(),
        window_key: observation.window_key.clone(),
        window_minutes: observation.window_minutes,
        effective_limit_usd: Some(limit),
        source: QuotaLearningSource::PlanAverage,
        sample_count: 3,
    }
}

#[test]
fn paid_new_accounts_keep_learned_reference_without_using_small_percentage_ratios() {
    let mut source = quota(vec![window("week", 7)]);
    for (percent, remaining) in [(0.0, 120.0), (1.0, 118.8), (100.0, 0.0)] {
        source.windows[0].used_percent = Some(percent);
        // This unaligned list cost must never replace the stored reference.
        source.windows[0].local_usage.as_mut().unwrap().costs[0].amount = "9999".parse().unwrap();
        let observations = observations(&source);
        assert_eq!(observations[0].observed_cost_usd, None);
        let learned =
            monitor_learned_windows(&source, &observations, &[estimate(&observations[0], 120.0)]);
        assert!((learned[0].remaining_usd.unwrap() - remaining).abs() < 1e-8);
        assert!(!learned[0].low_sample);
        assert!(
            monitor_learned_windows(&source, &observations, &[])[0]
                .remaining_usd
                .is_none()
        );
    }
}

#[test]
fn observations_use_explicit_account_plan_and_accept_future_plan_names() {
    let mut source = quota(vec![window("week", 7)]);
    source.plan_type = Some("business".to_owned());
    for (plan, expected) in [
        (Some(" Future-Plan_2027 "), "future-plan_2027"),
        (Some("unknown"), "business"),
        (None, "business"),
    ] {
        let result = monitor_learning_observations("acct_m", "openai", plan, &source, now());
        assert_eq!(result[0].plan_type, expected);
        assert_eq!(result[0].provider_kind, "openai");
        assert!(result[0].window_key.starts_with("monitor-v2:"));
    }
    source.plan_type = Some("unknown".to_owned());
    assert!(monitor_learning_observations("acct_m", "openai", None, &source, now()).is_empty());
}

#[test]
fn malformed_or_expired_restrictive_windows_do_not_borrow_other_estimates() {
    let source = quota(vec![window("week", 7)]);
    let original = observations(&source);
    for mode in [
        "key",
        "group",
        "limit",
        "role",
        "duration",
        "expired",
        "old",
        "future",
        "no-observation",
        "missing-percent",
        "nan",
        "negative",
        "over-limit",
        "too-long",
        "non-minute",
        "missing-duration",
    ] {
        let mut changed = source.clone();
        match mode {
            "key" => changed.windows[0].key = "other".to_owned(),
            "group" => changed.windows[0].group = "other".to_owned(),
            "limit" => changed.windows[0].limit_id = Some("other".to_owned()),
            "role" => changed.windows[0].role = Some(ProviderQuotaWindowRole::Primary),
            "duration" => changed.windows[0].window_seconds = Some(6 * 86_400),
            "expired" => changed.windows[0].reset_at = Some(now()),
            "old" => changed.observed_at = Some(now() - Duration::days(8)),
            "future" => changed.observed_at = Some(now() + Duration::seconds(1)),
            "no-observation" => changed.observed_at = None,
            "missing-percent" => changed.windows[0].used_percent = None,
            "nan" => changed.windows[0].used_percent = Some(f64::NAN),
            "negative" => changed.windows[0].used_percent = Some(-1.0),
            "over-limit" => changed.windows[0].used_percent = Some(101.0),
            "too-long" => changed.windows[0].window_seconds = Some(33 * 86_400),
            "non-minute" => changed.windows[0].window_seconds = Some(604_801),
            "missing-duration" => changed.windows[0].window_seconds = None,
            _ => unreachable!(),
        }
        let learned = monitor_learned_windows(
            &changed,
            &observations(&changed),
            &[estimate(&original[0], 120.0)],
        );
        assert_eq!(learned.len(), 1, "{mode}");
        assert_eq!(learned[0].remaining_usd, None, "{mode}");
        assert_eq!(monitor_learned_remaining(&learned, now()).0, None, "{mode}");
        assert!(monitor_learned_remaining(&learned, now()).1, "{mode}");
    }
}

#[test]
fn subsecond_window_start_rounding_is_tolerated_but_older_data_is_rejected() {
    let mut source = quota(vec![window("week", 7)]);
    source.windows[0].reset_at = Some(now() + Duration::days(7));
    for (millis, valid) in [(999, true), (1001, false)] {
        source.observed_at = Some(now() - Duration::milliseconds(millis));
        assert_eq!(!observations(&source).is_empty(), valid);
    }
}

#[test]
fn real_account_windows_take_minimum_and_missing_one_never_becomes_unlimited() {
    let mut source = quota(vec![window("week", 7), window("short", 1)]);
    source.windows[1].window_seconds = Some(5 * 3600);
    source.windows[1].reset_at = Some(now() + Duration::hours(4));
    let observed = observations(&source);
    let estimates = [estimate(&observed[0], 120.0), estimate(&observed[1], 20.0)];
    let learned = monitor_learned_windows(&source, &observed, &estimates);
    assert_eq!(monitor_learned_remaining(&learned, now()).0, Some(16.0));
    assert!(!monitor_learned_remaining(&learned, now()).1);
    let incomplete = monitor_learned_windows(&source, &observed, &estimates[..1]);
    assert!(monitor_learned_remaining(&incomplete, now()).1);
    source.windows[1].local_usage_attribution = QuotaLocalUsageAttribution::Unavailable;
    let learned = monitor_learned_windows(&source, &observations(&source), &estimates);
    assert_eq!(learned.len(), 1);
    assert_eq!(monitor_learned_remaining(&learned, now()).0, Some(96.0));
}
