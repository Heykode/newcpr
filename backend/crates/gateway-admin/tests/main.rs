use std::str::FromStr as _;

use chrono::{TimeDelta, Utc};
use gateway_core::account::OpaqueProviderData;

use gateway_admin::{
    model::{
        AdminModelError, PageSize, Revision,
        accounts::{AccountUsage, CumulativeCostAmount},
        auth::LoginCommand,
        client_keys::ClientKeyPageSize,
        observability::{DecimalAmount, RequestOutcome, TimeRange},
        provider_credentials::{
            CredentialCommitGuard, ProviderDocument, ProviderQuota, ProviderQuotaWindow,
            QuotaLocalUsageAttribution,
        },
        settings::AdminApiKey,
    },
    ports::store::{AccountStore, AuthStore, ClientKeyStore, ObservabilityStore, SettingsStore},
};

mod backup;
mod model;
mod use_case;

#[test]
fn revision_should_reject_zero() {
    assert!(Revision::new(0).is_err());
}

#[test]
fn page_size_should_accept_frozen_upper_bound() {
    assert_eq!(PageSize::new(200).map(PageSize::get), Ok(200));
}

#[test]
fn page_size_should_reject_value_above_frozen_upper_bound() {
    assert!(PageSize::new(201).is_err());
}

#[test]
fn client_key_page_size_should_keep_the_full_nonzero_u16_contract() {
    assert!(ClientKeyPageSize::new(0).is_err());
    assert_eq!(
        ClientKeyPageSize::new(u16::MAX).map(ClientKeyPageSize::get),
        Ok(u16::MAX)
    );
}

#[test]
fn request_outcome_filter_should_preserve_known_and_bounded_other_values() {
    let known = RequestOutcome::new("succeeded").expect("known outcome");
    let other = RequestOutcome::new("provider_future_state").expect("other outcome");

    assert_eq!(known, RequestOutcome::Succeeded);
    assert!(matches!(&other, RequestOutcome::Other(_)));
    assert_eq!(other.as_str(), "provider_future_state");
    assert!(RequestOutcome::new("").is_err());
    assert!(RequestOutcome::new("a".repeat(RequestOutcome::MAX_BYTES + 1)).is_err());
    assert!(RequestOutcome::new("future\nstate").is_err());
}

#[test]
fn time_range_should_accept_exactly_366_days() {
    let end = Utc::now();
    assert!(TimeRange::new(end - TimeDelta::days(366), end).is_ok());
}

#[test]
fn time_range_should_reject_more_than_366_days() {
    let end = Utc::now();
    assert!(TimeRange::new(end - TimeDelta::days(366) - TimeDelta::seconds(1), end).is_err());
}

#[test]
fn decimal_amount_should_canonicalize_redundant_zeroes() {
    assert_eq!(
        DecimalAmount::from_str("00012.34000").map(|amount| amount.to_string()),
        Ok("12.34".to_owned())
    );
}

#[test]
fn cumulative_cost_amount_should_canonicalize_exactly() {
    for (input, expected) in [
        (" 00012.34000 ", "12.34"),
        ("000.0000000000", "0"),
        ("00000", "0"),
        ("0.0000000001", "0.0000000001"),
        ("10000000000.0000000001", "10000000000.0000000001"),
        ("00010000000000.1200000000", "10000000000.12"),
    ] {
        let amount: CumulativeCostAmount = input.parse().expect("valid cumulative cost");
        assert_eq!(amount.as_str(), expected);
        assert_eq!(amount.to_string(), expected);
        assert_eq!(
            amount.to_string().parse::<CumulativeCostAmount>(),
            Ok(amount)
        );
    }
}

#[test]
fn cumulative_cost_amount_should_support_numeric_40_10_without_widening_window_costs() {
    for input in [
        "10000000000",
        "10000000000.0000000001",
        "999999999999999999999999999999",
        "999999999999999999999999999999.9999999999",
    ] {
        let amount = input.parse::<CumulativeCostAmount>().expect("large total");
        assert_eq!(amount.as_str(), input);
        assert_eq!(amount.to_string(), input);
        assert_eq!(
            input.parse::<DecimalAmount>(),
            Err(AdminModelError::InvalidDecimalAmount)
        );
    }
    assert!("9999999999.9999999999".parse::<DecimalAmount>().is_ok());
}

#[test]
fn cumulative_cost_amount_should_reject_invalid_or_out_of_range_values() {
    for input in [
        "",
        " ",
        "-1",
        "-0",
        "+1",
        ".1",
        "1.",
        "1.2.3",
        "1e10",
        "NaN",
        "Infinity",
        "1 2",
        "1_000",
        "\u{0661}",
        "1000000000000000000000000000000",
        "1.00000000001",
        "1.00000000000",
    ] {
        assert_eq!(
            input.parse::<CumulativeCostAmount>(),
            Err(AdminModelError::InvalidCumulativeCostAmount),
            "{input:?}"
        );
    }
}

#[test]
fn admin_api_key_debug_should_redact_plaintext() {
    let key = AdminApiKey::new("secret-admin-key");
    assert!(!format!("{key:?}").contains("secret-admin-key"));
}

#[test]
fn login_command_debug_should_redact_password() {
    let command = LoginCommand {
        username: None,
        password: "secret-password".to_owned(),
    };
    assert!(!format!("{command:?}").contains("secret-password"));
}

#[test]
fn all_store_capability_traits_should_be_object_safe() {
    fn assert_object_safe<T: ?Sized + Send + Sync>() {}

    assert_object_safe::<dyn AccountStore>();
    assert_object_safe::<dyn AuthStore>();
    assert_object_safe::<dyn ClientKeyStore>();
    assert_object_safe::<dyn ObservabilityStore>();
    assert_object_safe::<dyn SettingsStore>();

    fn assert_send_object_safe<T: ?Sized + Send>() {}
    assert_send_object_safe::<dyn CredentialCommitGuard>();
}

#[test]
fn provider_document_debug_should_not_expose_opaque_material() {
    let document = ProviderDocument::new(OpaqueProviderData::new(Default::default()));
    assert_eq!(
        format!("{document:?}"),
        "ProviderDocument([PROVIDER_OWNED])"
    );
}

#[test]
fn representative_quota_should_prefer_short_window_and_highest_usage() {
    let quota = ProviderQuota {
        plan_type: None,
        observed_at: None,
        refresh_token_expires_at: None,
        windows: vec![
            quota_window("monthly", Some(2_592_000), Some(99.0)),
            quota_window("shortTerm", Some(604_800), Some(80.0)),
            quota_window("shortTerm", Some(18_000), Some(30.0)),
            quota_window("shortTerm", Some(14_400), Some(45.0)),
        ],
        limit_reached: false,
        provider_data: None,
    };

    assert_eq!(quota.representative_used_percent(), Some(45.0));
}

#[test]
fn representative_quota_should_prefer_account_wide_window_over_model_specific_window() {
    let mut model_specific = quota_window("shortTerm", Some(18_000), Some(64.0));
    model_specific.local_usage_attribution = QuotaLocalUsageAttribution::Unavailable;
    let quota = ProviderQuota {
        plan_type: None,
        observed_at: None,
        refresh_token_expires_at: None,
        windows: vec![
            model_specific,
            quota_window("shortTerm", Some(604_800), Some(5.0)),
        ],
        limit_reached: false,
        provider_data: None,
    };

    assert_eq!(quota.representative_used_percent(), Some(5.0));
}

#[test]
fn usage_window_should_not_use_daily_rolling_usage_for_weekly_statistics() {
    let usage = AccountUsage {
        account_id: "acct_xai".to_owned(),
        request_count: 5,
        success_count: 5,
        input_tokens: Some(100),
        output_tokens: Some(20),
        cached_tokens: Some(80),
        cache_write_tokens: Some(0),
        reasoning_tokens: Some(0),
        image_input_tokens: Some(0),
        image_output_tokens: Some(0),
        image_request_count: 0,
        image_request_failed_count: 0,
        total_tokens: Some(120),
        cost_coverage: Default::default(),
        costs: Vec::new(),
        last_used_at: Some(Utc::now()),
        request_buckets: Vec::new(),
        models: Vec::new(),
    };
    let quota = ProviderQuota {
        plan_type: None,
        observed_at: None,
        refresh_token_expires_at: None,
        windows: vec![ProviderQuotaWindow {
            key: "free-rolling-24h".to_owned(),
            group: "shortTerm".to_owned(),
            label: "日限额".to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
            window_seconds: Some(86_400),
            used_percent: None,
            reset_at: None,
            limit_reached: false,
            local_usage: Some(usage),
            provider_data: None,
        }],
        limit_reached: false,
        provider_data: None,
    };

    assert_eq!(quota.usage_window(), None);
}

#[test]
fn exhausted_quota_should_project_full_usage_to_only_the_representative_window() {
    let mut quota = ProviderQuota {
        plan_type: None,
        observed_at: None,
        refresh_token_expires_at: None,
        windows: vec![
            quota_window("monthly", Some(2_592_000), Some(99.0)),
            quota_window("shortTerm", Some(604_800), Some(80.0)),
            quota_window("shortTerm", Some(18_000), Some(30.0)),
            quota_window("shortTerm", Some(14_400), Some(95.0)),
        ],
        limit_reached: true,
        provider_data: None,
    };

    assert_eq!(quota.representative_used_percent(), Some(100.0));
    quota.apply_limit_reached_display();

    assert_eq!(quota.windows[0].used_percent, Some(99.0));
    assert_eq!(quota.windows[1].used_percent, Some(80.0));
    assert_eq!(quota.windows[2].used_percent, Some(30.0));
    assert_eq!(quota.windows[3].used_percent, Some(100.0));
    assert!(quota.windows[3].limit_reached);
}

#[test]
fn exhausted_quota_should_preserve_the_provider_identified_reached_window() {
    let mut reached = quota_window("monthly", Some(2_592_000), Some(98.0));
    reached.limit_reached = true;
    let mut quota = ProviderQuota {
        plan_type: None,
        observed_at: None,
        refresh_token_expires_at: None,
        windows: vec![reached, quota_window("shortTerm", Some(18_000), Some(95.0))],
        limit_reached: true,
        provider_data: None,
    };

    quota.apply_limit_reached_display();

    assert_eq!(quota.windows[0].used_percent, Some(100.0));
    assert_eq!(quota.windows[1].used_percent, Some(95.0));
    assert!(!quota.windows[1].limit_reached);
}

fn quota_window(
    group: &str,
    window_seconds: Option<u64>,
    used_percent: Option<f64>,
) -> ProviderQuotaWindow {
    ProviderQuotaWindow {
        key: group.to_owned(),
        group: group.to_owned(),
        label: group.to_owned(),
        limit_id: None,
        limit_name: None,
        role: None,
        local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
        window_seconds,
        used_percent,
        reset_at: None,
        limit_reached: false,
        local_usage: None,
        provider_data: None,
    }
}
