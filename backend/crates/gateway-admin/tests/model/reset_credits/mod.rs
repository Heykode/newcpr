use chrono::{Duration, Utc};
use gateway_admin::model::{
    provider_credentials::{ProviderResetCredit, ProviderResetCredits},
    reset_credits::earliest_credit,
};

fn card(id: &str, hours: i64, kind: &str) -> ProviderResetCredit {
    ProviderResetCredit {
        id: id.into(),
        status: Some("available".into()),
        title: None,
        expires_at: Some(Utc::now() + Duration::hours(hours)),
        reset_type: Some(kind.into()),
    }
}

#[test]
fn reset_credits_earliest_valid_expiry_wins_not_upstream_order() {
    let mut inventory = ProviderResetCredits {
        available_count: 4,
        credits: vec![
            card("later", 48, "codex"),
            card("expired", -1, "codex"),
            card("soon", 1, "codex"),
            card("tomorrow", 24, "codex"),
        ],
    };
    assert_eq!(
        earliest_credit(&inventory, None, Utc::now()).unwrap().id,
        "soon"
    );
    inventory.credits[2].status = Some("redeemed".into());
    assert_eq!(
        earliest_credit(&inventory, None, Utc::now()).unwrap().id,
        "tomorrow"
    );
    inventory.available_count = 0;
    assert!(earliest_credit(&inventory, None, Utc::now()).is_err());
}

#[test]
fn reset_credits_mixed_windows_require_explicit_type() {
    let inventory = ProviderResetCredits {
        available_count: 2,
        credits: vec![card("weekly", 1, "weekly"), card("short", 2, "short")],
    };
    assert!(earliest_credit(&inventory, None, Utc::now()).is_err());
    assert_eq!(
        earliest_credit(&inventory, Some("short"), Utc::now())
            .unwrap()
            .id,
        "short"
    );
    assert!(earliest_credit(&inventory, Some("missing"), Utc::now()).is_err());
}

#[test]
fn reset_credits_missing_expiry_duplicates_and_count_only_are_not_guessed() {
    let mut inventory = ProviderResetCredits {
        available_count: 2,
        credits: vec![card("one", 1, "codex")],
    };
    inventory.credits[0].expires_at = None;
    assert!(earliest_credit(&inventory, None, Utc::now()).is_err());
    inventory.credits = vec![card("one", 1, "codex"), card("one", 2, "codex")];
    assert!(earliest_credit(&inventory, None, Utc::now()).is_err());
    inventory.credits.clear();
    assert!(earliest_credit(&inventory, None, Utc::now()).is_err());
}

fn automatic_quota(
    now: chrono::DateTime<Utc>,
    short: f64,
    long: f64,
) -> gateway_admin::model::provider_credentials::ProviderQuota {
    use gateway_admin::model::provider_credentials::*;
    ProviderQuota {
        plan_type: None,
        observed_at: Some(now),
        refresh_token_expires_at: None,
        credits: None,
        limit_reached: false,
        provider_data: None,
        windows: [(18_000, short), (604_800, long)]
            .into_iter()
            .map(|(seconds, used)| ProviderQuotaWindow {
                key: seconds.to_string(),
                group: "codex".into(),
                label: "fixture".into(),
                limit_id: None,
                limit_name: None,
                role: None,
                local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
                window_seconds: Some(seconds),
                used_percent: Some(used),
                reset_at: Some(now + Duration::hours(1)),
                limit_reached: false,
                local_usage: None,
                provider_data: None,
            })
            .collect(),
    }
}

#[test]
fn automatic_thresholds_are_default_off_exact_and_either_window_can_trigger() {
    use gateway_admin::model::reset_credits::AutoResetConfig;
    let mut config = AutoResetConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.five_hour_used_millis, 100_000);
    for value in [0, 100, 12_345, 100_000] {
        config.five_hour_used_millis = value;
        assert!(config.validate().is_ok());
    }
    for value in [1, 99, 100_001, u32::MAX] {
        config.five_hour_used_millis = value;
        assert!(config.validate().is_err());
    }
    let config = AutoResetConfig {
        enabled: true,
        five_hour_used_millis: 12_345,
        seven_day_used_millis: 100_000,
    };
    let now = Utc::now();
    let inspect = |short, long| {
        config
            .observation(&automatic_quota(now, short, long), now, now)
            .unwrap()
    };
    assert!(inspect(12.344, 99.9).triggered.is_empty());
    assert_eq!(inspect(12.345, 1.0).triggered.len(), 1);
    assert_eq!(inspect(1.0, 100.0).triggered.len(), 1);
    assert_eq!(inspect(12.345, 100.0).triggered.len(), 2);
    let config = AutoResetConfig {
        enabled: true,
        five_hour_used_millis: 0,
        seven_day_used_millis: 0,
    };
    assert!(
        config
            .observation(&automatic_quota(now, 100.0, 100.0), now, now)
            .unwrap()
            .triggered
            .is_empty()
    );
}

#[test]
fn automatic_evidence_rejects_stale_partial_named_and_duplicate_windows() {
    use gateway_admin::model::{
        provider_credentials::QuotaLocalUsageAttribution, reset_credits::AutoResetConfig,
    };
    let config = AutoResetConfig {
        enabled: true,
        ..Default::default()
    };
    let now = Utc::now();
    let mut quota = automatic_quota(now, 100.0, 0.0);
    quota.observed_at = Some(now - Duration::seconds(1));
    assert!(config.observation(&quota, now, now).is_err());
    quota.observed_at = Some(now + Duration::seconds(1));
    assert!(config.observation(&quota, now, now).is_err());
    quota.observed_at = Some(now);
    quota.windows[0].local_usage_attribution = QuotaLocalUsageAttribution::Unavailable;
    let evidence = config.observation(&quota, now, now).unwrap();
    assert!(evidence.triggered.is_empty());
    assert!(
        !evidence.all_below,
        "missing window cannot rearm consumption"
    );
    quota.windows[0].local_usage_attribution = QuotaLocalUsageAttribution::AccountWide;
    quota.windows.push(quota.windows[0].clone());
    assert!(
        config
            .observation(&quota, now, now)
            .unwrap()
            .triggered
            .is_empty()
    );
    quota.windows.pop();
    for used in [f64::NAN, f64::INFINITY, -1.0] {
        quota.windows[0].used_percent = Some(used);
        assert!(
            config
                .observation(&quota, now, now)
                .unwrap()
                .triggered
                .is_empty()
        );
    }
    quota.windows[0].used_percent = Some(100.0);
    quota.windows[0].reset_at = None;
    assert!(
        config
            .observation(&quota, now, now)
            .unwrap()
            .triggered
            .is_empty()
    );
    quota.windows[0].reset_at = Some(now);
    assert!(
        config
            .observation(&quota, now, now)
            .unwrap()
            .triggered
            .is_empty()
    );
}
