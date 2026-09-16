use serde_json::{Map, Value, json};

use super::{ChatConversionError as Error, Result, messages, nonempty, object, only_keys, tools};

/// A Responses request plus downstream-only Chat delivery options.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedChatRequest {
    pub responses: Map<String, Value>,
    pub stream: bool,
    pub include_usage: bool,
}

pub fn decode_chat_request(value: Value) -> Result<DecodedChatRequest> {
    let source = object(&value, "request")?;
    only_keys(
        source,
        &[
            "model",
            "messages",
            "stream",
            "stream_options",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
            "reasoning_effort",
            "response_format",
            "text",
            "verbosity",
            "max_tokens",
            "max_completion_tokens",
            "temperature",
            "top_p",
            "metadata",
            "store",
            "service_tier",
            "user",
            "safety_identifier",
            "prompt_cache_key",
            "prompt_cache_retention",
            "use_websocket",
            "web_search_options",
            "n",
            "audio",
            "modalities",
            "logprobs",
            "top_logprobs",
            "logit_bias",
            "presence_penalty",
            "frequency_penalty",
            "stop",
            "seed",
            "prediction",
            "functions",
            "function_call",
            "top_k",
        ],
        "",
    )?;
    let model = nonempty(&value["model"], "model")?;
    reject_lossy(source)?;
    let stream = boolean(source, "stream")?.unwrap_or(false);
    let include_usage = match source.get("stream_options").filter(|v| !v.is_null()) {
        Some(options) => {
            let options = object(options, "stream_options")?;
            only_keys(options, &["include_usage"], "stream_options")?;
            if !stream {
                return Err(Error::invalid("stream_options"));
            }
            boolean(options, "include_usage")?.unwrap_or(false)
        }
        None => false,
    };
    let mut responses = Map::new();
    responses.insert("model".into(), json!(model));
    responses.insert("stream".into(), json!(stream));
    let custom_names = tools::convert(source, &mut responses)?;
    responses.insert(
        "input".into(),
        messages::convert(&value["messages"], &custom_names)?,
    );
    for key in ["parallel_tool_calls", "store", "use_websocket"] {
        if let Some(flag) = boolean(source, key)? {
            responses.insert(key.into(), json!(flag));
        }
    }
    for (key, min, max) in [("temperature", 0.0, 2.0), ("top_p", 0.0, 1.0)] {
        if let Some(v) = source.get(key).filter(|v| !v.is_null()) {
            if !v
                .as_f64()
                .is_some_and(|n| n.is_finite() && n >= min && n <= max)
            {
                return Err(Error::invalid(key));
            }
            responses.insert(key.into(), v.clone());
        }
    }
    for key in [
        "service_tier",
        "user",
        "safety_identifier",
        "prompt_cache_key",
        "prompt_cache_retention",
    ] {
        if let Some(v) = source.get(key).filter(|v| !v.is_null()) {
            nonempty(v, key)?;
            responses.insert(key.into(), v.clone());
        }
    }
    if let Some(metadata) = source.get("metadata").filter(|v| !v.is_null()) {
        let entries = object(metadata, "metadata")?;
        if entries.values().any(|v| !v.is_string()) {
            return Err(Error::invalid("metadata"));
        }
        responses.insert("metadata".into(), metadata.clone());
    }
    let mut limit = None;
    for key in ["max_tokens", "max_completion_tokens"] {
        if let Some(v) = source.get(key).filter(|v| !v.is_null()) {
            if limit.is_some() || !v.as_u64().is_some_and(|v| v > 0) {
                return Err(Error::invalid(key));
            }
            limit = Some(v.clone());
        }
    }
    if let Some(limit) = limit {
        responses.insert("max_output_tokens".into(), limit);
    }
    if let Some(effort) = source.get("reasoning_effort").filter(|v| !v.is_null()) {
        if !effort
            .as_str()
            .is_some_and(|s| ["none", "minimal", "low", "medium", "high", "xhigh"].contains(&s))
        {
            return Err(Error::invalid("reasoning_effort"));
        }
        responses.insert("reasoning".into(), json!({"effort": effort}));
    }
    text_options(source, &mut responses)?;
    Ok(DecodedChatRequest {
        responses,
        stream,
        include_usage,
    })
}

fn boolean(source: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    source
        .get(key)
        .filter(|v| !v.is_null())
        .map(|v| v.as_bool().ok_or_else(|| Error::invalid(key)))
        .transpose()
}

fn reject_lossy(source: &Map<String, Value>) -> Result<()> {
    for key in [
        "audio",
        "seed",
        "prediction",
        "functions",
        "function_call",
        "top_k",
        "stop",
        "logit_bias",
    ] {
        if source.get(key).is_some_and(|v| !v.is_null()) {
            return Err(Error::unsupported(key));
        }
    }
    for key in ["presence_penalty", "frequency_penalty", "top_logprobs"] {
        if let Some(v) = source.get(key).filter(|v| !v.is_null())
            && v.as_f64() != Some(0.0)
        {
            return Err(Error::unsupported(key));
        }
    }
    if let Some(v) = source.get("n").filter(|v| !v.is_null())
        && v.as_u64() != Some(1)
    {
        return Err(Error::unsupported("n"));
    }
    if let Some(v) = source.get("logprobs").filter(|v| !v.is_null())
        && v.as_bool() != Some(false)
    {
        return Err(Error::unsupported("logprobs"));
    }
    if let Some(v) = source.get("modalities").filter(|v| !v.is_null())
        && v != &json!(["text"])
    {
        return Err(Error::unsupported("modalities"));
    }
    Ok(())
}

fn text_options(source: &Map<String, Value>, out: &mut Map<String, Value>) -> Result<()> {
    let mut text = match source.get("text").filter(|v| !v.is_null()) {
        Some(v) => {
            let text = object(v, "text")?;
            only_keys(text, &["format", "verbosity"], "text")?;
            text.clone()
        }
        None => Map::new(),
    };
    if let Some(format) = source.get("response_format").filter(|v| !v.is_null()) {
        if text.contains_key("format") {
            return Err(Error::invalid("response_format"));
        }
        let raw = object(format, "response_format")?;
        only_keys(raw, &["type", "json_schema"], "response_format")?;
        let converted = if format["type"] == "json_schema" {
            let mut schema = object(&format["json_schema"], "response_format.json_schema")?.clone();
            schema.insert("type".into(), json!("json_schema"));
            Value::Object(schema)
        } else {
            if raw.contains_key("json_schema") {
                return Err(Error::invalid("response_format.json_schema"));
            }
            format.clone()
        };
        validate_format(&converted)?;
        text.insert("format".into(), converted);
    } else if let Some(format) = text.get("format") {
        validate_format(format)?;
    }
    if let Some(verbosity) = source.get("verbosity").filter(|v| !v.is_null()) {
        if text.contains_key("verbosity") {
            return Err(Error::invalid("verbosity"));
        }
        text.insert("verbosity".into(), verbosity.clone());
    }
    if let Some(verbosity) = text.get("verbosity")
        && !verbosity
            .as_str()
            .is_some_and(|v| ["low", "medium", "high"].contains(&v))
    {
        return Err(Error::invalid("text.verbosity"));
    }
    if !text.is_empty() {
        out.insert("text".into(), Value::Object(text));
    }
    Ok(())
}

fn validate_format(value: &Value) -> Result<()> {
    let format = object(value, "text.format")?;
    match value["type"].as_str() {
        Some("text" | "json_object") => only_keys(format, &["type"], "text.format"),
        Some("json_schema") => {
            only_keys(
                format,
                &["type", "name", "schema", "strict", "description"],
                "text.format",
            )?;
            nonempty(&value["name"], "text.format.name")?;
            object(&value["schema"], "text.format.schema")?;
            boolean(format, "strict")?;
            if let Some(description) = format.get("description") {
                super::string(description, "text.format.description")?;
            }
            Ok(())
        }
        _ => Err(Error::unsupported("text.format")),
    }
}
