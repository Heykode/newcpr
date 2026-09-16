use chrono::{Duration, Utc};
use gateway_admin::model::{
    account_groups::{AccountGroupColor, AccountGroupRef},
    group_monitor::{
        MonitorAccountEstimate, MonitorUsage, monitor_remaining, project_group_monitor,
    },
    provider_credentials::AccountUsagePeriod,
    quota_forecast::{AccountQuotaForecast, AccountQuotaForecastReport, QuotaForecastSource},
};
use gateway_core::routing::AccountGroupId;

fn group() -> AccountGroupRef {
    AccountGroupRef {
        id: AccountGroupId::new("grp_00000000000000000000000000000001").expect("group"),
        name: "Monitor".to_owned(),
        color: AccountGroupColor::parse("#2563EBFF").expect("color"),
        enabled: true,
    }
}

fn account(id: &str) -> MonitorAccountEstimate {
    MonitorAccountEstimate {
        id: id.to_owned(),
        eligible: true,
        used_slots: Some(2),
        total_slots: 4,
        remaining_usd: Some(100.0),
        incomplete: false,
        unavailable: false,
        low_sample: false,
        reset_at: None,
        consumption: MonitorUsage {
            usd: 2.0,
            missing_costs: 0,
        },
        remaining_life_minutes: Some(10.0),
    }
}

#[test]
fn monitor_deduplicates_accounts_and_uses_shared_rate_without_subtracting_expiry() {
    let a = account("a");
    let mut b = account("b");
    b.eligible = false;
    let item = project_group_monitor(
        group(),
        &[a.clone(), a, b],
        &MonitorUsage {
            usd: 1.0,
            missing_costs: 0,
        },
    );
    assert_eq!(
        (
            item.total_accounts,
            item.eligible_accounts,
            item.estimated_accounts
        ),
        (2, 1, 1)
    );
    assert_eq!((item.used_slots, item.total_slots), (Some(2), 4));
    assert_eq!(item.remaining_usd, Some(100.0));
    assert_eq!(item.expected_expiry_usd, Some(80.0));
    assert_eq!(item.consume_usd_per_minute, Some(1.0));
    assert_eq!(item.quota_consume_usd_per_minute, Some(2.0));
    assert_eq!(item.eta_minutes, Some(50.0));
}

#[test]
fn monitor_distinguishes_unknown_partial_learning_idle_and_disabled() {
    let mut a = account("a");
    a.remaining_usd = None;
    assert_eq!(
        project_group_monitor(group(), &[a.clone()], &MonitorUsage::default()).remaining_status,
        "learning"
    );
    a.unavailable = true;
    assert_eq!(
        project_group_monitor(group(), &[a.clone()], &MonitorUsage::default()).remaining_status,
        "unknown"
    );
    let partial = project_group_monitor(group(), &[a, account("b")], &MonitorUsage::default());
    assert_eq!(partial.remaining_status, "partial");
    assert_eq!(partial.eta_minutes, None);
    assert_eq!(partial.expected_expiry_usd, None);
    let mut a = account("a");
    a.consumption.usd = 0.0;
    let idle = project_group_monitor(group(), &[a.clone()], &MonitorUsage::default());
    assert_eq!(idle.eta_status, "idle");
    assert_eq!(idle.eta_minutes, None);
    a.remaining_life_minutes = Some(-1.0);
    assert_eq!(
        project_group_monitor(group(), &[a.clone()], &MonitorUsage::default()).expected_expiry_usd,
        None
    );
    let mut disabled = group();
    disabled.enabled = false;
    assert_eq!(
        project_group_monitor(disabled, &[a], &MonitorUsage::default()).remaining_status,
        "disabled"
    );
    let empty = project_group_monitor(group(), &[], &MonitorUsage::default());
    assert_eq!(empty.remaining_usd, Some(0.0));
    assert_eq!(empty.eta_status, "empty");
}

#[test]
fn missing_cost_or_runtime_is_never_reported_as_zero_and_nonfinite_is_rejected() {
    let mut a = account("a");
    a.used_slots = None;
    a.consumption.missing_costs = 1;
    let item = project_group_monitor(
        group(),
        &[a.clone()],
        &MonitorUsage {
            usd: 1.0,
            missing_costs: 1,
        },
    );
    assert_eq!(item.used_slots, None);
    assert_eq!(item.consume_usd_per_minute, None);
    assert_eq!(item.quota_consume_usd_per_minute, None);
    assert_eq!(item.eta_status, "unknown");
    a.remaining_usd = Some(f64::INFINITY);
    assert_eq!(
        project_group_monitor(group(), &[a], &MonitorUsage::default()).remaining_usd,
        None
    );
}

#[test]
fn monitor_forecast_never_adds_extrapolations_or_hides_invalid_source_windows() {
    let now = Utc::now();
    let forecast = AccountQuotaForecast {
        period: AccountUsagePeriod::Weekly,
        target_seconds: 604800,
        extrapolated: false,
        source: Some(QuotaForecastSource {
            label: "week".to_owned(),
            used_percent: Some(50.0),
            observed_at: Some(now),
            reset_at: now + Duration::hours(1),
            tokens: None,
            usd: Some(100.0),
        }),
        unavailable_reason: None,
        low_sample: false,
        incomplete_cost: false,
        incomplete_tokens: false,
        estimated_tokens: None,
        estimated_usd: Some(200.0),
        remaining_tokens: None,
        remaining_usd: Some(100.0),
    };
    let mut report = AccountQuotaForecastReport {
        learned_windows: Vec::new(),
        account_id: "a".to_owned(),
        generated_at: now,
        forecasts: [forecast.clone(), forecast],
    };
    report.forecasts[1].extrapolated = true;
    assert_eq!(monitor_remaining(&report, now).0, Some(100.0));
    report.forecasts[1].remaining_usd = Some(80.0);
    assert_eq!(monitor_remaining(&report, now).0, Some(80.0));
    report.forecasts[1].unavailable_reason = Some("insufficient samples");
    assert!(monitor_remaining(&report, now).1);
    report.forecasts[0]
        .source
        .as_mut()
        .expect("source")
        .reset_at = now;
    assert_eq!(monitor_remaining(&report, now).0, None);
}
