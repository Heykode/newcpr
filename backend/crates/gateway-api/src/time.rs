use chrono::{DateTime, Utc};
use gateway_core::time::DeploymentTimeZone;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelativeTimeView {
    JustNow,
    Elapsed { value: i64, unit: RelativeTimeUnit },
    Absolute { value: String },
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelativeTimeUnit {
    Minute,
    Hour,
    Day,
}

impl RelativeTimeView {
    pub(crate) fn legacy_display(self) -> String {
        match self {
            Self::JustNow => "刚刚".to_owned(),
            Self::Absolute { value } => value,
            Self::Elapsed { value, unit } => format!(
                "{value} {}前",
                match unit {
                    RelativeTimeUnit::Minute => "分钟",
                    RelativeTimeUnit::Hour => "小时",
                    RelativeTimeUnit::Day => "天",
                }
            ),
        }
    }
}

pub(crate) fn relative_view(
    value: DateTime<Utc>,
    now: DateTime<Utc>,
    timezone: DeploymentTimeZone,
) -> RelativeTimeView {
    let elapsed = now.signed_duration_since(value);
    if elapsed.num_seconds() < 0 {
        RelativeTimeView::Absolute {
            value: timezone
                .local(value)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
        }
    } else if elapsed.num_seconds() < 60 {
        RelativeTimeView::JustNow
    } else if elapsed.num_minutes() < 60 {
        RelativeTimeView::Elapsed {
            value: elapsed.num_minutes(),
            unit: RelativeTimeUnit::Minute,
        }
    } else if elapsed.num_hours() < 24 {
        RelativeTimeView::Elapsed {
            value: elapsed.num_hours(),
            unit: RelativeTimeUnit::Hour,
        }
    } else {
        RelativeTimeView::Elapsed {
            value: elapsed.num_days(),
            unit: RelativeTimeUnit::Day,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times_use_server_now_and_preserve_future_absolute_time() {
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let timezone = DeploymentTimeZone::default();
        for (seconds, expected) in [
            (0, "刚刚"),
            (59, "刚刚"),
            (60, "1 分钟前"),
            (3600, "1 小时前"),
            (86400, "1 天前"),
        ] {
            assert_eq!(
                relative_view(now - chrono::Duration::seconds(seconds), now, timezone)
                    .legacy_display(),
                expected
            );
        }
        assert!(matches!(
            relative_view(now + chrono::Duration::seconds(1), now, timezone),
            RelativeTimeView::Absolute { .. }
        ));
    }
}
