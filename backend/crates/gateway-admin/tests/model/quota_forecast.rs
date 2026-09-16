use chrono::{DateTime, Duration, Utc};
use gateway_admin::model::{
    accounts::{AccountCost, AccountUsage},
    observability::CostCoverage,
    provider_credentials::{ProviderQuota, ProviderQuotaWindow, QuotaLocalUsageAttribution},
    quota_forecast::{AccountQuotaForecast, account_quota_forecasts, current_window_estimate},
    quota_forecast_sampling::{
        QuotaForecastMethod, QuotaForecastPoint, QuotaForecastSample, QuotaForecastUsage,
        select_forecast_sample,
    },
};

fn now() -> DateTime<Utc> {
    "2026-09-12T00:00:00Z".parse().unwrap()
}

fn window(key: &str, days: u64) -> ProviderQuotaWindow {
    ProviderQuotaWindow {
        key: key.to_owned(),
        group: if days > 7 { "monthly" } else { "shortTerm" }.to_owned(),
        label: key.to_owned(),
        limit_id: None,
        limit_name: None,
        role: None,
        local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
        window_seconds: Some(days * 86_400),
        used_percent: Some(20.0),
        reset_at: Some(now() + Duration::days(1)),
        limit_reached: false,
        local_usage: Some(AccountUsage {
            account_id: "acct_forecast".to_owned(),
            request_count: 10,
            success_count: 10,
            input_tokens: Some(900),
            output_tokens: Some(100),
            cached_tokens: Some(800),
            cache_write_tokens: None,
            reasoning_tokens: None,
            image_input_tokens: None,
            image_output_tokens: None,
            image_request_count: 0,
            image_request_failed_count: 0,
            total_tokens: Some(1_000),
            cost_coverage: CostCoverage {
                calculated_count: 10,
                ..Default::default()
            },
            costs: vec![AccountCost {
                currency: "USD".to_owned(),
                amount: "2".parse().unwrap(),
            }],
            last_used_at: Some(now()),
            request_buckets: Vec::new(),
            models: Vec::new(),
        }),
        provider_data: None,
    }
}

fn quota(windows: Vec<ProviderQuotaWindow>) -> ProviderQuota {
    ProviderQuota {
        plan_type: None,
        observed_at: Some(now()),
        refresh_token_expires_at: None,
        windows,
        limit_reached: false,
        provider_data: None,
    }
}

#[test]
fn current_window_estimates_recompute_below_five_percent_without_learning() {
    let mut source = window("week", 7);
    source.used_percent = Some(1.0);
    let first = current_window_estimate(&source, now()).unwrap();
    assert_eq!((first.total_usd, first.remaining_usd), (200.0, 198.0));
    source.local_usage.as_mut().unwrap().costs[0].amount = "3".parse().unwrap();
    assert_eq!(
        current_window_estimate(&source, now()).unwrap().total_usd,
        300.0
    );
    source.used_percent = Some(2.0);
    assert_eq!(
        current_window_estimate(&source, now()).unwrap().total_usd,
        150.0
    );
    source
        .local_usage
        .as_mut()
        .unwrap()
        .cost_coverage
        .unavailable_count = 1;
    assert!(
        current_window_estimate(&source, now())
            .unwrap()
            .incomplete_cost
    );
    source.used_percent = Some(100.0);
    assert_eq!(
        current_window_estimate(&source, now())
            .unwrap()
            .remaining_usd,
        0.0
    );
}

#[test]
fn current_window_estimates_reject_unknown_or_mismatched_facts() {
    let source = window("week", 7);
    for percent in [
        None,
        Some(0.0),
        Some(-1.0),
        Some(f64::NAN),
        Some(f64::INFINITY),
    ] {
        let mut changed = source.clone();
        changed.used_percent = percent;
        assert!(current_window_estimate(&changed, now()).is_none());
    }
    for mode in [
        "missing-cost",
        "zero",
        "currency",
        "missing-usage",
        "expired",
        "future",
        "model",
    ] {
        let mut changed = source.clone();
        match mode {
            "missing-cost" => changed.local_usage.as_mut().unwrap().costs.clear(),
            "zero" => changed.local_usage.as_mut().unwrap().costs[0].amount = "0".parse().unwrap(),
            "currency" => {
                changed.local_usage.as_mut().unwrap().costs[0].currency = "CNY".to_owned()
            }
            "missing-usage" => changed.local_usage = None,
            "expired" => changed.reset_at = Some(now()),
            "future" => changed.reset_at = Some(now() + Duration::days(8)),
            "model" => changed.local_usage_attribution = QuotaLocalUsageAttribution::Unavailable,
            _ => unreachable!(),
        }
        assert!(current_window_estimate(&changed, now()).is_none(), "{mode}");
    }
}

#[test]
fn weekly_usd_forecast_pairs_ten_dollars_with_twenty_percentage_points() {
    let mut full = quota(vec![window("week", 7)]);
    full.windows[0].local_usage.as_mut().unwrap().costs[0].amount = "10".parse().unwrap();
    assert_eq!(forecast(&full)[0].estimated_usd, Some(50.0));

    // Imported at 40%: prior consumption is not part of the locally observed $10.
    let imported = now() - Duration::hours(2);
    let observed = imported + Duration::minutes(1);
    let baseline_usage = QuotaForecastUsage {
        request_count: 1,
        known_cost_count: 1,
        usd: 2.0,
        ..Default::default()
    };
    let current_usage = QuotaForecastUsage {
        request_count: 11,
        known_cost_count: 11,
        usd: 12.0,
        ..Default::default()
    };
    full.windows[0].used_percent = Some(60.0);
    let paired = select_forecast_sample(
        "week".to_owned(),
        imported,
        QuotaForecastPoint {
            observed_at: now(),
            used_percent: 60.0,
            usage: current_usage,
        },
        vec![QuotaForecastPoint {
            observed_at: observed,
            used_percent: 40.0,
            usage: baseline_usage,
        }],
        0,
    );
    assert_eq!(paired.method, QuotaForecastMethod::Incremental);
    assert_eq!(paired.sampled_percent, 20.0);
    assert_eq!(paired.usage.usd, 10.0);
    let result = account_quota_forecasts(&full, imported, now(), &[paired]);
    assert_eq!(result[0].estimated_usd, Some(50.0));
    assert_eq!(result[0].remaining_usd, Some(20.0));
}

fn forecast(quota: &ProviderQuota) -> [AccountQuotaForecast; 2] {
    account_quota_forecasts(quota, now() - Duration::days(60), now(), &samples(quota))
}

fn samples(quota: &ProviderQuota) -> Vec<QuotaForecastSample> {
    quota
        .windows
        .iter()
        .filter_map(|window| {
            let usage = window.local_usage.as_ref()?;
            let usd = usage.costs.iter().find(|cost| cost.currency == "USD");
            Some(QuotaForecastSample {
                key: window.key.clone(),
                method: QuotaForecastMethod::Cumulative,
                start_at: window.reset_at?.checked_sub_signed(Duration::try_seconds(
                    i64::try_from(window.window_seconds?).ok()?,
                )?)?,
                end_at: quota.observed_at.unwrap_or(now()),
                baseline_percent: 0.0,
                sampled_percent: window.used_percent.unwrap_or(0.0),
                block_count: 0,
                observation_count: 0,
                pending_request_count: 0,
                discontinuous: false,
                usage: QuotaForecastUsage {
                    request_count: usage.request_count,
                    tokens: usage.total_tokens.unwrap_or(0),
                    input_tokens: usage.input_tokens.unwrap_or(0),
                    output_tokens: usage.output_tokens.unwrap_or(0),
                    cached_tokens: usage.cached_tokens.unwrap_or(0),
                    missing_token_count: u64::from(usage.total_tokens.is_none()),
                    known_cost_count: if usd.is_some() {
                        usage.cost_coverage.known_count()
                    } else {
                        0
                    },
                    unavailable_cost_count: usage.cost_coverage.partial_count
                        + usage.cost_coverage.unavailable_count,
                    usd: usd.map_or(0.0, |cost| cost.amount.as_str().parse().unwrap()),
                    excluded_request_count: 0,
                },
            })
        })
        .collect()
}

#[test]
fn forecasts_use_actual_periods_and_do_not_extrapolate_remaining_capacity() {
    let [week, month] = forecast(&quota(vec![window("week", 7)]));
    assert_eq!(week.estimated_tokens, Some(5_000));
    assert_eq!(week.estimated_usd, Some(10.0));
    assert_eq!(week.remaining_tokens, Some(4_000));
    assert_eq!(week.remaining_usd, Some(8.0));
    assert!(!week.extrapolated);
    assert!(month.extrapolated);
    assert_eq!(month.estimated_tokens, Some(21_429));
    assert!((month.estimated_usd.unwrap() - 10.0 * 30.0 / 7.0).abs() < 1e-10);
    assert_eq!(month.remaining_tokens, week.remaining_tokens);
    assert_eq!(month.remaining_usd, week.remaining_usd);
}

#[test]
fn corresponding_actual_window_takes_priority_and_retains_its_duration() {
    let [week, month] = forecast(&quota(vec![window("month", 31), window("week", 7)]));
    assert_eq!(week.source.unwrap().label, "week");
    assert_eq!(month.source.unwrap().label, "month");
    assert!(!month.extrapolated);
    assert_eq!(month.target_seconds, 31 * 86_400);
    assert_eq!(month.estimated_tokens, Some(5_000));
    let [week, _] = forecast(&quota(vec![window("month", 31)]));
    assert!(week.extrapolated);
    assert_eq!(week.estimated_tokens, Some(1_129));
}

#[test]
fn short_or_model_specific_windows_are_not_account_capacity() {
    let mut model = window("model", 7);
    model.local_usage_attribution = QuotaLocalUsageAttribution::Unavailable;
    for item in forecast(&quota(vec![window("day", 1), model])) {
        assert!(item.source.is_none());
        assert!(item.unavailable_reason.is_some());
        assert!(item.estimated_tokens.is_none());
    }
}

#[test]
fn invalid_or_tiny_percentages_cannot_produce_estimates() {
    for percent in [
        None,
        Some(f64::NAN),
        Some(f64::INFINITY),
        Some(-1.0),
        Some(101.0),
        Some(0.0),
        Some(0.5),
        Some(1.0),
        Some(4.9),
    ] {
        let mut sample = window("week", 7);
        sample.used_percent = percent;
        let [week, _] = forecast(&quota(vec![sample]));
        assert!(week.unavailable_reason.is_some(), "{percent:?}");
        assert!(week.estimated_tokens.is_none());
        assert!(week.estimated_usd.is_none());
    }
}

#[test]
fn low_samples_warn_and_exhaustion_uses_raw_not_display_percentages() {
    let mut sample = window("week", 7);
    sample.used_percent = Some(5.0);
    sample.limit_reached = true;
    let mut source = quota(vec![sample]);
    source.limit_reached = true;
    let [week, _] = forecast(&source);
    assert!(week.low_sample);
    assert_eq!(week.estimated_tokens, Some(20_000));
    source.windows[0].used_percent = Some(100.0);
    let [week, _] = forecast(&source);
    assert!(!week.low_sample);
    assert_eq!(week.remaining_tokens, Some(0));
    assert_eq!(week.remaining_usd, Some(0.0));
}

#[test]
fn stale_future_or_missing_snapshots_do_not_predict() {
    for observed in [
        None,
        Some(now() - Duration::days(8)),
        Some(now() + Duration::seconds(1)),
    ] {
        let mut source = quota(vec![window("week", 7)]);
        source.observed_at = observed;
        assert!(forecast(&source)[0].unavailable_reason.is_some());
    }
    let mut sample = window("week", 7);
    sample.reset_at = Some(now());
    assert!(
        forecast(&quota(vec![sample]))[0]
            .unavailable_reason
            .is_some()
    );
}

#[test]
fn partial_account_history_or_missing_usage_keep_diagnostics_without_estimates() {
    let source = quota(vec![window("week", 7)]);
    let [week, _] =
        account_quota_forecasts(&source, now() - Duration::days(1), now(), &samples(&source));
    assert!(week.unavailable_reason.unwrap().contains("记录不完整"));
    assert!(week.estimated_tokens.is_none());
    let mut sample = window("week", 7);
    sample.local_usage = None;
    let [week, _] = forecast(&quota(vec![sample]));
    assert!(week.source.is_some());
    assert!(
        week.unavailable_reason
            .unwrap()
            .contains("没有网关用量记录")
    );
}

#[test]
fn incremental_sample_supports_mid_cycle_accounts_and_remaining_uses_current_percent() {
    let source = quota(vec![window("week", 7)]);
    let mut sample = samples(&source).remove(0);
    sample.method = QuotaForecastMethod::Incremental;
    sample.start_at = now() - Duration::hours(6);
    sample.baseline_percent = 10.0;
    sample.sampled_percent = 10.0;
    sample.block_count = 2;
    let [week, month] =
        account_quota_forecasts(&source, now() - Duration::days(1), now(), &[sample]);
    assert_eq!(week.estimated_tokens, Some(10_000));
    assert_eq!(week.remaining_tokens, Some(8_000));
    assert_eq!(month.remaining_tokens, Some(8_000));
    assert!(week.unavailable_reason.is_none());
    assert!(!week.low_sample);
}

#[test]
fn missing_tokens_keep_estimates_from_recorded_usage() {
    let source = quota(vec![window("week", 7)]);
    let mut sample = samples(&source).remove(0);
    sample.usage.missing_token_count = 1;
    let [week, _] = account_quota_forecasts(&source, now() - Duration::days(60), now(), &[sample]);
    assert!(week.incomplete_tokens);
    assert!(week.unavailable_reason.is_none());
    assert_eq!(week.estimated_tokens, Some(5_000));
    assert_eq!(week.remaining_tokens, Some(4_000));
    assert_eq!(week.estimated_usd, Some(10.0));
    assert_eq!(week.source.unwrap().tokens, Some(1_000));
}

#[test]
fn discontinuous_samples_never_fall_back_to_cumulative_predictions() {
    let source = quota(vec![window("week", 7)]);
    let mut sample = samples(&source).remove(0);
    sample.discontinuous = true;
    let [week, _] = account_quota_forecasts(&source, now() - Duration::days(60), now(), &[sample]);
    assert!(week.unavailable_reason.unwrap().contains("不连续"));
    assert!(week.estimated_tokens.is_none());
}

#[test]
fn partial_costs_keep_estimates_from_known_amounts() {
    for coverage in [
        CostCoverage {
            calculated_count: 9,
            partial_count: 1,
            ..Default::default()
        },
        CostCoverage {
            calculated_count: 9,
            unavailable_count: 1,
            ..Default::default()
        },
    ] {
        let mut sample = window("week", 7);
        sample.local_usage.as_mut().unwrap().cost_coverage = coverage;
        let [week, _] = forecast(&quota(vec![sample]));
        assert!(week.incomplete_cost);
        assert!(week.unavailable_reason.is_none());
        assert_eq!(week.estimated_usd, Some(10.0));
        assert_eq!(week.remaining_usd, Some(8.0));
        assert_eq!(week.estimated_tokens, Some(5_000));
    }
}

#[test]
fn missing_tokens_and_costs_do_not_block_recorded_capacity_or_remaining_estimates() {
    let source = quota(vec![window("week", 7)]);
    let mut sample = samples(&source).remove(0);
    // 完整请求之外混有缺少计量的成功请求，不把已有用量和费用一并作废。
    sample.usage.request_count += 2;
    sample.usage.missing_token_count = 2;
    sample.usage.unavailable_cost_count = 2;
    sample.usage.excluded_request_count = 1;
    sample.pending_request_count = 1;
    let [week, month] =
        account_quota_forecasts(&source, now() - Duration::days(60), now(), &[sample]);
    assert!(week.incomplete_tokens);
    assert!(week.incomplete_cost);
    assert!(week.unavailable_reason.is_none());
    assert_eq!(week.estimated_tokens, Some(5_000));
    assert_eq!(week.estimated_usd, Some(10.0));
    assert_eq!(week.remaining_tokens, Some(4_000));
    assert_eq!(week.remaining_usd, Some(8.0));
    assert_eq!(month.estimated_tokens, Some(21_429));
    assert_eq!(month.remaining_tokens, week.remaining_tokens);
    assert_eq!(month.remaining_usd, week.remaining_usd);
}

#[test]
fn entirely_unknown_costs_leave_only_money_estimates_unavailable() {
    for coverage in [
        CostCoverage {
            unavailable_count: 10,
            ..Default::default()
        },
        CostCoverage::default(),
    ] {
        let mut sample = window("week", 7);
        let usage = sample.local_usage.as_mut().unwrap();
        usage.cost_coverage = coverage;
        usage.costs.clear();
        let [week, _] = forecast(&quota(vec![sample]));
        assert!(week.incomplete_cost);
        assert!(week.unavailable_reason.is_none());
        assert_eq!(week.estimated_tokens, Some(5_000));
        assert_eq!(week.estimated_usd, None);
        assert_eq!(week.remaining_usd, None);
        assert_eq!(week.source.unwrap().usd, None);
    }
}

#[test]
fn money_without_known_cost_evidence_is_not_estimated() {
    let source = quota(vec![window("week", 7)]);
    let mut sample = samples(&source).remove(0);
    sample.usage.known_cost_count = 0;
    let [week, _] = account_quota_forecasts(&source, now() - Duration::days(60), now(), &[sample]);
    assert!(week.incomplete_cost);
    assert!(week.unavailable_reason.is_none());
    assert_eq!(week.estimated_tokens, Some(5_000));
    assert_eq!(week.estimated_usd, None);
    assert_eq!(week.remaining_usd, None);
    assert_eq!(week.source.unwrap().usd, None);
}

#[test]
fn entirely_unknown_tokens_leave_known_money_estimates_available() {
    let source = quota(vec![window("week", 7)]);
    let mut sample = samples(&source).remove(0);
    sample.usage.tokens = 0;
    sample.usage.missing_token_count = sample.usage.request_count;
    let [week, _] = account_quota_forecasts(&source, now() - Duration::days(60), now(), &[sample]);
    assert!(week.incomplete_tokens);
    assert!(!week.incomplete_cost);
    assert!(week.unavailable_reason.is_none());
    assert_eq!(week.estimated_tokens, None);
    assert_eq!(week.remaining_tokens, None);
    assert_eq!(week.estimated_usd, Some(10.0));
    assert_eq!(week.remaining_usd, Some(8.0));
    assert_eq!(week.source.unwrap().tokens, None);
}

#[test]
fn entirely_unknown_tokens_and_costs_do_not_invent_zero_estimates() {
    let source = quota(vec![window("week", 7)]);
    let mut sample = samples(&source).remove(0);
    sample.usage = QuotaForecastUsage {
        request_count: 2,
        missing_token_count: 2,
        unavailable_cost_count: 2,
        ..Default::default()
    };
    let [week, month] =
        account_quota_forecasts(&source, now() - Duration::days(60), now(), &[sample]);
    for forecast in [week, month] {
        assert!(forecast.incomplete_tokens);
        assert!(forecast.incomplete_cost);
        assert!(forecast.unavailable_reason.is_some());
        assert_eq!(forecast.estimated_tokens, None);
        assert_eq!(forecast.estimated_usd, None);
        assert_eq!(forecast.remaining_tokens, None);
        assert_eq!(forecast.remaining_usd, None);
        let source = forecast.source.unwrap();
        assert_eq!(source.tokens, None);
        assert_eq!(source.usd, None);
    }
}

#[test]
fn known_zero_and_partial_positive_token_totals_remain_visible() {
    let source = quota(vec![window("week", 7)]);
    for missing in [0, 1] {
        let mut sample = samples(&source).remove(0);
        sample.usage.tokens = 0;
        sample.usage.missing_token_count = missing;
        let [week, _] =
            account_quota_forecasts(&source, now() - Duration::days(60), now(), &[sample]);
        assert_eq!(week.incomplete_tokens, missing > 0);
        assert_eq!(week.source.unwrap().tokens, Some(0));
    }

    let mut sample = samples(&source).remove(0);
    sample.usage.tokens = 17;
    sample.usage.missing_token_count = sample.usage.request_count;
    let [week, _] = account_quota_forecasts(&source, now() - Duration::days(60), now(), &[sample]);
    assert!(week.incomplete_tokens);
    assert_eq!(week.source.unwrap().tokens, Some(17));
    assert_eq!(week.estimated_tokens, Some(85));
}

#[test]
fn partial_zero_known_cost_remains_zero_without_imputing_missing_usage() {
    let source = quota(vec![window("week", 7)]);
    let mut sample = samples(&source).remove(0);
    sample.usage.usd = 0.0;
    sample.usage.known_cost_count = 9;
    sample.usage.unavailable_cost_count = 1;
    let [week, _] = account_quota_forecasts(&source, now() - Duration::days(60), now(), &[sample]);
    assert!(week.incomplete_cost);
    assert!(week.unavailable_reason.is_none());
    assert_eq!(week.estimated_usd, Some(0.0));
    assert_eq!(week.remaining_usd, Some(0.0));
    assert_eq!(week.estimated_tokens, Some(5_000));
}

#[test]
fn zero_known_cost_is_valid_but_missing_usd_is_not_invented() {
    let mut sample = window("week", 7);
    sample.local_usage.as_mut().unwrap().costs[0].amount = "0".parse().unwrap();
    assert_eq!(
        forecast(&quota(vec![sample.clone()]))[0].estimated_usd,
        Some(0.0)
    );
    sample.local_usage.as_mut().unwrap().costs[0].currency = "EUR".to_owned();
    assert_eq!(forecast(&quota(vec![sample]))[0].estimated_usd, None);
}

#[test]
fn oversized_window_or_estimate_is_unavailable_without_panics_or_saturation() {
    let mut sample = window("month", 30);
    sample.window_seconds = Some(i64::MAX as u64);
    assert!(
        forecast(&quota(vec![sample]))[1]
            .unavailable_reason
            .is_some()
    );
    let mut sample = window("week", 7);
    sample.local_usage.as_mut().unwrap().total_tokens = Some(u64::MAX);
    let [week, _] = forecast(&quota(vec![sample]));
    assert!(week.estimated_tokens.is_none());
    assert_eq!(week.estimated_usd, Some(10.0));
}
