//! Narrow repairs for rejected OAuth wire formats, only on the selected send copy.

use std::collections::BTreeMap;

use jsonschema::{Retrieve, Uri};
use serde_json::{Map, Value};

struct LocalSchema;

impl Retrieve for LocalSchema {
    fn retrieve(&self, _: &Uri<String>) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schemas are disabled".into())
    }
}

pub(crate) fn default_missing_format_name(body: &mut Map<String, Value>) {
    let Some(format) = body
        .get_mut("text")
        .and_then(|text| text.get_mut("format"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if format.get("type").and_then(Value::as_str) != Some("json_schema")
        || format.contains_key("name")
        || format
            .keys()
            .any(|key| !matches!(key.as_str(), "type" | "schema" | "strict" | "description"))
        || format.get("strict").is_some_and(|v| !v.is_boolean())
        || format.get("description").is_some_and(|v| !v.is_string())
    {
        return;
    }
    let Some(schema) = format.get("schema").filter(|v| v.is_object()) else {
        return;
    };
    if serde_json::to_vec(schema).is_ok_and(|v| v.len() <= 1024 * 1024)
        && jsonschema::draft202012::options()
            .with_retriever(LocalSchema)
            .should_validate_formats(true)
            .build(schema)
            .is_ok()
    {
        format.insert("name".into(), "response".into());
    }
}

fn opaque_or_referenced(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(opaque_or_referenced),
        Value::Object(fields) => {
            fields.contains_key("encrypted_content")
                || matches!(
                    fields.get("type").and_then(Value::as_str),
                    Some("encrypted_content" | "item_reference")
                )
                || fields.values().any(opaque_or_referenced)
        }
        _ => false,
    }
}

pub(crate) fn normalize_custom_history_ids(body: &mut Map<String, Value>) {
    // ID-only or encrypted continuation cannot prove a safe full-history replay.
    if ["previous_response_id", "conversation"]
        .iter()
        .any(|key| body.get(*key).is_some_and(|v| !v.is_null()))
    {
        return;
    }
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    if !input.iter().any(|item| {
        item["type"] == "custom_tool_call"
            && item["id"].as_str().is_some_and(|id| id.starts_with("fc_"))
    }) {
        return;
    }
    let mut calls = BTreeMap::new();
    let mut outputs = BTreeMap::new();
    for item in input.iter() {
        if !item.is_object() || opaque_or_referenced(item) {
            return;
        }
        let kind = item.get("type").and_then(Value::as_str);
        let (map, custom) = match kind {
            Some("function_call") => (&mut calls, false),
            Some("custom_tool_call") => (&mut calls, true),
            Some("function_call_output") => (&mut outputs, false),
            Some("custom_tool_call_output") => (&mut outputs, true),
            None | Some("message" | "agent_message" | "reasoning" | "additional_tools") => continue,
            _ => return,
        };
        let Some(id) = item["call_id"].as_str().filter(|id| !id.is_empty()) else {
            return;
        };
        if map.insert(id.to_owned(), custom).is_some() {
            return;
        }
    }
    if calls != outputs {
        return;
    }
    for item in input {
        if item["type"] == "custom_tool_call"
            && item["input"].is_string()
            && item["id"].as_str().is_some_and(|id| id.starts_with("fc_"))
        {
            item.as_object_mut().expect("checked object").remove("id");
        }
    }
}
