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
