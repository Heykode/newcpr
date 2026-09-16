//! Request location is business metadata, independent of transport and device identity.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequestLocation {
    pub country: String,
    pub region: String,
    pub city: String,
    pub timezone: chrono_tz::Tz,
}

impl Default for RequestLocation {
    fn default() -> Self {
        Self {
            country: "US".to_owned(),
            region: "Ohio".to_owned(),
            city: "Piketon".to_owned(),
            timezone: chrono_tz::America::New_York,
        }
    }
}

impl RequestLocation {
    pub fn validate(&self) -> bool {
        self.country.len() == 2
            && self.country.bytes().all(|byte| byte.is_ascii_uppercase())
            && [&self.region, &self.city].into_iter().all(|value| {
                !value.chars().any(char::is_control)
                    && !value.trim().is_empty()
                    && value.trim().chars().count() <= 128
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_validation_rejects_empty_controls_and_invalid_timezones() {
        assert!(RequestLocation::default().validate());
        for country in ["", "USA", "us", "U1"] {
            assert!(
                !RequestLocation {
                    country: country.into(),
                    ..Default::default()
                }
                .validate()
            );
        }
        for text in ["", " ", "Ohio\n"] {
            assert!(
                !RequestLocation {
                    region: text.into(),
                    ..Default::default()
                }
                .validate()
            );
            assert!(
                !RequestLocation {
                    city: text.into(),
                    ..Default::default()
                }
                .validate()
            );
        }
        let mut value = serde_json::to_value(RequestLocation::default()).unwrap();
        value["timezone"] = serde_json::json!("Invalid/Zone");
        assert!(serde_json::from_value::<RequestLocation>(value).is_err());
    }
}
