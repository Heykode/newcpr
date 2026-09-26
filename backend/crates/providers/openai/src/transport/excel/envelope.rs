//! Bounded formatting compatibility; never evaluate code or infer missing values.

use serde_json::{Value, json};

use super::ExcelRequestError;

const MAX_BYTES: usize = 1024 * 1024;
const INVALID: ExcelRequestError = ExcelRequestError::ToolCall;
pub(super) const FUNCTION_CODE_PREFIX: &str = "codex2api.function_code/";
pub(super) const FUNCTION_CMD_PREFIX: &str = "codex2api.function_cmd/";

pub(super) fn native_envelope(
    native: &Value,
    resolve: &impl Fn(&str) -> Option<(String, bool)>,
    supports_code: &impl Fn(&str) -> bool,
    supports_cmd: &impl Fn(&str) -> bool,
) -> Result<(Value, bool), ExcelRequestError> {
    let arguments = json_value(native.get("arguments").ok_or(INVALID)?)?;
    let arguments = arguments.as_object().ok_or(INVALID)?;
    if let Some(name) = arguments
        .get("summary")
        .and_then(Value::as_str)
        .and_then(|summary| summary.strip_prefix(FUNCTION_CMD_PREFIX))
    {
        if !supports_cmd(name) {
            return Err(INVALID);
        }
        let cmd = arguments
            .get("code")
            .and_then(Value::as_str)
            .ok_or(INVALID)?;
        let metadata = arguments
            .get("extended_summary")
            .and_then(Value::as_str)
            .ok_or(INVALID)?;
        if cmd.len() > MAX_BYTES || metadata.len() > MAX_BYTES - cmd.len() {
            return Err(INVALID);
        }
        let mut args: Value = serde_json::from_str(metadata).map_err(|_| INVALID)?;
        let object = args.as_object_mut().ok_or(INVALID)?;
        if object
            .get("cmd")
            .is_some_and(|value| value.as_str() != Some(cmd))
        {
            return Err(INVALID);
        }
        object.insert("cmd".into(), cmd.into());
        if args.to_string().len() > MAX_BYTES {
            return Err(INVALID);
        }
        return Ok((json!({"name":name,"arguments":args}), false));
    }
    if let Some(name) = arguments
        .get("summary")
        .and_then(Value::as_str)
        .and_then(|summary| summary.strip_prefix(FUNCTION_CODE_PREFIX))
    {
        if !supports_code(name) {
            return Err(INVALID);
        }
        let code = arguments
            .get("code")
            .and_then(Value::as_str)
            .ok_or(INVALID)?;
        let metadata = arguments
            .get("extended_summary")
            .and_then(Value::as_str)
            .ok_or(INVALID)?;
        if code.len() > MAX_BYTES || metadata.len() > MAX_BYTES - code.len() {
            return Err(INVALID);
        }
        // Code is opaque source text, never parsed, repaired or evaluated.
        let mut args: Value = serde_json::from_str(metadata).map_err(|_| INVALID)?;
        let object = args.as_object_mut().ok_or(INVALID)?;
        if object.contains_key("code") {
            return Err(INVALID);
        }
        object.insert("code".into(), code.into());
        if args.to_string().len() > MAX_BYTES {
            return Err(INVALID);
        }
        return Ok((json!({"name":name,"arguments":args}), false));
    }
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
        let envelope = decode_code(value, resolve)?;
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

fn decode_code(
    mut value: Value,
    resolve: &impl Fn(&str) -> Option<(String, bool)>,
) -> Result<Value, ExcelRequestError> {
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
        if let Some(decoded) = catalog_invocation(raw, resolve) {
            return Ok(decoded);
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
    (prose_prefix(prefix) && prose_prefix(suffix)).then_some(value)
}

fn catalog_invocation(
    raw: &str,
    resolve: &impl Fn(&str) -> Option<(String, bool)>,
) -> Option<Value> {
    let mut raw = raw.trim();
    for keyword in ["return", "await"] {
        if let Some(tail) = raw.strip_prefix(keyword)
            && tail.starts_with(char::is_whitespace)
        {
            raw = tail.trim_start();
        }
    }
    let (callee, arguments) = raw.split_once('(')?;
    let callee = callee.trim_end();
    let mut chars = callee.bytes();
    if !chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == b'_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'.' | b'-'))
    {
        return None;
    }
    let (name, custom) = resolve(callee)?;
    let mut decoder = serde_json::Deserializer::from_str(arguments).into_iter::<Value>();
    let literal = decoder.next()?.ok()?;
    let tail = arguments[decoder.byte_offset()..]
        .trim()
        .strip_prefix(')')?
        .trim();
    if !matches!(tail, "" | ";") {
        return None;
    }
    // The callee selects the tool; fields named name/arguments are ordinary payload.
    match (custom, literal) {
        (true, Value::String(input)) => Some(json!({"name":name,"input":input})),
        (false, Value::Object(args)) => Some(json!({"name":name,"arguments":args})),
        _ => None,
    }
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
