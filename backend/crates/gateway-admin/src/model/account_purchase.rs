//! Optional admin purchase metadata, separate from provider quota and customer billing.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::{AdminError, observability::DecimalAmount};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountPurchaseUpdate {
    #[serde(deserialize_with = "required_amount")]
    pub amount_cny: Option<String>,
    pub cycle_start: Option<NaiveDate>,
}

fn required_amount<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

impl AccountPurchaseUpdate {
    pub fn validate(&self) -> Result<(), AdminError> {
        if let Some(amount) = &self.amount_cny {
            amount
                .parse::<DecimalAmount>()
                .map_err(|_| AdminError::invalid("账号成本必须为非负十进制金额"))?;
        }
        if let Some(date) = self.cycle_start {
            let today = (chrono::Utc::now() + chrono::Duration::hours(8)).date_naive();
            if date > today || date < NaiveDate::from_ymd_opt(2000, 1, 1).expect("valid date") {
                return Err(AdminError::invalid(
                    "成本周期开始日期必须在2000年起至今天之间",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPurchaseView {
    pub amount_cny: Option<String>,
    pub cycle_anchor: Option<NaiveDate>,
    pub period_start: Option<NaiveDate>,
    pub period_end: Option<NaiveDate>,
    pub usage_usd: String,
    pub breakeven_cny_per_usd: Option<String>,
    pub history_complete: bool,
    pub history_complete_from: Option<chrono::DateTime<chrono::Utc>>,
}
