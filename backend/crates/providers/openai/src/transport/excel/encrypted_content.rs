//! Optional lossy message handling aligned with Sub2API #154 (96cb3762).
//! Operates on the Excel send copy, never the client's or replay store's history.

use serde_json::{Map, Value, json};

const OMITTED: &str = "[Encrypted content omitted: it cannot be forwarded through Excel / BPS.]";

pub(crate) fn omit_encrypted_content(source: &mut Map<String, Value>) -> usize {
    let Some(input) = source.get_mut("input").and_then(Value::as_array_mut) else {
        return 0;
    };
    let mut replaced = 0;
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        let field = match item.get("type").and_then(Value::as_str) {
            None | Some("" | "message" | "agent_message") => "content",
            Some("function_call_output" | "custom_tool_call_output") => "output",
            _ => continue,
        };
        let kind = if field == "content"
            && item.get("role").and_then(Value::as_str) == Some("assistant")
        {
            "output_text"
        } else {
            "input_text"
        };
        let Some(parts) = item.get_mut(field).and_then(Value::as_array_mut) else {
            continue;
        };
        for part in parts {
            if part.get("type").and_then(Value::as_str) == Some("encrypted_content") {
                *part = json!({"type": kind, "text": OMITTED});
                replaced += 1;
            }
        }
    }
    replaced
}
