use std::collections::BTreeMap;

use chrono::Duration;
use gateway_admin::model::{
    group_monitor::MonitorQuotaPeer, group_monitor_quota::monitor_quota_estimates,
    provider_credentials::ProviderQuota,
};

use super::quota_forecast::{now, quota, window};

fn peer(id: &str, age: i64) -> MonitorQuotaPeer {
    MonitorQuotaPeer {
        id: id.to_owned(),
        provider: "openai".to_owned(),
        plan: Some("Plus".to_owned()),
        created_at: now() - Duration::hours(age),
    }
}

fn source(cost: &str, percent: f64) -> ProviderQuota {
    let mut w = window("week", 7);
    w.used_percent = Some(percent);
    w.local_usage.as_mut().unwrap().costs[0].amount = cost.parse().unwrap();
    quota(vec![w])
}

#[test]
fn latest_three_live_peers_recalculate_without_recursively_learning_fallbacks() {
    let mut peers = vec![peer("new", 0), peer("other-new", 0)];
    let mut quotas = BTreeMap::from([
        ("new".to_owned(), source("0", 20.0)),
        ("other-new".to_owned(), source("0", 0.0)),
    ]);
    assert!(monitor_quota_estimates(&peers, &quotas, now()).is_empty());
    for (id, age, cost, mean) in [
        ("a", 4, "10", 100.0),
        ("b", 3, "20", 150.0),
        ("c", 2, "30", 200.0),
        ("d", 1, "40", 300.0),
    ] {
        peers.push(peer(id, age));
        quotas.insert(id.to_owned(), source(cost, 10.0));
        let estimates = monitor_quota_estimates(&peers, &quotas, now());
        assert!((estimates["new"].total_usd - mean).abs() < 1e-8);
        assert!((estimates["new"].remaining_usd - mean * 0.8).abs() < 1e-8);
        assert!((estimates["other-new"].remaining_usd - mean).abs() < 1e-8);
    }
    quotas.insert("new".to_owned(), source("1", 1.0));
    let own = monitor_quota_estimates(&peers, &quotas, now());
    assert_eq!(
        (own["new"].total_usd, own["new"].remaining_usd),
        (100.0, 99.0)
    );
    quotas.insert("new".to_owned(), source("2", 1.0));
    assert_eq!(
        monitor_quota_estimates(&peers, &quotas, now())["new"].total_usd,
        200.0
    );
    quotas.retain(|id, _| id == "other-new");
    assert!(monitor_quota_estimates(&peers, &quotas, now()).is_empty());
}

#[test]
fn subsecond_reset_rounding_is_live_but_older_observations_stay_unknown() {
    let peers = vec![peer("new", 0), peer("source", 1)];
    let reset = now() + Duration::days(7);
    let mut recipient = source("0", 0.0);
    recipient.windows[0].reset_at = Some(reset);
    recipient.observed_at = Some(now() - Duration::milliseconds(999));
    let mut donor = source("10", 10.0);
    donor.windows[0].reset_at = Some(reset);
    let mut quotas = BTreeMap::from([("new".to_owned(), recipient), ("source".to_owned(), donor)]);

    assert_eq!(
        monitor_quota_estimates(&peers, &quotas, now())["new"].remaining_usd,
        100.0
    );

    quotas.get_mut("new").unwrap().observed_at = Some(now() - Duration::milliseconds(1_001));
    assert!(!monitor_quota_estimates(&peers, &quotas, now()).contains_key("new"));

    quotas.get_mut("new").unwrap().observed_at = Some(now() + Duration::milliseconds(1));
    assert!(!monitor_quota_estimates(&peers, &quotas, now()).contains_key("new"));
}

#[test]
fn incompatible_or_incomplete_sources_never_supply_a_fallback() {
    for mode in [
        "provider",
        "plan",
        "unknown-plan",
        "key",
        "duration",
        "limit",
        "role",
        "partial",
        "missing-cost",
        "expired",
        "old-observation",
        "no-observation",
    ] {
        let mut peers = vec![peer("new", 0), peer("source", 1)];
        let mut donor = source("10", 10.0);
        match mode {
            "provider" => peers[1].provider = "other".to_owned(),
            "plan" => peers[1].plan = Some("team".to_owned()),
            "unknown-plan" => peers[1].plan = Some("unknown".to_owned()),
            "key" => donor.windows[0].key = "other".to_owned(),
            "duration" => donor.windows[0].window_seconds = Some(604_801),
            "limit" => donor.windows[0].limit_id = Some("model-only".to_owned()),
            "role" => {
                donor.windows[0].role = Some(
                    gateway_admin::model::provider_credentials::ProviderQuotaWindowRole::Primary,
                )
            }
            "partial" => {
                donor.windows[0]
                    .local_usage
                    .as_mut()
                    .unwrap()
                    .cost_coverage
                    .partial_count = 1
            }
            "missing-cost" => {
                donor.windows[0]
                    .local_usage
                    .as_mut()
                    .unwrap()
                    .cost_coverage
                    .unavailable_count = 1
            }
            "expired" => donor.windows[0].reset_at = Some(now()),
            "old-observation" => donor.observed_at = Some(now() - Duration::days(8)),
            "no-observation" => donor.observed_at = None,
            _ => unreachable!(),
        }
        let quotas = BTreeMap::from([
            ("new".to_owned(), source("0", 0.0)),
            ("source".to_owned(), donor),
        ]);
        assert!(
            !monitor_quota_estimates(&peers, &quotas, now()).contains_key("new"),
            "{mode}"
        );
    }
}

#[test]
fn missing_recipient_evidence_stays_unknown_and_new_plan_names_work() {
    let mut peers = vec![peer("new", 0), peer("source", 1)];
    peers[0].plan = Some(" New-Plan ".to_owned());
    peers[1].plan = Some("new-plan".to_owned());
    for mode in [
        "valid",
        "zero-without-requests",
        "percent",
        "expired",
        "missing-cost",
        "currency",
        "partial",
    ] {
        let mut recipient = source("0", 0.0);
        match mode {
            "percent" => recipient.windows[0].used_percent = None,
            "expired" => recipient.windows[0].reset_at = Some(now()),
            "missing-cost" => {
                recipient.windows[0]
                    .local_usage
                    .as_mut()
                    .unwrap()
                    .cost_coverage
                    .unavailable_count = 1
            }
            "partial" => {
                recipient.windows[0]
                    .local_usage
                    .as_mut()
                    .unwrap()
                    .cost_coverage
                    .partial_count = 1
            }
            "currency" => {
                recipient.windows[0].local_usage.as_mut().unwrap().costs[0].currency =
                    "EUR".to_owned()
            }
            "zero-without-requests" => {
                let usage = recipient.windows[0].local_usage.as_mut().unwrap();
                usage.request_count = 0;
                usage.costs.clear();
            }
            _ => {}
        }
        let quotas = BTreeMap::from([
            ("new".to_owned(), recipient),
            ("source".to_owned(), source("10", 10.0)),
        ]);
        assert_eq!(
            monitor_quota_estimates(&peers, &quotas, now()).contains_key("new"),
            matches!(mode, "valid" | "zero-without-requests"),
            "{mode}"
        );
    }
}
