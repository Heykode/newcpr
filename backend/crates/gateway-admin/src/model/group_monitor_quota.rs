//! Persistent, observation-aligned capacity used only by the group monitor.

use chrono::{DateTime, Duration, Utc};

use super::{
    provider_credentials::{
        ProviderQuota, ProviderQuotaWindow, QuotaLocalUsageAttribution, explicit_plan_type,
    },
    quota_learning::{
        LearnedQuotaWindow, MONITOR_LEARNING_PREFIX, QuotaLearningEstimate,
        QuotaLearningObservation,
    },
};

fn learning_key(window: &ProviderQuotaWindow) -> String {
    format!(
        "{MONITOR_LEARNING_PREFIX}{}",
        serde_json::json!([
            window.key,
            window.group,
            window.limit_id,
            window.role.map(|role| role.as_str()),
        ])
    )
}

#[must_use]
pub fn monitor_learning_observations(
    account_id: &str,
    provider_kind: &str,
    account_plan: Option<&str>,
    quota: &ProviderQuota,
    now: DateTime<Utc>,
) -> Vec<QuotaLearningObservation> {
    let Some(plan) =
        explicit_plan_type(account_plan).or_else(|| explicit_plan_type(quota.plan_type.as_deref()))
    else {
        return Vec::new();
    };
    let Some(observed_at) = quota.observed_at.filter(|observed| *observed <= now) else {
        return Vec::new();
    };
    quota
        .windows
        .iter()
        .filter(|window| window.local_usage_attribution == QuotaLocalUsageAttribution::AccountWide)
        .filter_map(|window| {
            let seconds = window
                .window_seconds
                .filter(|seconds| *seconds > 0 && *seconds <= 32 * 86_400 && seconds % 60 == 0)?;
            let reset_at = window.reset_at.filter(|reset| *reset > now)?;
            let start = reset_at.checked_sub_signed(Duration::seconds(seconds as i64))?;
            let percent = window
                .used_percent
                .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))?;
            // Reset times have whole-second precision; tolerate only that rounding gap.
            if observed_at < start - Duration::seconds(1) || observed_at >= reset_at {
                return None;
            }
            Some(QuotaLearningObservation {
                account_id: account_id.to_owned(),
                provider_kind: provider_kind.to_owned(),
                plan_type: plan.trim().to_ascii_lowercase(),
                window_key: learning_key(window),
                window_minutes: (seconds / 60) as i32,
                used_percent: percent,
                reset_at,
                observed_at,
                observed_cost_usd: None,
            })
        })
        .collect()
}

#[must_use]
pub fn monitor_learned_windows(
    quota: &ProviderQuota,
    observations: &[QuotaLearningObservation],
    estimates: &[QuotaLearningEstimate],
) -> Vec<LearnedQuotaWindow> {
    quota
        .windows
        .iter()
        .filter(|window| window.local_usage_attribution == QuotaLocalUsageAttribution::AccountWide)
        .map(|window| {
            let key = learning_key(window);
            let observation = observations.iter().find(|observation| {
                observation.window_key == key
                    && window.window_seconds == Some(observation.window_minutes as u64 * 60)
            });
            let estimate = observation.and_then(|observation| {
                estimates.iter().find(|estimate| {
                    estimate.account_id == observation.account_id
                        && estimate.window_key == observation.window_key
                        && estimate.window_minutes == observation.window_minutes
                })
            });
            let remaining = observation
                .zip(estimate)
                .and_then(|(observation, estimate)| {
                    let limit = estimate
                        .effective_limit_usd
                        .filter(|value| value.is_finite() && *value > 0.0)?;
                    let amount = limit * (1.0 - observation.used_percent / 100.0);
                    amount.is_finite().then_some(amount.max(0.0))
                });
            LearnedQuotaWindow {
                remaining_usd: remaining,
                reset_at: window.reset_at,
                low_sample: estimate.is_some_and(|estimate| estimate.sample_count < 3),
            }
        })
        .collect()
}
