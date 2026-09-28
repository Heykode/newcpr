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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_only_message_and_tool_result_parts_in_place() {
        for kind in [
            "message",
            "agent_message",
            "",
            "function_call_output",
            "custom_tool_call_output",
        ] {
            for role in ["user", "assistant", "developer"] {
                let field = if kind.ends_with("_output") {
                    "output"
                } else {
                    "content"
                };
                let original = json!({"input":[{"type":kind,"role":role,"call_id":"call_fixture",
                "author":"worker","recipient":"parent",field:[
                    {"type":"input_text","text":"before"},
                    {"type":"encrypted_content","encrypted_content":"opaque-fixture"},
                    {"type":"input_text","text":"after"}
                ]}]});
                let mut source = original.as_object().unwrap().clone();
                assert_eq!(omit_encrypted_content(&mut source), 1);
                let item = &source["input"][0];
                assert_eq!(item["call_id"], "call_fixture");
                assert_eq!(item["author"], "worker");
                assert_eq!(item["recipient"], "parent");
                assert_eq!(item[field][0], original["input"][0][field][0]);
                assert_eq!(item[field][2], original["input"][0][field][2]);
                assert_eq!(
                    item[field][1],
                    json!({"type":if role == "assistant" && field == "content" {"output_text"} else {"input_text"},"text":OMITTED})
                );
                assert_eq!(omit_encrypted_content(&mut source), 0);
                assert_eq!(original["input"][0][field][1]["type"], "encrypted_content");
            }
        }
    }

    #[test]
    fn preserves_reasoning_compaction_arguments_definitions_and_large_integers() {
        let original = json!({"input":[
            {"type":"reasoning","encrypted_content":"reasoning-fixture"},
            {"type":"compaction","encrypted_content":"compact-fixture"},
            {"type":"function_call","arguments":"encrypted_content","encrypted_function_args":["message"]},
            {"role":"user","content":[{"type":"input_text","text":"encrypted_content"}]},
            {"type":"function_call_output","output":"encrypted_content"}
        ],"tools":[{"type":"function","parameters":{"encrypted_content":true}}],
            "metadata":{"sequence":9007199254740993_u64}});
        let mut source = original.as_object().unwrap().clone();
        let bytes = serde_json::to_vec(&source).unwrap();
        assert_eq!(omit_encrypted_content(&mut source), 0);
        assert_eq!(serde_json::to_vec(&source).unwrap(), bytes);
        source
            .get_mut("input")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(json!({
                "role":"user","content":[{"type":"encrypted_content","encrypted_content":"fixture"}]
            }));
        assert_eq!(omit_encrypted_content(&mut source), 1);
        source
            .get_mut("input")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .pop();
        assert_eq!(serde_json::to_value(&source).unwrap(), original);
    }
}
