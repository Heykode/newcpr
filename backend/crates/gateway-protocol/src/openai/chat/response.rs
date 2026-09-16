use std::borrow::Cow;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::openai::output_recovery::recover_response_output;

use super::{ChatConversionError as Error, Result};

#[derive(Debug)]
pub(super) struct Image<'a> {
    result: &'a str,
    mime: Cow<'a, str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ImageFingerprint {
    digest: [u8; 32],
}

impl Image<'_> {
    pub fn fingerprint(&self) -> ImageFingerprint {
        ImageFingerprint {
            digest: Sha256::digest(self.result.as_bytes()).into(),
        }
    }

    pub fn entry(&self, index: usize) -> Value {
        let prefix = format!("data:{};base64,", self.mime);
        let mut url = String::with_capacity(prefix.len() + self.result.len());
        url.push_str(&prefix);
        url.push_str(self.result);
        let mut entry = json!({"index":index,"type":"image_url","image_url":{"url":null}});
        entry["image_url"]["url"] = Value::String(url);
        entry
    }
}

/// Converts buffered wire events without materializing intermediate Chat chunks.
pub fn chat_response_from_events<'a>(
    events: impl IntoIterator<Item = (Option<&'a str>, &'a Value)>,
) -> Result<Value> {
    let mut terminal = None;
    let mut collected = Vec::new();
    for (header, data) in events {
        let kind = event_kind(header, data)?;
        collected.push((kind, data));
        match kind {
            "response.created" | "response.in_progress" | "response.queued" => {
                check_failure(&data["response"])?;
            }
            "response.completed" | "response.incomplete" => {
                check_failure(&data["response"])?;
                terminal = Some(&data["response"]);
            }
            _ => {}
        }
    }
    let mut response = terminal.ok_or_else(|| Error::output("status"))?.clone();
    recover_response_output(&mut response, collected);
    chat_response(&response)
}

pub fn chat_response(response: &Value) -> Result<Value> {
    finish_reason(response, false)?;
    let mut text = String::new();
    let mut refusal = String::new();
    let mut reasoning = String::new();
    let mut tools = Vec::new();
    let mut images = Vec::new();
    for item in response["output"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("message") => {
                for part in item["content"].as_array().into_iter().flatten() {
                    match part["type"].as_str() {
                        Some("output_text") => {
                            text.push_str(&string_value(&part["text"]));
                        }
                        Some("refusal") => {
                            refusal.push_str(&string_value(&part["refusal"]));
                        }
                        _ => {}
                    }
                }
            }
            Some("reasoning") => {
                for (field, kind) in [("summary", "summary_text"), ("content", "reasoning_text")] {
                    for part in item[field].as_array().into_iter().flatten() {
                        if part["type"] == kind {
                            reasoning.push_str(&string_value(&part["text"]));
                        }
                    }
                }
            }
            Some("function_call" | "custom_tool_call") => {
                let input_field = if item["type"] == "custom_tool_call" {
                    "input"
                } else {
                    "arguments"
                };
                tools.push(json!({
                    "id":string_value(&item["call_id"]),
                    "type":"function",
                    "function":{
                        "name":string_value(&item["name"]),
                        "arguments":string_value(&item[input_field])
                    }
                }));
            }
            Some("image_generation_call") => {
                let result = string_value(&item["result"]);
                if !result.is_empty() {
                    let image = parse_image_payload(
                        &result,
                        item.get("output_format"),
                        "output.image_generation_call",
                    )?;
                    images.push(image.entry(images.len()));
                }
            }
            _ => {}
        }
    }
    let finish = finish_reason(response, !tools.is_empty())?;
    let mut message = json!({"role":"assistant","content":null});
    if !text.is_empty() {
        message["content"] = Value::String(text);
    }
    if !refusal.is_empty() {
        message["refusal"] = json!(refusal);
    }
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    if !tools.is_empty() {
        message["tool_calls"] = json!(tools);
    }
    if !images.is_empty() {
        message["images"] = Value::Array(images);
    }
    let id = string_value(&response["id"]);
    let model = string_value(&response["model"]);
    let created = response["created_at"].as_u64().unwrap_or_default();
    let mut result = json!({
        "id":chat_id(&id), "object":"chat.completion", "created":created, "model":model,
        "choices":[{"index":0,"message":null,"finish_reason":finish,"logprobs":null}]
    });
    result["choices"][0]["message"] = message;
    if let Some(usage) = chat_usage(response)? {
        result["usage"] = usage;
    }
    for key in ["service_tier", "system_fingerprint"] {
        if let Some(value) = response.get(key) {
            result[key] = value.clone();
        }
    }
    Ok(result)
}

pub(super) fn chat_id(id: &str) -> String {
    format!("chatcmpl-{}", id.strip_prefix("resp_").unwrap_or(id))
}

pub(super) fn string_value(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(value) => Cow::Borrowed(value),
        Value::Null => Cow::Borrowed(""),
        value => Cow::Owned(value.to_string()),
    }
}

pub(super) fn event_kind<'a>(header: Option<&'a str>, data: &'a Value) -> Result<&'a str> {
    let body = data.get("type").and_then(Value::as_str);
    let is_failure = |kind| matches!(kind, "error" | "response.failed" | "response.cancelled");
    // Explicit failures cannot be hidden by a conflicting SSE header/body.
    if header.is_some_and(is_failure) || body.is_some_and(is_failure) {
        return Err(Error::failed());
    }
    Ok(body.or(header).unwrap_or_default())
}

pub(super) fn check_failure(response: &Value) -> Result<()> {
    if response.get("error").is_some_and(|value| !value.is_null())
        || matches!(response["status"].as_str(), Some("failed" | "cancelled"))
    {
        Err(Error::failed())
    } else {
        Ok(())
    }
}

pub(super) fn finish_reason(response: &Value, has_tools: bool) -> Result<&'static str> {
    check_failure(response)?;
    match response["status"].as_str() {
        Some("completed") => Ok(if has_tools { "tool_calls" } else { "stop" }),
        Some("incomplete") => match response
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
        {
            Some("max_tokens" | "max_output_tokens") => Ok("length"),
            Some("content_filter") => Ok("content_filter"),
            _ => Ok("stop"),
        },
        _ => Err(Error::output("status")),
    }
}

pub(super) fn parse_image_payload<'a>(
    result: &'a str,
    format: Option<&'a Value>,
    _path: &str,
) -> Result<Image<'a>> {
    let format = format.map(string_value).unwrap_or_default();
    let mime = if format.contains('/') {
        format
    } else if format.eq_ignore_ascii_case("jpg") || format.eq_ignore_ascii_case("jpeg") {
        Cow::Borrowed("image/jpeg")
    } else if format.eq_ignore_ascii_case("webp") {
        Cow::Borrowed("image/webp")
    } else if format.eq_ignore_ascii_case("gif") {
        Cow::Borrowed("image/gif")
    } else {
        Cow::Borrowed("image/png")
    };
    Ok(Image { result, mime })
}

pub(super) fn chat_usage(response: &Value) -> Result<Option<Value>> {
    let Some(source) = response.get("usage").and_then(Value::as_object) else {
        return Ok(None);
    };
    let mut usage = serde_json::Map::new();
    for (from, to) in [
        ("input_tokens", "prompt_tokens"),
        ("output_tokens", "completion_tokens"),
        ("total_tokens", "total_tokens"),
    ] {
        if let Some(value) = source.get(from).filter(|value| value.as_u64().is_some()) {
            usage.insert(to.into(), value.clone());
        }
    }
    if !usage.contains_key("total_tokens")
        && let (Some(input), Some(output)) = (
            source.get("input_tokens").and_then(Value::as_u64),
            source.get("output_tokens").and_then(Value::as_u64),
        )
        && let Some(total) = input.checked_add(output)
    {
        usage.insert("total_tokens".into(), json!(total));
    }
    for (from, to) in [
        ("input_tokens_details", "prompt_tokens_details"),
        ("output_tokens_details", "completion_tokens_details"),
    ] {
        if let Some(value) = source.get(from).and_then(Value::as_object) {
            let details: serde_json::Map<String, Value> = value
                .iter()
                .filter(|(_, value)| value.as_u64().is_some())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if !details.is_empty() {
                usage.insert(to.into(), Value::Object(details));
            }
        }
    }
    Ok(Some(Value::Object(usage)))
}
