//! Validated fixed-point weights shared by Smart and Sticky scheduling.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Values", into = "Values")]
pub struct SmartSchedulingConfig {
    weights: [u8; 6],
    prefer_higher_weight: bool,
}

impl Default for SmartSchedulingConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

impl SmartSchedulingConfig {
    pub const fn defaults() -> Self {
        Self {
            weights: [10, 8, 10, 5, 0, 0],
            prefer_higher_weight: false,
        }
    }

    pub fn new(weights: [f64; 6], prefer_higher_weight: bool) -> Result<Self, &'static str> {
        let mut tenths = [0; 6];
        for (index, value) in weights.into_iter().enumerate() {
            if !value.is_finite()
                || !(0.0..=10.0).contains(&value)
                || (value * 10.0).round() / 10.0 != value
            {
                return Err("smart weights must be between 0 and 10 in steps of 0.1");
            }
            tenths[index] = (value * 10.0).round() as u8;
        }
        if tenths == [0; 6] {
            return Err("at least one smart weight must be positive");
        }
        Ok(Self {
            weights: tenths,
            prefer_higher_weight,
        })
    }

    pub fn weights(self) -> [f64; 6] {
        self.weights.map(|weight| f64::from(weight) / 10.0)
    }

    pub const fn prefer_higher_weight(self) -> bool {
        self.prefer_higher_weight
    }

    pub(crate) fn score_tolerance(self) -> f64 {
        0.05 * f64::from(
            self.weights[..5]
                .iter()
                .copied()
                .map(u16::from)
                .sum::<u16>(),
        ) / 33.0
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Values {
    load_weight: f64,
    quota_weight: f64,
    health_weight: f64,
    latency_weight: f64,
    reset_weight: f64,
    queue_weight: f64,
    prefer_higher_weight: bool,
}

impl TryFrom<Values> for SmartSchedulingConfig {
    type Error = &'static str;
    fn try_from(value: Values) -> Result<Self, Self::Error> {
        Self::new(
            [
                value.load_weight,
                value.quota_weight,
                value.health_weight,
                value.latency_weight,
                value.reset_weight,
                value.queue_weight,
            ],
            value.prefer_higher_weight,
        )
    }
}

impl From<SmartSchedulingConfig> for Values {
    fn from(value: SmartSchedulingConfig) -> Self {
        let [
            load_weight,
            quota_weight,
            health_weight,
            latency_weight,
            reset_weight,
            queue_weight,
        ] = value.weights();
        Self {
            load_weight,
            quota_weight,
            health_weight,
            latency_weight,
            reset_weight,
            queue_weight,
            prefer_higher_weight: value.prefer_higher_weight,
        }
    }
}
