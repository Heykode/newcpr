//! 查询 wire 校验的固定合同测试。

use gateway_api::admin::observability::{
    DashboardQuery, DiagnosticDimension, DiagnosticsQuery, OpsQuery, TrendKind, UsageQuery,
    parse_attempt_index, parse_datetime, parse_status,
};
use serde_json::json;

#[test]
fn dashboard_query_should_parse_terminal_trend_kinds() {
    let query: DashboardQuery = serde_json::from_value(json!({"kind": "errors"})).unwrap();
    assert_eq!(query.trend_kind().unwrap(), TrendKind::Errors);
}

#[test]
fn dashboard_query_should_reject_unknown_trend_kind() {
    let query: DashboardQuery = serde_json::from_value(json!({"kind": "secret"})).unwrap();
    assert_eq!(query.trend_kind().unwrap_err().field(), "kind");
}

#[test]
fn usage_query_should_accept_element_plus_pagination() {
    let query: UsageQuery = serde_json::from_value(json!({
        "currentPage": 129,
        "pageSize": 10
    }))
    .unwrap();
    assert_eq!(query.validate_pagination().unwrap(), (129, 10));
}

#[test]
fn usage_query_should_reject_removed_total_toggle() {
    assert!(serde_json::from_value::<UsageQuery>(json!({"includeTotal": true})).is_err());
}

#[test]
fn usage_query_should_reject_removed_cursor_contract() {
    assert!(serde_json::from_value::<UsageQuery>(json!({"cursor": "opaque"})).is_err());
}

#[test]
fn usage_query_should_reject_nonstandard_page_name() {
    assert!(serde_json::from_value::<UsageQuery>(json!({"page": 1})).is_err());
}

#[test]
fn ops_query_should_reject_page_size_above_terminal_limit() {
    let query: OpsQuery = serde_json::from_value(json!({"pageSize": 101})).unwrap();
    assert_eq!(query.validate_pagination().unwrap_err().field(), "pageSize");
}

#[test]
fn ops_query_should_reject_removed_filters() {
    assert!(serde_json::from_value::<OpsQuery>(json!({"route": "/v1/responses"})).is_err());
    assert!(serde_json::from_value::<OpsQuery>(json!({"failureClass": "rate_limited"})).is_err());
}

#[test]
fn diagnostics_query_should_keep_wire_dimension_name() {
    let query: DiagnosticsQuery =
        serde_json::from_value(json!({"dimension": "failure_class"})).unwrap();
    assert_eq!(query.dimension().unwrap(), DiagnosticDimension::Failure);
    assert_eq!(DiagnosticDimension::Failure.display_name(), "failureClass");
}

#[test]
fn key_model_diagnostics_should_default_to_bounded_pages() {
    let query: DiagnosticsQuery = serde_json::from_value(json!({"dimension": "keyModel"})).unwrap();
    let dimension = query.dimension().unwrap();
    assert_eq!(dimension, DiagnosticDimension::KeyModel);
    assert_eq!(dimension.display_name(), "keyModel");
    let page = query.page(dimension).unwrap().unwrap();
    assert_eq!(page.current_page, 1);
    assert_eq!(page.page_size.get(), 20);
}

#[test]
fn diagnostics_pagination_should_reject_invalid_pages_and_other_dimensions() {
    for (value, field) in [
        (
            json!({"dimension": "keyModel", "currentPage": 0}),
            "currentPage",
        ),
        (json!({"dimension": "keyModel", "pageSize": 0}), "pageSize"),
        (
            json!({"dimension": "keyModel", "pageSize": 101}),
            "pageSize",
        ),
        (json!({"dimension": "model", "currentPage": 1}), "dimension"),
        (json!({"dimension": "apiKey", "pageSize": 20}), "dimension"),
    ] {
        let query: DiagnosticsQuery = serde_json::from_value(value).unwrap();
        assert_eq!(
            query.page(query.dimension().unwrap()).unwrap_err().field(),
            field
        );
    }
    let query: DiagnosticsQuery = serde_json::from_value(json!({
        "dimension": "keyModel", "currentPage": 2, "pageSize": 100,
    }))
    .unwrap();
    let page = query.page(query.dimension().unwrap()).unwrap().unwrap();
    assert_eq!(page.current_page, 2);
    assert_eq!(page.page_size.get(), 100);
    assert!(
        DiagnosticsQuery::default()
            .page(DiagnosticDimension::Model)
            .unwrap()
            .is_none()
    );
    assert!(
        serde_json::from_value::<DiagnosticsQuery>(json!({
            "dimension": "keyModel", "currentPage": -1,
        }))
        .is_err()
    );
}

#[test]
fn scalar_query_parsers_should_reject_out_of_range_values_without_echoing_input() {
    assert_eq!(parse_status(Some(99)).unwrap_err().field(), "statusCode");
    assert_eq!(
        parse_attempt_index(Some(0)).unwrap_err().field(),
        "attemptIndex"
    );
    assert_eq!(
        parse_datetime(Some("not-a-time")).unwrap_err().field(),
        "timeRange"
    );
}
