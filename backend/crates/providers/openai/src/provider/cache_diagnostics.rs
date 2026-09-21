//! Opt-in cache comparisons with bounded work and no retained prompt text.

use serde_json::{Map, Value, json};

const MAX_BYTES: usize = 64 * 1024;
const MAX_FIELD_BYTES: usize = 16 * 1024;
const MAX_NODES: usize = 2048;

pub(super) fn fingerprints(body: &Map<String, Value>) -> Value {
    let mut bytes = MAX_BYTES;
    let mut nodes = MAX_NODES;
    let mut field = |value: Option<&Value>| {
        let Some(value) = value else {
            return Value::Null;
        };
        let mut available = bytes.min(MAX_FIELD_BYTES);
        let initial = available;
        // Check string lengths before serialization, which otherwise scans an entire string.
        let fits = bounded_size(value, &mut available, &mut nodes, 0);
        bytes -= initial - available;
        if !fits {
            return json!({"omitted": "inspection_limit"});
        }
        match serde_json::to_vec(value) {
            Ok(encoded) => gateway_core::diagnostics::body_fingerprint(&encoded),
            Err(_) => json!({"omitted": "encoding_error"}),
        }
    };
    let mut fields = Map::new();
    for (label, key) in [
        ("cacheKey", "prompt_cache_key"),
        ("instructions", "instructions"),
        ("tools", "tools"),
        ("reasoning", "reasoning"),
        ("text", "text"),
    ] {
        fields.insert(label.to_owned(), field(body.get(key)));
    }
    fields.insert(
        "inputPrefix".to_owned(),
        body.get("input")
            .and_then(Value::as_array)
            .map(|items| Value::Array(items.iter().take(8).map(|item| field(Some(item))).collect()))
            .unwrap_or(Value::Null),
    );
    fields.insert("maxInspectedBytes".to_owned(), json!(MAX_BYTES));
    Value::Object(fields)
}

fn bounded_size(value: &Value, bytes: &mut usize, nodes: &mut usize, depth: usize) -> bool {
    if depth > 64 || *nodes == 0 {
        return false;
    }
    *nodes -= 1;
    if !consume(bytes, 2) {
        return false;
    }
    match value {
        Value::String(value) => consume(bytes, value.len().saturating_mul(6)),
        Value::Number(value) => consume(bytes, value.as_str().len()),
        Value::Bool(_) | Value::Null => consume(bytes, 5),
        Value::Array(items) => items
            .iter()
            .all(|item| consume(bytes, 1) && bounded_size(item, bytes, nodes, depth + 1)),
        Value::Object(fields) => fields.iter().all(|(key, value)| {
            consume(bytes, key.len().saturating_mul(6).saturating_add(4))
                && bounded_size(value, bytes, nodes, depth + 1)
        }),
    }
}

fn consume(remaining: &mut usize, amount: usize) -> bool {
    if amount > *remaining {
        *remaining = 0;
        false
    } else {
        *remaining -= amount;
        true
    }
}
