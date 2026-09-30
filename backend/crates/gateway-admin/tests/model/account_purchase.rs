use chrono::NaiveDate;
use gateway_admin::model::account_purchase::AccountPurchaseUpdate;
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
    let clear: AccountPurchaseUpdate = serde_json::from_value(json!({"amountCny":null})).unwrap();
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
