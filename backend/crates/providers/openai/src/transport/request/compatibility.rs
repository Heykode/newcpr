//! Narrow repairs for rejected OAuth wire formats, only on the selected send copy.

use std::collections::BTreeMap;

use jsonschema::{Retrieve, Uri};
use serde_json::{Map, Value};

struct LocalSchema;

/// Sub2API compatibility alias, applied only to the selected native OAuth request.
pub(crate) fn normalize_minimal_effort(request: &mut super::CodexResponsesRequest) {
    let Some(reasoning) = request
        .body_mut()
        .get_mut("reasoning")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if reasoning
        .get("effort")
        .and_then(Value::as_str)
        .is_some_and(|effort| effort.trim() == "minimal")
    {
        reasoning.insert("effort".into(), Value::String("none".into()));
        request.requested_reasoning_effort = Some("minimal".into());
    }
}

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

/// Repair only complete, unreferenced stateless messages with the rejected item_ prefix.
/// Reasoning IDs and tool call IDs may carry upstream identity and are never rewritten.
pub(crate) fn normalize_message_history_ids(body: &mut Map<String, Value>) {
    if body.get("store") == Some(&Value::Bool(true))
        || ["previous_response_id", "conversation"]
            .iter()
            .any(|key| body.get(*key).is_some_and(|v| !v.is_null()))
    {
        return;
    }
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    // Unknown history may hold references whose schema we do not understand.
    if input.iter().any(|item| {
        !matches!(
            item.get("type").and_then(Value::as_str),
            Some(
                "message"
                    | "reasoning"
                    | "compaction"
                    | "function_call"
                    | "function_call_output"
                    | "custom_tool_call"
                    | "custom_tool_call_output"
                    | "item_reference"
            )
        ) && !(item.get("type").is_none()
            && item.get("role").is_some_and(Value::is_string)
            && item
                .get("content")
                .is_some_and(|v| v.is_string() || v.is_array()))
    }) {
        return;
    }
    let mut ids = BTreeMap::<String, usize>::new();
    for item in input.iter() {
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            *ids.entry(id.to_owned()).or_default() += 1;
        }
    }
    for item in input {
        if item.get("type").and_then(Value::as_str) != Some("message")
            || !matches!(
                item.get("role").and_then(Value::as_str),
                Some("user" | "assistant" | "developer" | "system")
            )
            || !item
                .get("content")
                .is_some_and(|v| v.is_string() || v.is_array())
            || opaque_or_referenced(item)
        {
            continue;
        }
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(suffix) = id.strip_prefix("item_") else {
            continue;
        };
        let normalized = format!("msg_{suffix}");
        if suffix.is_empty()
            || normalized.len() > 64
            || !suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || ids.get(id) != Some(&1)
            || ids.contains_key(&normalized)
        {
            continue;
        }
        item["id"] = Value::String(normalized);
    }
}

/// Move only fully understood plaintext into an empty summary without dropping fields.
pub(crate) fn normalize_plaintext_reasoning(item: &mut Map<String, Value>, stateless: bool) {
    if item
        .get("encrypted_content")
        .is_some_and(|v| !v.is_null() && !v.as_str().is_some_and(|text| text.trim().is_empty()))
        || item
            .get("summary")
            .is_some_and(|v| !v.is_null() && !v.as_array().is_some_and(Vec::is_empty))
    {
        return;
    }
    let Some(content) = item
        .get("content")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty())
    else {
        return;
    };
    if !content.iter().all(|part| {
        part.as_object().is_some_and(|part| {
            part.len() == 2
                && part.get("type").and_then(Value::as_str) == Some("reasoning_text")
                && part.get("text").is_some_and(Value::is_string)
        })
    }) {
        return;
    }
    let summary = content
        .iter()
        .map(|part| {
            serde_json::json!({
                "type":"summary_text", "text":part["text"]
            })
        })
        .collect();
    item.insert("summary".to_owned(), Value::Array(summary));
    item.remove("content");
    if stateless
        && item
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("rs_"))
    {
        item.remove("id");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn minimal_alias_has_no_model_allowlist_and_keeps_other_reasoning_fields() {
        for model in ["gpt-5.6-sol", "gpt-6.1-sol", "future-model"] {
            let body = json!({"model": model, "input": "test", "reasoning": {
                "effort": " minimal ", "summary": "auto", "future_field": true
            }});
            let mut request =
                super::super::CodexResponsesRequest::from_body(body.as_object().unwrap().clone());
            normalize_minimal_effort(&mut request);
            assert_eq!(
                request.body()["reasoning"],
                json!({
                    "effort": "none", "summary": "auto", "future_field": true
                })
            );
            assert_eq!(
                request.requested_reasoning_effort.as_deref(),
                Some("minimal")
            );
            normalize_minimal_effort(&mut request);
            assert_eq!(
                request.requested_reasoning_effort.as_deref(),
                Some("minimal")
            );
            assert_eq!(body["reasoning"]["effort"], " minimal ");
        }
    }

    #[test]
    fn minimal_alias_does_not_invent_defaults_or_change_other_efforts() {
        for reasoning in [
            json!(null),
            json!({}),
            json!({"summary": "auto"}),
            json!({"effort": "none"}),
            json!({"effort": "low"}),
            json!({"effort": "medium"}),
            json!({"effort": "high"}),
            json!({"effort": "xhigh"}),
            json!({"effort": "max"}),
            json!({"effort": "future-effort"}),
        ] {
            let mut request = super::super::CodexResponsesRequest::from_body(
                json!({"model": "synthetic", "reasoning": reasoning})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            normalize_minimal_effort(&mut request);
            assert_eq!(request.body()["reasoning"], reasoning);
            assert!(request.requested_reasoning_effort.is_none());
        }
        let mut request = super::super::CodexResponsesRequest::from_body(Map::new());
        normalize_minimal_effort(&mut request);
        assert!(request.body().is_empty());
    }
}
