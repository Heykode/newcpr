//! Fixed cleanup categories; account and billing data are never cleanup targets.
use super::AdminError;
use crate::backup::policy::BackupSchedule;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupCategory {
    Requests,
    Files,
    Captures,
    Audit,
}
impl CleanupCategory {
    pub const ALL: [Self; 4] = [Self::Requests, Self::Files, Self::Captures, Self::Audit];
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanupSelection {
    pub selected: bool,
    pub retention_days: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupFrequency {
    Hourly,
    SixHourly,
    Daily,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanupConfig {
    pub enabled: bool,
    pub frequency: CleanupFrequency,
    pub daily_hour: u8,
    pub daily_minute: u8,
    pub timezone: String,
    pub requests: CleanupSelection,
    pub files: CleanupSelection,
    pub captures: CleanupSelection,
    pub audit: CleanupSelection,
}
impl CleanupConfig {
    pub fn selection(&self, category: CleanupCategory) -> &CleanupSelection {
        match category {
            CleanupCategory::Requests => &self.requests,
            CleanupCategory::Files => &self.files,
            CleanupCategory::Captures => &self.captures,
            CleanupCategory::Audit => &self.audit,
        }
    }
    pub fn validate(&self) -> Result<(), AdminError> {
        if self.daily_hour > 23 || self.daily_minute > 59 {
            return Err(AdminError::invalid("清理时间不合法"));
        }
        for category in CleanupCategory::ALL {
            let days = self.selection(category).retention_days;
            let minimum = if category == CleanupCategory::Requests {
                31
            } else {
                1
            };
            let maximum = if category == CleanupCategory::Captures {
                30
            } else {
                3650
            };
            if !(minimum..=maximum).contains(&days) {
                return Err(AdminError::invalid(
                    "保留天数超出范围：请求日志31至3650天，采集1至30天，其他1至3650天",
                ));
            }
        }
        if self.enabled
            && !CleanupCategory::ALL
                .iter()
                .any(|c| self.selection(*c).selected)
        {
            return Err(AdminError::invalid("自动清理至少选择一个项目"));
        }
        self.schedule()?;
        Ok(())
    }
    fn schedule(&self) -> Result<BackupSchedule, AdminError> {
        let cron = match self.frequency {
            CleanupFrequency::Hourly => "0 * * * *".to_owned(),
            CleanupFrequency::SixHourly => "0 */6 * * *".to_owned(),
            CleanupFrequency::Daily => format!("{} {} * * *", self.daily_minute, self.daily_hour),
        };
        BackupSchedule::parse(&cron, &self.timezone)
            .map_err(|_| AdminError::invalid("清理时区或时间不合法"))
    }
    pub fn next_after(&self, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, AdminError> {
        if self.enabled {
            Ok(self.schedule()?.next_after(now))
        } else {
            Ok(None)
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupJobStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupJob {
    pub id: String,
    pub instance_id: String,
    pub automatic: bool,
    pub config: CleanupConfig,
    pub cutoff_at: DateTime<Utc>,
    pub status: CleanupJobStatus,
    pub category_index: usize,
    pub removed: u64,
    pub errors: Vec<CleanupCategory>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupState {
    pub revision: i64,
    pub config: CleanupConfig,
    pub next_run_at: Option<DateTime<Utc>>,
    pub job: Option<CleanupJob>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanupCommand {
    pub revision: i64,
    pub config: CleanupConfig,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanupPreview {
    pub revision: i64,
    pub config: CleanupConfig,
    pub cutoff_at: DateTime<Utc>,
}
impl CleanupPreview {
    pub fn validate(&self) -> Result<(), AdminError> {
        self.config.validate()?;
        let now = Utc::now();
        if self.cutoff_at > now || self.cutoff_at < now - Duration::minutes(10) {
            return Err(AdminError::invalid("清理确认已过期，请重新预览"));
        }
        if !CleanupCategory::ALL
            .iter()
            .any(|c| self.config.selection(*c).selected)
        {
            return Err(AdminError::invalid("请选择清理项目"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupUsage {
    pub category: CleanupCategory,
    pub bytes: Option<u64>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupFootprint {
    pub measured_at: DateTime<Utc>,
    pub items: Vec<CleanupUsage>,
}
#[derive(Debug, Clone, Copy)]
pub struct CleanupBatch {
    pub removed: u64,
    pub complete: bool,
}
