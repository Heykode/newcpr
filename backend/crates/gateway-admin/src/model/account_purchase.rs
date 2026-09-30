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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn purchase_patch_requires_explicit_amount_and_rejects_ambiguous_values() {
        for value in [
            json!({}),
            json!({"cycleStart":"2026-01-01"}),
            json!({"amountCny":50}),
            json!({"amountCny":"50", "extra":true}),
            json!({"amountCny":"1","cycleStart":"2026-02-30"}),
        ] {
            assert!(serde_json::from_value::<AccountPurchaseUpdate>(value).is_err());
        }
        let clear: AccountPurchaseUpdate =
            serde_json::from_value(json!({"amountCny":null})).unwrap();
        assert_eq!(clear.amount_cny, None);
        assert!(clear.validate().is_ok());
        for amount in ["", "-1", "1e3", "NaN", "10000000000", "0.00000000001"] {
            let patch = AccountPurchaseUpdate {
                amount_cny: Some(amount.into()),
                cycle_start: None,
            };
            assert!(patch.validate().is_err(), "{amount}");
        }
        for amount in ["0", "50", "0.0000000001", "9999999999.9999999999"] {
            let patch = AccountPurchaseUpdate {
                amount_cny: Some(amount.into()),
                cycle_start: Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            };
            assert!(patch.validate().is_ok(), "{amount}");
        }
        let future = (chrono::Utc::now() + chrono::Duration::days(2)).date_naive();
        assert!(
            AccountPurchaseUpdate {
                amount_cny: Some("1".into()),
                cycle_start: Some(future)
            }
            .validate()
            .is_err()
        );
    }
}
