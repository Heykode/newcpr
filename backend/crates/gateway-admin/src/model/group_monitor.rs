//! Read-only estimates. None of these projections are scheduling or billing facts.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::{
    account_groups::{AccountGroupMemberFact, AccountGroupRef},
    quota_forecast::AccountQuotaForecastReport,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonitorUsage {
    pub usd: f64,
    pub missing_costs: u64,
}

#[derive(Debug, Clone)]
pub struct MonitorMember {
    pub member: AccountGroupMemberFact,
    pub provider: String,
    pub plan: Option<String>,
    pub first_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct MonitorLifespan {
    pub provider: String,
    pub plan: String,
    pub minutes: f64,
    pub samples: u64,
}

#[derive(Debug, Clone, Default)]
pub struct GroupMonitorFacts {
    pub config_revision: u64,
    pub groups: Vec<AccountGroupRef>,
    pub members: Vec<MonitorMember>,
    pub group_usage: BTreeMap<String, MonitorUsage>,
    pub account_usage: BTreeMap<String, MonitorUsage>,
    pub lifespans: Vec<MonitorLifespan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupMonitorReport {
    pub generated_at: DateTime<Utc>,
    pub items: Vec<GroupMonitorItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupMonitorItem {
    pub group: AccountGroupRef,
    pub total_accounts: u64,
    pub eligible_accounts: u64,
    pub estimated_accounts: u64,
    pub used_slots: Option<u64>,
    pub total_slots: u64,
    pub remaining_usd: Option<f64>,
    pub remaining_status: &'static str,
    pub expected_expiry_usd: Option<f64>,
    pub expiry_status: &'static str,
    pub consume_usd_per_minute: Option<f64>,
    pub quota_consume_usd_per_minute: Option<f64>,
    pub eta_minutes: Option<f64>,
    pub eta_status: &'static str,
    pub low_sample: bool,
    pub earliest_reset_at: Option<DateTime<Utc>>,
}

/// Account-local input after runtime availability and forecasts are resolved.
#[derive(Debug, Clone)]
pub struct MonitorAccountEstimate {
    pub id: String,
    pub eligible: bool,
    pub used_slots: Option<u64>,
    pub total_slots: u64,
    pub remaining_usd: Option<f64>,
    pub incomplete: bool,
    pub unavailable: bool,
    pub low_sample: bool,
    pub reset_at: Option<DateTime<Utc>>,
    pub consumption: MonitorUsage,
    pub remaining_life_minutes: Option<f64>,
}

/// Weekly/monthly projections can share one source; never add those balances.
#[must_use]
pub fn monitor_remaining(
    report: &AccountQuotaForecastReport,
    now: DateTime<Utc>,
) -> (Option<f64>, bool, bool, Option<DateTime<Utc>>) {
    if !report.learned_windows.is_empty() {
        let mut remaining: Option<f64> = None;
        let mut reset: Option<DateTime<Utc>> = None;
        let mut partial = false;
        let mut low_sample = false;
        for window in &report.learned_windows {
            low_sample |= window.low_sample;
            let valid = window
                .remaining_usd
                .filter(|amount| amount.is_finite() && *amount >= 0.0)
                .zip(window.reset_at.filter(|reset_at| *reset_at > now));
            if let Some((amount, reset_at)) = valid {
                remaining = Some(remaining.map_or(amount, |value| value.min(amount)));
                reset = Some(reset.map_or(reset_at, |value| value.min(reset_at)));
            } else {
                partial = true;
            }
        }
        return (remaining, partial, low_sample, reset);
    }
    let is_valid = |forecast: &&super::quota_forecast::AccountQuotaForecast| {
        forecast.unavailable_reason.is_none()
            && forecast
                .source
                .as_ref()
                .is_some_and(|source| source.reset_at > now)
            && forecast
                .remaining_usd
                .is_some_and(|amount| amount.is_finite() && amount >= 0.0)
    };
    let valid = report.forecasts.iter().filter(is_valid);
    let mut remaining: Option<f64> = None;
    let mut reset: Option<DateTime<Utc>> = None;
    let mut partial = report.forecasts.iter().any(|forecast| !is_valid(&forecast));
    let mut low_sample = false;
    for forecast in valid {
        if let Some(amount) = forecast.remaining_usd {
            remaining = Some(remaining.map_or(amount, |current| current.min(amount)));
        }
        if let Some(source) = &forecast.source {
            reset = Some(reset.map_or(source.reset_at, |current| current.min(source.reset_at)));
        }
        partial |= forecast.incomplete_cost;
        low_sample |= forecast.low_sample;
    }
    (remaining, partial, low_sample, reset)
}

#[must_use]
pub fn project_group_monitor(
    group: AccountGroupRef,
    accounts: &[MonitorAccountEstimate],
    group_usage: &MonitorUsage,
) -> GroupMonitorItem {
    let unique = accounts
        .iter()
        .map(|account| (&account.id, account))
        .collect::<BTreeMap<_, _>>();
    let eligible = unique
        .values()
        .copied()
        .filter(|account| group.enabled && account.eligible)
        .collect::<Vec<_>>();
    let mut item = GroupMonitorItem {
        group,
        total_accounts: unique.len() as u64,
        eligible_accounts: eligible.len() as u64,
        estimated_accounts: 0,
        used_slots: Some(0),
        total_slots: 0,
        remaining_usd: None,
        remaining_status: "learning",
        expected_expiry_usd: None,
        expiry_status: "learning",
        consume_usd_per_minute: known_usage(group_usage),
        quota_consume_usd_per_minute: Some(0.0),
        eta_minutes: None,
        eta_status: "learning",
        low_sample: false,
        earliest_reset_at: None,
    };
    let mut remaining = 0.0;
    let mut expiry = Some(0.0);
    let mut partial = false;
    let mut unavailable = false;
    for account in eligible {
        item.total_slots = item.total_slots.saturating_add(account.total_slots);
        item.used_slots = item
            .used_slots
            .zip(account.used_slots)
            .map(|(left, right)| left.saturating_add(right));
        item.quota_consume_usd_per_minute = item
            .quota_consume_usd_per_minute
            .zip(known_usage(&account.consumption))
            .map(|(left, right)| left + right);
        item.low_sample |= account.low_sample;
        partial |= account.incomplete;
        unavailable |= account.unavailable;
        if let Some(reset) = account.reset_at {
            item.earliest_reset_at = Some(
                item.earliest_reset_at
                    .map_or(reset, |current| current.min(reset)),
            );
        }
        if let Some(amount) = account
            .remaining_usd
            .filter(|amount| amount.is_finite() && *amount >= 0.0)
        {
            item.estimated_accounts += 1;
            remaining += amount;
            expiry = expiry
                .zip(
                    account
                        .remaining_life_minutes
                        .filter(|minutes| minutes.is_finite() && *minutes > 0.0),
                )
                .zip(known_usage(&account.consumption))
                .map(|((total, minutes), rate)| total + (amount - rate * minutes).max(0.0));
        } else {
            expiry = None;
        }
    }
    if !item.group.enabled {
        item.remaining_status = "disabled";
        item.expiry_status = "disabled";
        item.eta_status = "disabled";
        return item;
    }
    let complete = item.estimated_accounts == item.eligible_accounts
        && !partial
        && !unavailable
        && remaining.is_finite();
    if item.estimated_accounts > 0 || item.eligible_accounts == 0 {
        item.remaining_usd = remaining.is_finite().then_some(remaining);
        item.remaining_status = if complete { "ready" } else { "partial" };
    }
    if complete {
        item.expected_expiry_usd = expiry
            .filter(|value| value.is_finite())
            .map(|value| value.min(remaining));
        item.expiry_status = if item.expected_expiry_usd.is_some() {
            "ready"
        } else {
            "learning"
        };
        if remaining == 0.0 {
            item.eta_minutes = Some(0.0);
            item.eta_status = "empty";
        } else if let Some(rate) = item
            .quota_consume_usd_per_minute
            .filter(|rate| rate.is_finite())
        {
            if rate > 0.0 {
                item.eta_minutes = Some(remaining / rate).filter(|value| value.is_finite());
                item.eta_status = if item.eta_minutes.is_some() {
                    "ready"
                } else {
                    "unknown"
                };
            } else {
                item.eta_status = "idle";
            }
        } else {
            item.eta_status = "unknown";
        }
    } else if item.remaining_usd.is_some() {
        item.eta_status = "partial";
        item.expiry_status = "partial";
    } else if unavailable {
        item.remaining_status = "unknown";
        item.expiry_status = "unknown";
        item.eta_status = "unknown";
    }
    item.quota_consume_usd_per_minute = item
        .quota_consume_usd_per_minute
        .filter(|value| value.is_finite());
    item
}

fn known_usage(usage: &MonitorUsage) -> Option<f64> {
    (usage.missing_costs == 0 && usage.usd.is_finite() && usage.usd >= 0.0).then_some(usage.usd)
}
