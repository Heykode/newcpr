use super::*;
use crate::model::provider_credentials::{ProviderQuota, QuotaLocalUsageAttribution};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutoResetConfig {
    pub enabled: bool,
    pub five_hour_used_millis: u32,
    pub seven_day_used_millis: u32,
    #[serde(default)]
    pub expiry_enabled: bool,
    #[serde(default = "default_expiry_lead_minutes")]
    pub expiry_lead_minutes: u32,
    #[serde(default)]
    pub max_weekly_remaining_percent: Option<u8>,
}

fn default_expiry_lead_minutes() -> u32 {
    10
}

impl Default for AutoResetConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            five_hour_used_millis: 100_000,
            seven_day_used_millis: 100_000,
            expiry_enabled: false,
            expiry_lead_minutes: default_expiry_lead_minutes(),
            max_weekly_remaining_percent: None,
        }
    }
}

impl AutoResetConfig {
    pub fn validate(&self) -> Result<(), AdminError> {
        if !(1..=1440).contains(&self.expiry_lead_minutes)
            || self.max_weekly_remaining_percent.is_some_and(|n| n > 100)
        {
            return Err(AdminError::invalid(
                "到期提前量须为 1 至 1440 分钟，周剩余额度须为 0 至 100%",
            ));
        }
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
            expiry_triggered: false,
            inventory: None,
            weekly_condition_met: self.max_weekly_remaining_percent.is_none_or(|limit| {
                let windows: Vec<_> = quota
                    .windows
                    .iter()
                    .filter(|window| {
                        window.window_seconds == Some(604_800)
                            && window.local_usage_attribution
                                == QuotaLocalUsageAttribution::AccountWide
                    })
                    .collect();
                windows.len() == 1
                    && windows[0].used_percent.is_some_and(|used| {
                        used.is_finite()
                            && (0.0..=100.0).contains(&used)
                            && 100.0 - used <= f64::from(limit)
                            && windows[0].reset_at.is_some_and(|reset| reset > now)
                    })
            }),
        })
    }

    pub fn expiry_due(
        &self,
        credit: &super::super::provider_credentials::ProviderResetCredit,
        now: DateTime<Utc>,
    ) -> bool {
        self.expiry_enabled
            && credit.expires_at.is_some_and(|at| {
                at > now
                    && at <= now + chrono::Duration::minutes(i64::from(self.expiry_lead_minutes))
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResetInventory {
    pub checked_at: DateTime<Utc>,
    pub credit: Option<super::super::provider_credentials::ProviderResetCredit>,
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
    pub inventory: Option<AutoResetInventory>,
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
    #[serde(default)]
    pub expiry_triggered: bool,
    #[serde(default)]
    pub inventory: Option<AutoResetInventory>,
    #[serde(default = "default_weekly_condition")]
    pub weekly_condition_met: bool,
}

fn default_weekly_condition() -> bool {
    true
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
