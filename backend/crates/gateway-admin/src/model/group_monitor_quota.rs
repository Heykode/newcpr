//! Sample-local peer estimates, never persisted as account or Plan capacity.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};

use super::{
    group_monitor::MonitorQuotaPeer,
    provider_credentials::{
        AccountUsagePeriod, ProviderQuota, ProviderQuotaWindow, explicit_plan_type,
    },
    quota_forecast::{CurrentQuotaEstimate, current_window_estimate},
};

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct PeerKey {
    provider: String,
    plan: String,
    window: String,
    group: String,
    limit_id: Option<String>,
    role: Option<&'static str>,
    seconds: u64,
}

fn peer_key(peer: &MonitorQuotaPeer, window: &ProviderQuotaWindow) -> Option<PeerKey> {
    Some(PeerKey {
        provider: peer.provider.clone(),
        plan: explicit_plan_type(peer.plan.as_deref())?
            .trim()
            .to_ascii_lowercase(),
        window: window.key.clone(),
        group: window.group.clone(),
        limit_id: window.limit_id.clone(),
        role: window.role.map(|role| role.as_str()),
        seconds: window.window_seconds?,
    })
}

fn live_window(quota: &ProviderQuota, now: DateTime<Utc>) -> Option<&ProviderQuotaWindow> {
    let (window, period) = quota.usage_window()?;
    if period != AccountUsagePeriod::Weekly {
        return None;
    }
    let reset = window.reset_at?;
    let seconds = i64::try_from(window.window_seconds?).ok()?;
    let start = reset.checked_sub_signed(Duration::try_seconds(seconds)?)?;
    let observed = quota.observed_at?;
    (seconds > 0
        && start <= observed
        && observed <= now
        && now < reset
        && window
            .used_percent
            .is_some_and(|percent| percent.is_finite() && (0.0..=100.0).contains(&percent)))
    .then_some(window)
}

/// Prefer own current-window estimate. Only genuine, complete estimates feed peers.
#[must_use]
pub fn monitor_quota_estimates(
    peers: &[MonitorQuotaPeer],
    quotas: &BTreeMap<String, ProviderQuota>,
    now: DateTime<Utc>,
) -> BTreeMap<String, CurrentQuotaEstimate> {
    let mut result = BTreeMap::new();
    let mut sources: BTreeMap<PeerKey, Vec<(&MonitorQuotaPeer, f64)>> = BTreeMap::new();
    for peer in peers {
        let Some(quota) = quotas.get(&peer.id) else {
            continue;
        };
        let Some((window, AccountUsagePeriod::Weekly)) = quota.usage_window() else {
            continue;
        };
        if let Some(estimate) = current_window_estimate(window, now) {
            result.insert(peer.id.clone(), estimate);
            if !estimate.incomplete_cost
                && window
                    .local_usage
                    .as_ref()
                    .is_some_and(|usage| usage.cost_coverage.partial_count == 0)
                && live_window(quota, now).is_some()
                && let Some(key) = peer_key(peer, window)
            {
                sources
                    .entry(key)
                    .or_default()
                    .push((peer, estimate.total_usd));
            }
        }
    }
    let averages = sources
        .into_iter()
        .map(|(key, mut values)| {
            values.sort_by(|(a, _), (b, _)| {
                b.created_at
                    .cmp(&a.created_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
            values.dedup_by(|(a, _), (b, _)| a.id == b.id);
            values.truncate(3);
            let count = values.len() as f64;
            (
                key,
                values.iter().map(|(_, total)| total / count).sum::<f64>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for peer in peers {
        if result.contains_key(&peer.id) {
            continue;
        }
        let Some(window) = quotas
            .get(&peer.id)
            .and_then(|quota| live_window(quota, now))
        else {
            continue;
        };
        // Missing billed costs are not the same as a newly imported zero-use account.
        let Some(usage) = window.local_usage.as_ref() else {
            continue;
        };
        if usage.cost_coverage.unavailable_count > 0
            || usage.cost_coverage.partial_count > 0
            || (usage.request_count > 0
                && !usage
                    .costs
                    .iter()
                    .any(|cost| cost.currency.eq_ignore_ascii_case("USD")))
            || usage.costs.iter().any(|cost| {
                cost.currency.eq_ignore_ascii_case("USD")
                    && cost.amount.to_string().parse::<f64>().ok() != Some(0.0)
            })
        {
            continue;
        }
        let Some(total) = peer_key(peer, window)
            .and_then(|key| averages.get(&key))
            .copied()
        else {
            continue;
        };
        let Some(percent) = window.used_percent else {
            continue;
        };
        let remaining = total * (1.0 - percent / 100.0);
        if total.is_finite() && remaining.is_finite() {
            result.insert(
                peer.id.clone(),
                CurrentQuotaEstimate {
                    total_usd: total,
                    remaining_usd: remaining.max(0.0),
                    incomplete_cost: false,
                },
            );
        }
    }
    result
}
