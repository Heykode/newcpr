//! Bounded recovery aligned with Sub2API #118, commit 3bfce058.
//! Only a rejected request copy loses opaque reasoning, never user context.

use serde_json::{Map, Value};

const MAX_REJECTION_BYTES: usize = 512 * 1024;

pub(crate) fn retry_body(body: &Map<String, Value>, rejection: &str) -> Option<Map<String, Value>> {
    if !is_rejection(rejection) {
        return None;
    }
    let input = body.get("input")?.as_array()?;
    let mut kept = Vec::with_capacity(input.len());
    let mut removed = false;
    let mut has_history = false;
    for item in input {
        let kind = item.get("type").and_then(Value::as_str);
        if kind == Some("reasoning")
            && item
                .get("encrypted_content")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        {
            removed = true;
            continue;
        }
        if item.get("encrypted_content").is_some()
            || item
                .get("encrypted_function_args")
                .and_then(Value::as_array)
                .is_some_and(|parts| !parts.is_empty())
        {
            return None;
        }
        for field in ["content", "output"] {
            if item
                .get(field)
                .and_then(Value::as_array)
                .is_some_and(|parts| {
                    parts.iter().any(|part| {
                        part.get("type").and_then(Value::as_str) == Some("encrypted_content")
                            || part.get("encrypted_content").is_some()
                    })
                })
            {
                return None;
            }
        }
        has_history |= matches!(
            item.get("role").and_then(Value::as_str),
            Some("user" | "assistant")
        ) || matches!(
            kind,
            Some("agent_message" | "function_call" | "function_call_output")
        );
        kept.push(item.clone());
    }
    if !removed || !has_history {
        return None;
    }
    let mut retry = body.clone();
    retry.insert("input".into(), Value::Array(kept));
    Some(retry)
}

pub(super) fn is_rejection(raw: &str) -> bool {
    if raw.len() > MAX_REJECTION_BYTES {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return false;
    };
    if let Some(code) = value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .filter(|code| !code.is_empty())
    {
        return code == "invalid_encrypted_content";
    }
    let Some(message) = value.pointer("/error/message").and_then(Value::as_str) else {
        return false;
    };
    let message = message.trim().to_ascii_lowercase();
    message.starts_with("the encrypted content ")
        && message.contains("could not be verified")
        && message.contains("could not be decrypted or parsed")
}
