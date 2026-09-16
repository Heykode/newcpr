use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaLearningObservation {
    pub account_id: String,
    pub provider_kind: String,
    pub plan_type: String,
    pub window_key: String,
    pub window_minutes: i32,
    pub used_percent: f64,
    pub reset_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub observed_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaLearningEstimate {
    pub account_id: String,
    pub window_key: String,
    pub window_minutes: i32,
    pub effective_limit_usd: Option<f64>,
    pub source: QuotaLearningSource,
    pub sample_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LearnedQuotaWindow {
    pub remaining_usd: Option<f64>,
    pub reset_at: Option<DateTime<Utc>>,
    pub low_sample: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaLearningSource {
    Personal,
    PlanAverage,
    Learning,
}
