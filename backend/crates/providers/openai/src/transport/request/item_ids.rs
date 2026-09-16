use serde_json::{Map, Value};

/// Remove only invalid optional IDs on self-contained input, never invent an
/// upstream reference or change the call_id shared by a tool and its result.
pub(super) fn normalize_standalone_item_ids(body: &mut Map<String, Value>) {
    if body.contains_key("previous_response_id") || body.contains_key("conversation") {
        return;
    }
    let Some(Value::Array(input)) = body.get_mut("input") else {
        return;
    };
    // Unknown items can introduce new reference semantics. Leave such transcripts,
    // and every explicit item-reference chain, entirely alone.
    if input.iter().any(|item| {
        !matches!(
            item.get("type").and_then(Value::as_str),
            Some(
                "message"
                    | "function_call"
                    | "function_call_output"
                    | "custom_tool_call"
                    | "custom_tool_call_output"
                    | "reasoning"
                    | "compaction"
            )
        )
    }) {
        return;
    }
    for item in input {
        let Some(fields) = item.as_object_mut() else {
            continue;
        };
        let (prefix, allowed, complete): (&str, &[&str], bool) =
            match fields.get("type").and_then(Value::as_str) {
                Some("message") => (
                    "msg",
                    &["type", "id", "role", "content", "status"],
                    matches!(
                        fields.get("role").and_then(Value::as_str),
                        Some("user" | "assistant" | "system" | "developer")
                    ) && fields.get("content").is_some_and(is_plain_text),
                ),
                Some("function_call") => (
                    "fc",
                    &["type", "id", "call_id", "name", "arguments", "status"],
                    nonblank(fields.get("call_id"))
                        && nonblank(fields.get("name"))
                        && fields.get("arguments").is_some_and(Value::is_string),
                ),
                Some("custom_tool_call") => (
                    "ctc",
                    &["type", "id", "call_id", "name", "input", "status"],
                    nonblank(fields.get("call_id"))
                        && nonblank(fields.get("name"))
                        && fields.get("input").is_some_and(Value::is_string),
                ),
                _ => continue,
            };
        if complete
            && fields.keys().all(|key| allowed.contains(&key.as_str()))
            && fields
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.starts_with(prefix))
        {
            fields.remove("id");
        }
    }
}

fn nonblank(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn is_plain_text(value: &Value) -> bool {
    match value {
        Value::String(_) => true,
        Value::Array(parts) => {
            !parts.is_empty()
                && parts.iter().all(|part| {
                    part.as_object().is_some_and(|fields| {
                        fields
                            .keys()
                            .all(|key| matches!(key.as_str(), "type" | "text"))
                            && matches!(
                                fields.get("type").and_then(Value::as_str),
                                Some("input_text" | "output_text")
                            )
                            && fields.get("text").is_some_and(Value::is_string)
                    })
                })
        }
        _ => false,
    }
}
