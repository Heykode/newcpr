use std::collections::BTreeSet;

use chrono::{DateTime, Timelike as _, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{BarkChannelView, BarkLevel, GroupAlertPolicy};
use crate::model::AdminError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BarkSchedule {
    pub timezone: String,
    pub default_enabled: bool,
    pub rules: Vec<BarkScheduleRule>,
}

impl Default for BarkSchedule {
    fn default() -> Self {
        Self {
            timezone: "Asia/Shanghai".to_owned(),
            default_enabled: true,
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BarkScheduleRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub start_time: String,
    pub end_time: String,
    pub level: Option<BarkLevel>,
    pub sound: Option<String>,
    pub volume: Option<u8>,
    pub call: Option<bool>,
}

impl BarkSchedule {
    pub fn normalize(&mut self) -> Result<(), AdminError> {
        self.timezone = self.timezone.trim().to_owned();
        if self.timezone.parse::<chrono_tz::Tz>().is_err() {
            return Err(AdminError::invalid("Bark 时区不合法"));
        }
        if self.rules.len() > 24 {
            return Err(AdminError::invalid("Bark 时间段最多 24 条"));
        }
        let mut ids = BTreeSet::new();
        for rule in &mut self.rules {
            rule.id = rule.id.trim().to_owned();
            rule.name = rule.name.trim().to_owned();
            rule.start_time = rule.start_time.trim().to_owned();
            rule.end_time = rule.end_time.trim().to_owned();
            rule.sound = rule
                .sound
                .take()
                .map(|sound| sound.trim().to_owned())
                .filter(|sound| !sound.is_empty());
            if rule.id.is_empty()
                || rule.id.len() > 80
                || rule.id.eq_ignore_ascii_case("default")
                || !ids.insert(rule.id.to_ascii_lowercase())
                || rule.name.is_empty()
                || rule.name.chars().count() > 80
            {
                return Err(AdminError::invalid("Bark 时间段名称或标识不合法、重复"));
            }
            if clock_minutes(&rule.start_time).is_none()
                || clock_minutes(&rule.end_time).is_none()
                || rule.start_time == rule.end_time
            {
                return Err(AdminError::invalid(
                    "Bark 时间段需使用 HH:mm，开始和结束不能相同",
                ));
            }
            if rule.volume.is_some_and(|volume| volume > 10)
                || rule.sound.as_ref().is_some_and(|sound| sound.len() > 128)
            {
                return Err(AdminError::invalid("Bark 时间段音量或铃声不合法"));
            }
        }
        Ok(())
    }
}

fn clock_minutes(value: &str) -> Option<u32> {
    let bytes = value.as_bytes();
    if bytes.len() != 5
        || bytes[2] != b':'
        || ![bytes[0], bytes[1], bytes[3], bytes[4]]
            .iter()
            .all(u8::is_ascii_digit)
    {
        return None;
    }
    let hour = u32::from(bytes[0] - b'0') * 10 + u32::from(bytes[1] - b'0');
    let minute = u32::from(bytes[3] - b'0') * 10 + u32::from(bytes[4] - b'0');
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBarkRule {
    pub id: String,
    pub name: String,
    pub level: BarkLevel,
    pub sound: Option<String>,
    pub volume: u8,
    pub call: bool,
}

impl ResolvedBarkRule {
    #[must_use]
    pub fn fingerprint(&self) -> String {
        bark_delivery_key(
            &self.id,
            self.level,
            self.sound.as_deref(),
            self.volume,
            self.call,
        )
    }
}

#[must_use]
pub fn bark_delivery_key(
    id: &str,
    level: BarkLevel,
    sound: Option<&str>,
    volume: u8,
    call: bool,
) -> String {
    let encoded = serde_json::json!([id, level.as_str(), sound, volume, call]).to_string();
    hex::encode(Sha256::digest(encoded.as_bytes()))
}

impl BarkLevel {
    #[must_use]
    pub const fn delivery_strength(self, volume: u8, call: bool) -> u16 {
        let rank = match self {
            Self::Passive => 0,
            Self::Active => 1,
            Self::TimeSensitive => 2,
            Self::Critical => 3,
        };
        rank * 100 + volume as u16 + if call { 20 } else { 0 }
    }
}

impl GroupAlertPolicy {
    /// Resolves presentation only. Channel readiness and the group switch remain caller-owned.
    #[must_use]
    pub fn bark_rule_at(
        &self,
        channel: &BarkChannelView,
        now: DateTime<Utc>,
    ) -> Option<ResolvedBarkRule> {
        let zone = self.bark_schedule.timezone.parse::<chrono_tz::Tz>().ok()?;
        let local = now.with_timezone(&zone);
        let minute = local.hour() * 60 + local.minute();
        let selected = self.bark_schedule.rules.iter().find(|rule| {
            if !rule.enabled {
                return false;
            }
            let (Some(start), Some(end)) = (
                clock_minutes(&rule.start_time),
                clock_minutes(&rule.end_time),
            ) else {
                return false;
            };
            if start < end {
                start <= minute && minute < end
            } else if start > end {
                minute >= start || minute < end
            } else {
                false
            }
        });
        if selected.is_none() && !self.bark_schedule.default_enabled {
            return None;
        }
        Some(ResolvedBarkRule {
            id: selected.map_or_else(|| "default".to_owned(), |rule| rule.id.clone()),
            name: selected.map_or_else(|| "默认提醒".to_owned(), |rule| rule.name.clone()),
            level: selected
                .and_then(|rule| rule.level)
                .or(self.bark_level)
                .unwrap_or(channel.level),
            sound: selected
                .and_then(|rule| rule.sound.clone())
                .or_else(|| self.bark_sound.clone())
                .or_else(|| channel.sound.clone()),
            volume: selected
                .and_then(|rule| rule.volume)
                .or(self.bark_volume)
                .unwrap_or(channel.volume),
            call: selected
                .and_then(|rule| rule.call)
                .or(self.bark_call)
                .unwrap_or(channel.call),
        })
    }
}
