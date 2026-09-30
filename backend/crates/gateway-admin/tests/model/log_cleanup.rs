use chrono::{TimeZone, Utc};
use gateway_admin::model::log_cleanup::*;

fn config() -> CleanupConfig {
    serde_json::from_value(serde_json::json!({
        "enabled":true,"frequency":"daily","dailyHour":3,"dailyMinute":0,"timezone":"Asia/Shanghai",
        "requests":{"selected":true,"retentionDays":31},"files":{"selected":true,"retentionDays":7},
        "captures":{"selected":false,"retentionDays":7},"audit":{"selected":false,"retentionDays":90}
    })).unwrap()
}
#[test]
fn log_cleanup_validates_windows_and_calendar_schedule() {
    let mut config = config();
    config.validate().unwrap();
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 1, 0).unwrap();
    assert_eq!(
        config.next_after(now).unwrap().unwrap(),
        Utc.with_ymd_and_hms(2026, 1, 1, 19, 0, 0).unwrap()
    );
    config.frequency = CleanupFrequency::Hourly;
    assert_eq!(
        config.next_after(now).unwrap().unwrap(),
        Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap()
    );
    config.frequency = CleanupFrequency::SixHourly;
    assert_eq!(
        config.next_after(now).unwrap().unwrap(),
        Utc.with_ymd_and_hms(2026, 1, 1, 4, 0, 0).unwrap()
    );
    config.requests.retention_days = 30;
    assert!(config.validate().is_err());
    config.requests.retention_days = 31;
    config.timezone = "invalid".into();
    assert!(config.validate().is_err());
}
