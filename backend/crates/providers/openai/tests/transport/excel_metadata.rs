use super::*;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn caller_cannot_supply_bps_transport_identity_or_tools_version() {
    let source = json!({"model":"gpt-example","input":"hello","metadata":{
        "task_id":"caller-task","turn_id":"caller-turn","agent_iteration":"999",
        "bps_tools_version_id":"caller-tools","trace_label":"allowed","number":42,
        "nested":{"unsupported":true}
    }});
    let source = source.as_object().unwrap();
    let tools = ClientTools::parse(source).unwrap();
    let result = prepare_request(source, &tools, &BTreeMap::new(), None).unwrap();
    assert!(result["metadata"].get("bps_tools_version_id").is_none());
    assert_ne!(result["metadata"]["task_id"], "caller-task");
    assert_ne!(result["metadata"]["turn_id"], "caller-turn");
    assert_ne!(result["metadata"]["agent_iteration"], "999");
    assert_eq!(result["metadata"]["trace_label"], "allowed");
    assert_eq!(result["metadata"]["number"], "42");
    assert!(result["metadata"].get("nested").is_none());
}
