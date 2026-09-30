use gateway_api::admin::accounts::{
    AccountImportRequest, BatchUpdateAccountsRequest, UpdateAccountRequest,
};
use serde_json::json;

#[test]
fn purchase_wire_preserves_omission_and_explicit_clearing_in_all_edit_paths() {
    let batch: BatchUpdateAccountsRequest =
        serde_json::from_value(json!({"accountIds":["acct_test"]})).unwrap();
    assert!(batch.purchase_cost.is_none());
    for amount in [json!("50.0000000001"), json!(null)] {
        let purchase = json!({"amountCny":amount,"cycleStart":"2026-01-31"});
        let batch: BatchUpdateAccountsRequest =
            serde_json::from_value(json!({"accountIds":["acct_test"],"purchaseCost":purchase}))
                .unwrap();
        assert!(batch.validate().is_ok());
        assert!(batch.enabled.is_none());
        assert!(batch.egress_mode.is_none());
        assert!(batch.responses_upstream.is_none());
        let single: UpdateAccountRequest=serde_json::from_value(json!({"accountId":"acct_test","purchaseCost":purchase,"enabled":true,"concurrencyLimit":null,"weight":1,"groupIds":[]})).unwrap();
        assert!(single.validate().is_ok());
        assert_eq!(single.purchase_cost, batch.purchase_cost);
        let import: AccountImportRequest=serde_json::from_value(json!({"provider":"openai","data":{},"settings":{"purchaseCost":purchase,"enabled":true,"concurrencyLimit":null,"weight":1,"groupIds":[]}})).unwrap();
        assert!(import.validate().is_ok());
        assert_eq!(
            serde_json::to_value(&import).unwrap()["settings"]["purchaseCost"],
            purchase
        );
    }
    for invalid in [
        json!({}),
        json!({"amountCny":3}),
        json!({"amountCny":"-1"}),
        json!({"amountCny":"1e3"}),
        json!({"amountCny":"1","cycleStart":"2100-01-01"}),
    ] {
        let request = serde_json::from_value::<BatchUpdateAccountsRequest>(
            json!({"accountIds":["acct_test"],"purchaseCost":invalid}),
        );
        assert!(request.is_err() || request.unwrap().validate().is_err());
    }
}
