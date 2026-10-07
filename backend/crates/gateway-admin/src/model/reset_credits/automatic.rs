use super::*;
use crate::model::provider_credentials::{ProviderQuota, QuotaLocalUsageAttribution};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutoResetConfig {
    pub enabled: bool,
    pub five_hour_used_millis: u32,
    pub seven_day_used_millis: u32,
}

impl Default for AutoResetConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            five_hour_used_millis: 100_000,
            seven_day_used_millis: 100_000,
        }
    }
}

impl AutoResetConfig {
    pub fn validate(&self) -> Result<(), AdminError> {
        if [self.five_hour_used_millis, self.seven_day_used_millis]
            .into_iter()
            .any(|n| n != 0 && !(100..=100_000).contains(&n))
        {
            return Err(AdminError::invalid("自动重置阈值必须为 0 或 0.1% 至 100%"));
        }
        Ok(())
    }

    pub fn observation(
        &self,
        quota: &ProviderQuota,
        started_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<AutoResetObservation, AdminError> {
        self.validate()?;
        let observed_at = quota
            .observed_at
            .filter(|at| {
                *at >= started_at
                    && *at <= now
                    && now.signed_duration_since(*at).num_seconds() <= 90
            })
            .ok_or_else(|| AdminError::invalid("没有新鲜的额度观测，本次未消费"))?;
        let mut triggered = Vec::new();
        let mut all_below = true;
        for (seconds, threshold) in [
            (18_000, self.five_hour_used_millis),
            (604_800, self.seven_day_used_millis),
        ] {
            if threshold == 0 {
                continue;
            }
            let windows: Vec<_> = quota
                .windows
                .iter()
                .filter(|window| {
                    window.window_seconds == Some(seconds)
                        && window.local_usage_attribution == QuotaLocalUsageAttribution::AccountWide
                })
                .collect();
            if windows.len() != 1 {
                all_below = false;
                continue;
            }
            let window = windows[0];
            let Some((used, reset_at)) = window
                .used_percent
                .zip(window.reset_at)
                .filter(|(used, reset)| used.is_finite() && *used >= 0.0 && *reset > now)
            else {
                all_below = false;
                continue;
            };
            if used >= f64::from(threshold) / 1000.0 {
                all_below = false;
                triggered.push(AutoResetWindow { seconds, reset_at });
            }
        }
        Ok(AutoResetObservation {
            observed_at,
            started_at,
            triggered,
            all_below,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResetPolicy {
    pub account_id: String,
    pub revision: i64,
    pub config: AutoResetConfig,
    pub checked_at: Option<DateTime<Utc>>,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AutoResetCheck {
    pub policy: AutoResetPolicy,
    pub claim_id: Uuid,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResetWindow {
    pub seconds: u64,
    pub reset_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResetObservation {
    pub started_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub triggered: Vec<AutoResetWindow>,
    pub all_below: bool,
}

impl AutoResetObservation {
    pub fn overlaps(&self, previous: &[AutoResetWindow]) -> bool {
        self.triggered.iter().any(|current| {
            previous.iter().any(|old| {
                current.seconds == old.seconds
                    && (current.reset_at - old.reset_at).abs() <= chrono::Duration::seconds(2)
            })
        })
    }
}
