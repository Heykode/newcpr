//! Bounded formatting compatibility; never evaluate code or infer missing values.

use serde_json::{Value, json};

use super::ExcelRequestError;

const MAX_BYTES: usize = 1024 * 1024;
const INVALID: ExcelRequestError = ExcelRequestError::ToolCall;

pub(super) fn native_envelope(native: &Value) -> Result<(Value, bool), ExcelRequestError> {
    let arguments = json_value(native.get("arguments").ok_or(INVALID)?)?;
    let arguments = arguments.as_object().ok_or(INVALID)?;
    if let Some(summary) = arguments.get("summary").and_then(Value::as_str)
        && let Some(name) = summary
            .strip_prefix("cpr.custom/")
            .or_else(|| summary.strip_prefix("codex2api.custom/"))
    {
        if name.is_empty()
            || name
                .chars()
                .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '/' | '\\'))
        {
            return Err(INVALID);
        }
        let input = arguments
            .get("code")
            .and_then(Value::as_str)
            .filter(|s| s.len() <= MAX_BYTES)
            .ok_or(INVALID)?;
        return Ok((json!({"name":name,"input":input}), true));
    }
    let mut value = arguments.get("code").cloned().ok_or(INVALID)?;
    for _ in 0..3 {
        let envelope = decode_code(value)?;
        if !matches!(name(&envelope)?, "run_officejs" | "functions.run_officejs") {
            return Ok((envelope, false));
        }
        let args = json_value(arguments_field(&envelope)?)?;
        value = args.get("code").cloned().ok_or(INVALID)?;
    }
    Err(INVALID)
}

pub(super) fn name(envelope: &Value) -> Result<&str, ExcelRequestError> {
    let name = envelope.get("name").and_then(Value::as_str);
    let alias = envelope.get("tool").and_then(Value::as_str);
    if name.zip(alias).is_some_and(|(name, alias)| name != alias) {
        return Err(INVALID);
    }
    name.or(alias)
        .filter(|name| !name.is_empty() && name.trim() == *name)
        .ok_or(INVALID)
}

pub(super) fn arguments_field(envelope: &Value) -> Result<&Value, ExcelRequestError> {
    match (envelope.get("arguments"), envelope.get("args")) {
        (Some(_), Some(_)) | (None, None) => Err(INVALID),
        (Some(value), None) | (None, Some(value)) => Ok(value),
    }
}

pub(super) fn json_value(value: &Value) -> Result<Value, ExcelRequestError> {
    match value.as_str() {
        Some(raw) if raw.len() <= MAX_BYTES => serde_json::from_str(raw).map_err(|_| INVALID),
        Some(_) => Err(INVALID),
        None => Ok(value.clone()),
    }
}

fn decode_code(mut value: Value) -> Result<Value, ExcelRequestError> {
    for _ in 0..4 {
        if value.is_object() {
            return Ok(value);
        }
        let raw = value
            .as_str()
            .filter(|s| s.len() <= MAX_BYTES)
            .ok_or(INVALID)?
            .trim();
        if let Some(decoded) = parse_formatted_json(raw) {
            value = decoded;
            continue;
        }
        if let Some((prefix, fenced)) = raw.split_once("```")
            && prose_prefix(prefix)
            && let Some((language, body)) = fenced.split_once('\n')
            && (language.trim().is_empty() || language.trim().eq_ignore_ascii_case("json"))
            && let Some(body) = body.strip_suffix("```")
        {
            value = body.trim().into();
            continue;
        }
        if let Some(decoded) = embedded_envelope(raw) {
            value = decoded;
            continue;
        }
        return Err(INVALID);
    }
    value.is_object().then_some(value).ok_or(INVALID)
}

fn embedded_envelope(raw: &str) -> Option<Value> {
    let index = raw.find('{')?;
    let prefix = raw[..index].trim();
    let mut decoder = serde_json::Deserializer::from_str(&raw[index..]).into_iter::<Value>();
    let value = decoder.next()?.ok()?;
    if !value.is_object() {
        return None;
    }
    let suffix = raw[index + decoder.byte_offset()..].trim();
    if prose_prefix(prefix) && prose_prefix(suffix) {
        return Some(value);
    }
    // Recognize one inert name({...}) wrapper, not a program or multiple candidates.
    let wrapper = prefix.strip_suffix('(')?.trim();
    if !matches!(suffix, ")" | ");")
        || wrapper.is_empty()
        || !wrapper
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'.'))
    {
        return None;
    }
    let declared = name(&value).ok()?;
    (wrapper == declared || wrapper.strip_prefix("functions.") == Some(declared)).then_some(value)
}

fn prose_prefix(value: &str) -> bool {
    value.len() <= 512 && !value.contains(['{', '}', '[', ']', '(', ')', ';', '=', '`', '"'])
}

fn parse_formatted_json(raw: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(raw) {
        return Some(value);
    }
    let bytes = raw.as_bytes();
    let mut repaired = Vec::with_capacity(bytes.len());
    let mut quoted = false;
    let mut index = 0;
    while let Some(&ch) = bytes.get(index) {
        if ch == b'"' {
            quoted = !quoted;
        }
        if quoted {
            let escape: Option<&[u8]> = match ch {
                b'\n' => Some(b"\\n"),
                b'\r' => Some(b"\\r"),
                b'\t' => Some(b"\\t"),
                _ => None,
            };
            if let Some(escape) = escape {
                repaired.extend_from_slice(escape);
                index += 1;
                continue;
            }
        }
        if quoted
            && ch == b'\\'
            && let Some(&next) = bytes.get(index + 1)
        {
            let valid = b"\"\\/bfnrt".contains(&next)
                || (next == b'u'
                    && bytes
                        .get(index + 2..index + 6)
                        .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit)));
            repaired.push(b'\\');
            if valid {
                repaired.push(next);
                index += 2;
            } else {
                repaired.push(b'\\');
                index += 1;
            }
        } else {
            repaired.push(ch);
            index += 1;
        }
    }
    serde_json::from_slice(&repaired).ok()
}
