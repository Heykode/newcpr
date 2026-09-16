use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::{ChatConversionError as Error, Result, nonempty, object, only_keys, string, tools};

pub(super) fn convert(value: &Value, custom_names: &BTreeSet<String>) -> Result<Value> {
    let messages = value
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Error::invalid("messages"))?;
    let mut input = Vec::new();
    let mut calls: BTreeMap<String, (String, &'static str)> = BTreeMap::new();
    let mut answered = BTreeSet::new();
    for (index, message) in messages.iter().enumerate() {
        let path = format!("messages[{index}]");
        let msg = object(message, &path)?;
        only_keys(
            msg,
            &[
                "role",
                "content",
                "tool_calls",
                "tool_call_id",
                "refusal",
                "name",
                "images",
            ],
            &path,
        )?;
        let role = string(&message["role"], &format!("{path}.role"))?;
        let has_images = if let Some(images) = msg.get("images") {
            if role != "assistant" {
                return Err(Error::invalid(format!("{path}.images")));
            }
            output_images(images, &format!("{path}.images"))?
        } else {
            false
        };
        if role == "tool" {
            if msg.contains_key("tool_calls") || msg.contains_key("refusal") {
                return Err(Error::invalid(&path));
            }
            let id = nonempty(&message["tool_call_id"], &format!("{path}.tool_call_id"))?;
            let (call_name, output_type) = calls
                .get(id)
                .ok_or_else(|| Error::invalid(format!("{path}.tool_call_id")))?;
            if !answered.insert(id.to_owned()) {
                return Err(Error::invalid(format!("{path}.tool_call_id")));
            }
            if let Some(name) = msg.get("name").filter(|v| !v.is_null())
                && call_name != string(name, &format!("{path}.name"))?
            {
                return Err(Error::invalid(format!("{path}.name")));
            }
            let output = match &message["content"] {
                Value::String(_) => message["content"].clone(),
                Value::Array(_) => Value::Array(content(&message["content"], "tool", &path)?),
                _ => return Err(Error::invalid(format!("{path}.content"))),
            };
            input.push(json!({"type":output_type,"call_id":id,"output":output}));
            continue;
        }
        if !["system", "developer", "user", "assistant"].contains(&role) {
            return Err(Error::unsupported(format!("{path}.role")));
        }
        if msg.get("name").is_some_and(|v| !v.is_null()) {
            return Err(Error::unsupported(format!("{path}.name")));
        }
        if msg.contains_key("tool_call_id")
            || (role != "assistant"
                && (msg.contains_key("tool_calls") || msg.contains_key("refusal")))
        {
            return Err(Error::invalid(&path));
        }
        let empty_image_content = has_images
            && (message["content"] == ""
                || message["content"].as_array().is_some_and(Vec::is_empty));
        let mut parts =
            if (message["content"].is_null() && role == "assistant") || empty_image_content {
                Vec::new()
            } else {
                content(&message["content"], role, &path)?
            };
        if let Some(refusal) = msg.get("refusal").filter(|v| !v.is_null()) {
            parts.push(
                json!({"type":"refusal","refusal":string(refusal, &format!("{path}.refusal"))?}),
            );
        }
        if !parts.is_empty() {
            let mut item = json!({"type":"message","role":role,"content":parts});
            if role == "assistant" {
                item["status"] = json!("completed");
            }
            input.push(item);
        }
        let mut has_calls = false;
        if let Some(tool_calls) = msg.get("tool_calls").filter(|v| !v.is_null()) {
            let tool_calls = tool_calls
                .as_array()
                .ok_or_else(|| Error::invalid(format!("{path}.tool_calls")))?;
            for (call_index, call) in tool_calls.iter().enumerate() {
                let cp = format!("{path}.tool_calls[{call_index}]");
                let raw = object(call, &cp)?;
                let id = nonempty(&call["id"], &format!("{cp}.id"))?;
                let (kind, payload) = match call["type"].as_str() {
                    Some("function") => {
                        only_keys(raw, &["id", "type", "function"], &cp)?;
                        ("function", "arguments")
                    }
                    Some("custom") => {
                        only_keys(raw, &["id", "type", "custom", "function"], &cp)?;
                        tools::empty_function(raw, &cp)?;
                        ("custom", "input")
                    }
                    _ => return Err(Error::unsupported(format!("{cp}.type"))),
                };
                let tool_path = format!("{cp}.{kind}");
                let tool = object(&call[kind], &tool_path)?;
                only_keys(tool, &["name", payload], &tool_path)?;
                let name = nonempty(&call[kind]["name"], &format!("{tool_path}.name"))?;
                // CPA's function envelope is reversible only with validated declarations.
                let (item_type, output_type, input_field) =
                    if kind == "custom" || custom_names.contains(name) {
                        ("custom_tool_call", "custom_tool_call_output", "input")
                    } else {
                        ("function_call", "function_call_output", "arguments")
                    };
                if calls
                    .insert(id.to_owned(), (name.to_owned(), output_type))
                    .is_some()
                {
                    return Err(Error::invalid(format!("{cp}.id")));
                }
                let value = string(&call[kind][payload], &format!("{tool_path}.{payload}"))?;
                let mut item = json!({"type":item_type,"call_id":id,"name":name});
                item[input_field] = json!(value);
                input.push(item);
                has_calls = true;
            }
        }
        if message["content"].is_null() && message["refusal"].is_null() && !has_calls && !has_images
        {
            return Err(Error::invalid(format!("{path}.content")));
        }
    }
    Ok(Value::Array(input))
}

// Validate output-only history without decoding, copying or reconstructing image inputs.
fn output_images(value: &Value, path: &str) -> Result<bool> {
    if value.is_null() {
        return Ok(false);
    }
    let images = value.as_array().ok_or_else(|| Error::invalid(path))?;
    let mut indices = BTreeSet::new();
    let indexed = images
        .first()
        .is_some_and(|image| image.get("index").is_some());
    for (index, image) in images.iter().enumerate() {
        let path = format!("{path}[{index}]");
        let raw = object(image, &path)?;
        only_keys(raw, &["type", "index", "image_url"], &path)?;
        if image["type"] != "image_url" {
            return Err(Error::unsupported(format!("{path}.type")));
        }
        if raw.contains_key("index") != indexed {
            return Err(Error::invalid(format!("{path}.index")));
        }
        if let Some(index) = raw.get("index") {
            let index = index
                .as_u64()
                .ok_or_else(|| Error::invalid(format!("{path}.index")))?;
            if !indices.insert(index) {
                return Err(Error::invalid(format!("{path}.index")));
            }
        }
        let url_path = format!("{path}.image_url");
        let url = object(&image["image_url"], &url_path)?;
        only_keys(url, &["url"], &url_path)?;
        nonempty(&image["image_url"]["url"], &format!("{url_path}.url"))?;
    }
    Ok(!images.is_empty())
}

fn content(value: &Value, role: &str, path: &str) -> Result<Vec<Value>> {
    let text_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    if let Some(text) = value.as_str() {
        return Ok(vec![json!({"type":text_type,"text":text})]);
    }
    let blocks = value
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Error::invalid(format!("{path}.content")))?;
    let mut result = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let bp = format!("{path}.content[{index}]");
        let raw = object(block, &bp)?;
        match block["type"].as_str() {
            Some("text") => {
                only_keys(raw, &["type", "text"], &bp)?;
                result.push(
                    json!({"type":text_type,"text":string(&block["text"], &format!("{bp}.text"))?}),
                );
            }
            Some("image_url") if role == "user" || role == "tool" => {
                only_keys(raw, &["type", "image_url"], &bp)?;
                let image = object(&block["image_url"], &format!("{bp}.image_url"))?;
                only_keys(
                    image,
                    &["url", "detail", "MimeType"],
                    &format!("{bp}.image_url"),
                )?;
                // NewAPI serializes this empty Go-only field into Messages image fixtures.
                if image
                    .get("MimeType")
                    .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
                {
                    return Err(Error::unsupported(format!("{bp}.image_url.MimeType")));
                }
                let url = nonempty(&block["image_url"]["url"], &format!("{bp}.image_url.url"))?;
                let mut converted = json!({"type":"input_image","image_url":url});
                if let Some(detail) = image.get("detail").filter(|v| !v.is_null()) {
                    if !detail
                        .as_str()
                        .is_some_and(|v| ["auto", "low", "high", "original"].contains(&v))
                    {
                        return Err(Error::invalid(format!("{bp}.image_url.detail")));
                    }
                    converted["detail"] = detail.clone();
                }
                result.push(converted);
            }
            Some("refusal") if role == "assistant" => {
                only_keys(raw, &["type", "refusal"], &bp)?;
                result.push(json!({"type":"refusal","refusal":string(&block["refusal"], &format!("{bp}.refusal"))?}));
            }
            _ => return Err(Error::unsupported(bp)),
        }
    }
    Ok(result)
}
