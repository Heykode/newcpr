//! Fixed-label facts from the already parsed wire body, including large requests.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

const MAX_ITEMS: usize = 512;
const MAX_PARTS: usize = 4096;

pub(crate) fn request_summary(body: &Map<String, Value>, bytes: usize) -> Value {
    let input = body.get("input").and_then(Value::as_array);
    let mut kinds = BTreeMap::<&str, usize>::new();
    let mut calls = BTreeSet::new();
    let mut results = Vec::new();
    let mut images = [0usize; 3];
    let mut tool_images = 0usize;
    let mut encrypted = 0usize;
    let mut text_bytes = 0usize;
    let mut parts = 0usize;
    let mut truncated = false;
    let mut pairing_incomplete = false;
    if let Some(input) = input {
        truncated = input.len() > MAX_ITEMS;
        for item in input.iter().take(MAX_ITEMS) {
            let kind = match item.get("type").and_then(Value::as_str) {
                Some("message") => "message",
                None if item.get("role").is_some() => "message",
                Some("function_call") => "function_call",
                Some("function_call_output") => "function_call_output",
                Some("reasoning") => "reasoning",
                Some("compaction") => "compaction",
                Some("compaction_trigger") => "compaction_trigger",
                _ => "other",
            };
            *kinds.entry(kind).or_default() += 1;
            encrypted += usize::from(item.get("encrypted_content").is_some());
            if matches!(kind, "function_call" | "function_call_output") {
                if let Some(id) = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty() && id.len() <= 256)
                {
                    if kind == "function_call" {
                        calls.insert(id);
                    } else {
                        results.push(id);
                    }
                } else {
                    pairing_incomplete = true;
                }
            }
            let content = if kind == "function_call_output" {
                item.get("output")
            } else {
                item.get("content")
            };
            if let Some(text) = content.and_then(Value::as_str) {
                text_bytes = text_bytes.saturating_add(text.len());
            }
            if let Some(content) = content.and_then(Value::as_array) {
                for part in content {
                    if parts == MAX_PARTS {
                        truncated = true;
                        break;
                    }
                    parts += 1;
                    match part.get("type").and_then(Value::as_str) {
                        Some("input_text" | "output_text") => {
                            text_bytes = text_bytes.saturating_add(
                                part.get("text").and_then(Value::as_str).map_or(0, str::len),
                            );
                        }
                        Some("input_image") => {
                            let index = if part.get("file_id").is_some() {
                                0
                            } else if part
                                .get("image_url")
                                .and_then(Value::as_str)
                                .is_some_and(|url| url.starts_with("data:"))
                            {
                                1
                            } else {
                                2
                            };
                            images[index] += 1;
                            tool_images += usize::from(kind == "function_call_output");
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    let unpaired = results.iter().filter(|id| !calls.contains(**id)).count();
    json!({
        "bodyBytes":bytes,
        "inputItems":input.map_or(0,Vec::len),
        "inputKinds":kinds,
        "fileImages":images[0],"inlineImages":images[1],"urlImages":images[2],
        "toolImages":tool_images,"textBytes":text_bytes,"encryptedItems":encrypted,
        "unpairedToolResults":unpaired,"toolPairingIncomplete":pairing_incomplete || truncated,
        "tools":body.get("tools").and_then(Value::as_array).map_or(0,Vec::len),
        "contextManagementPresent":body.contains_key("context_management"),
        "previousResponseIdPresent":body.contains_key("previous_response_id"),
        "truncated":truncated
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_large_request_summary_contains_no_client_content_or_identifiers() {
        let body = json!({"input":[
            {"role":"user","content":[{"type":"input_text","text":"private-content".repeat(20000)},{"type":"input_image","image_url":"https://example.com/private-content"}]},
            {"type":"function_call","call_id":"private-call","arguments":"private-content"},
            {"type":"function_call_output","call_id":"private-call","output":[{"type":"input_image","file_id":"private-file"}]},
            {"type":"function_call_output","call_id":"private-unpaired","output":"private-content"},
            {"type":"compaction","encrypted_content":"private-ciphertext"},
            {"type":"private-unknown"}
        ],"tools":[{"name":"private-name"}],"context_management":[{"private-field":"private-content"}]});
        let summary = request_summary(body.as_object().unwrap(), 400000);
        assert_eq!(summary["bodyBytes"], 400000);
        assert_eq!(summary["toolImages"], 1);
        assert_eq!(summary["urlImages"], 1);
        assert_eq!(summary["fileImages"], 1);
        assert_eq!(summary["encryptedItems"], 1);
        assert_eq!(summary["unpairedToolResults"], 1);
        assert_eq!(summary["contextManagementPresent"], true);
        assert_eq!(summary["truncated"], false);
        assert!(!summary.to_string().contains("private"));
        assert!(summary.to_string().len() < 1024);
    }

    #[test]
    fn excel_structure_scan_is_bounded_and_flags_incomplete_pairing() {
        let mut input = vec![json!({"role":"user","content":[]}); MAX_ITEMS + 1];
        input[0]["content"] = json!(vec![
            json!({"type":"input_image","image_url":"data:image/png;base64,AQID"});
            MAX_PARTS + 1
        ]);
        let body = json!({"input":input});
        let summary = request_summary(body.as_object().unwrap(), 1);
        assert_eq!(summary["inlineImages"], MAX_PARTS);
        assert_eq!(summary["truncated"], true);
        assert_eq!(summary["toolPairingIncomplete"], true);
    }
}
