use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{ClientTools, StructuredOutput};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum ExcelRequestError {
    #[error("invalid model identifier for the Excel upstream")]
    Model,
    #[error("Excel supports only low, medium, high and xhigh reasoning effort")]
    Effort,
    #[error("Excel does not support non-generating WebSocket warmup")]
    Warmup,
    #[error("Excel previous response history is unavailable; resend complete input")]
    History,
    #[error(
        "Excel tool history is incomplete (path=input[{input}]; type={kind}); resend the matching complete tool call with its result"
    )]
    HistoryInput { input: usize, kind: &'static str },
    #[error("unsupported or malformed Excel request input")]
    Input,
    #[error("unsupported Excel content (path=input[{input}].{field}[{part}]; type={kind})")]
    Content {
        input: usize,
        field: &'static str,
        part: usize,
        kind: &'static str,
    },
    #[error(
        "Excel cannot forward encrypted message parts (path=input[{input}].{field}[{part}]; type=encrypted_content); resend plaintext from its source or start a new conversation"
    )]
    EncryptedContent {
        input: usize,
        field: &'static str,
        part: usize,
    },
    #[error("unsupported client tool or tool choice for Excel")]
    Tool,
    #[error("Excel tool catalog changed concurrently; retry this request")]
    CatalogConflict,
    #[error("Excel returned an undeclared or malformed client tool call")]
    ToolCall,
    #[error("Excel returned a tool outside the client's catalog")]
    UnknownTool,
    #[error("unsupported or invalid Excel structured output format")]
    Format,
    #[error("Excel image relay is unavailable or full")]
    ImageRelay,
    #[error("Excel image input: {0}")]
    ImageInput(&'static str),
    #[error(
        "unsupported Excel image-tool request; use PNG, gpt-image-2 and supported image options"
    )]
    Image,
}

pub(crate) fn prepare_request(
    source: &Map<String, Value>,
    tools: &ClientTools,
    native_calls: &BTreeMap<String, Value>,
    structured: Option<&StructuredOutput>,
) -> Result<Map<String, Value>, ExcelRequestError> {
    let model = source
        .get("model")
        .and_then(Value::as_str)
        .ok_or(ExcelRequestError::Model)?;
    gateway_core::account::ExcelModels::try_from(vec![model.to_owned()])
        .map_err(|_| ExcelRequestError::Model)?;
    if source.get("generate") == Some(&Value::Bool(false)) {
        return Err(ExcelRequestError::Warmup);
    }
    if source
        .get("previous_response_id")
        .is_some_and(|value| !value.is_null())
    {
        return Err(ExcelRequestError::History);
    }
    let (_, effort) = reasoning_effort(source)?;
    let raw = source.get("input").ok_or(ExcelRequestError::Input)?;
    let history = translate_input(raw, native_calls)?;
    let root = stable_hash(history.first().unwrap_or(&Value::Null));
    let cache_key = [
        "prompt_cache_key",
        "promptCacheKey",
        "session_id",
        "sessionId",
    ]
    .iter()
    .find_map(|name| source.get(*name).and_then(Value::as_str))
    .filter(|value| !value.trim().is_empty())
    .unwrap_or(&root);
    let (turn, _) = turn_identity(&history);
    // Rebuilt native calls may separate output-only items on the wire; count client rounds.
    let (_, iteration) = turn_identity(raw.as_array().map(Vec::as_slice).unwrap_or(&history));
    let mut input = Vec::with_capacity(history.len() + 3);
    if let Some(instructions) = source.get("instructions").and_then(Value::as_str)
        && !instructions.trim().is_empty()
    {
        input.push(message("developer", instructions));
    }
    let mut protocol = tools.instructions();
    if let Some(format) = structured {
        protocol.push('\n');
        protocol.push_str(&format.instructions());
    }
    input.push(message("developer", &protocol));
    input.extend(history);
    let mut metadata = Map::new();
    if let Some(values) = source.get("metadata").and_then(Value::as_object) {
        for (key, value) in values {
            if value.is_string() || value.is_number() || value.is_boolean() {
                let text = value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string());
                metadata.insert(
                    key.chars().take(64).collect(),
                    text.chars().take(512).collect::<String>().into(),
                );
            }
        }
    }
    // Caller metadata cannot replace the account/client-scoped transport identity.
    metadata.insert("task_id".into(), stable_id("task", cache_key).into());
    metadata.insert(
        "turn_id".into(),
        stable_id("turn", &format!("{cache_key}/{turn}")).into(),
    );
    metadata.insert("agent_iteration".into(), iteration.to_string().into());
    let mut result = json!({
        "model": model,
        "model_selection": "explicit",
        "stream": true,
        "store": false,
        "input": input,
        "reasoning_effort": effort,
        "context_management": source.get("context_management").filter(|value| value.is_array())
            .cloned().unwrap_or_else(|| json!([{"type": "compaction", "compact_threshold": 920000}])),
        "metadata": metadata,
    }).as_object().expect("object literal").clone();
    if source.contains_key("prompt_cache_key") {
        result.insert("prompt_cache_key".into(), cache_key.into());
    }
    Ok(result)
}

pub(crate) fn reasoning_effort(
    source: &Map<String, Value>,
) -> Result<(String, &'static str), ExcelRequestError> {
    if source
        .get("reasoning")
        .and_then(|value| value.get("mode"))
        .is_some_and(|value| !value.is_null() && value != "" && value != "standard")
    {
        return Err(ExcelRequestError::Effort);
    }
    let value = source
        .get("reasoning")
        .and_then(|value| value.get("effort"))
        .or_else(|| source.get("reasoning_effort"));
    let requested = match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.trim().to_ascii_lowercase(),
        _ => return Err(ExcelRequestError::Effort),
    };
    let effective = match requested.as_str() {
        "" | "medium" => "medium",
        "none" | "minimal" | "low" => "low",
        "high" => "high",
        "xhigh" | "x-high" | "extra-high" | "extra_high" | "max" | "ultra" => "xhigh",
        _ => return Err(ExcelRequestError::Effort),
    };
    Ok((requested, effective))
}

pub(crate) fn message(role: &str, text: &str) -> Value {
    json!({
        "type": "message", "role": role,
        "content": [{"type": if role == "assistant" {"output_text"} else {"input_text"}, "text": text}]
    })
}

fn translate_input(
    raw: &Value,
    native_calls: &BTreeMap<String, Value>,
) -> Result<Vec<Value>, ExcelRequestError> {
    if let Some(text) = raw.as_str() {
        return Ok(vec![message("user", text)]);
    }
    let items = raw.as_array().ok_or(ExcelRequestError::Input)?;
    let mut output = Vec::with_capacity(items.len());
    let mut origins = BTreeMap::new();
    let mut completed = BTreeSet::new();
    let mut trigger = None;
    for (index, value) in items.iter().enumerate() {
        let mut item = value.as_object().cloned().ok_or(ExcelRequestError::Input)?;
        item.remove("internal_chat_message_metadata_passthrough");
        validate_content(item.get("content"), index, "content")?;
        match item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message")
        {
            "additional_tools" => continue,
            "configuration_update" => return Err(ExcelRequestError::Input),
            "compaction_trigger" => {
                trigger = Some(item.into());
            }
            "function_call" | "custom_tool_call" => {
                let id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .ok_or(ExcelRequestError::Input)?;
                let native = native_calls.get(id).ok_or(ExcelRequestError::History)?;
                if origins.contains_key(id) || completed.contains(id) {
                    return Err(ExcelRequestError::History);
                }
                origins.insert(
                    id.to_owned(),
                    native
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                );
                output.push(native.clone());
            }
            "function_call_output" | "custom_tool_call_output" => {
                let id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .ok_or(ExcelRequestError::Input)?
                    .to_owned();
                if !completed.insert(id.clone()) {
                    return Err(ExcelRequestError::History);
                }
                let origin = origins
                    .get(&id)
                    .map(String::as_str)
                    .or_else(|| native_calls.get(&id)?.get("name")?.as_str())
                    .ok_or(ExcelRequestError::History)?;
                if !origins.contains_key(&id) {
                    output.push(
                        native_calls
                            .get(&id)
                            .cloned()
                            .ok_or(ExcelRequestError::History)?,
                    );
                }
                if matches!(origin, "update_plan" | "functions.update_plan") {
                    item.insert("output".into(), "{\"status\":\"ok\"}".into());
                }
                validate_content(item.get("output"), index, "output")?;
                item.insert("type".into(), "function_call_output".into());
                item.insert("id".into(), function_item_id(&id).into());
                if item.get("output").is_none_or(|value| {
                    value.is_null() || value.as_str().is_some_and(|s| s.trim().is_empty())
                }) {
                    item.insert(
                        "output".into(),
                        "(tool call succeeded with no output)".into(),
                    );
                }
                let tool_images = separate_tool_images(&mut item);
                output.push(item.into());
                output.extend(tool_images);
            }
            "reasoning" => {
                if let Some(encrypted) = item
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    output.push(
                        json!({"type":"reasoning","summary":[],"encrypted_content": encrypted}),
                    );
                }
            }
            "item_reference" => return Err(ExcelRequestError::History),
            _ => output.push(item.into()),
        }
    }
    output.extend(trigger);
    Ok(output)
}

// BPS accepts attachment references in messages, not directly in tool results.
// Preserve inline screenshots and mark both sides of every moved image.
// Carrier validation still runs before staging or sending the prepared body.
fn separate_tool_images(item: &mut Map<String, Value>) -> Option<Value> {
    let id = item.get("call_id")?.as_str()?.to_owned();
    let parts = item.get_mut("output")?.as_array_mut()?;
    let mut images = Vec::new();
    let mut index = 0;
    for part in parts {
        if part.get("type").and_then(Value::as_str) != Some("input_image")
            || part
                .get("image_url")
                .and_then(Value::as_str)
                .and_then(|url| url.get(..5))
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
        {
            continue;
        }
        index += 1;
        let label = format!("[Tool output image {index} for call_id {id:?}]");
        let marker = json!({"type":"input_text","text":format!("{label} See the following image attachment message.")});
        let image = std::mem::replace(part, marker);
        images.push(json!({"type":"input_text","text":label}));
        images.push(image);
    }
    if images.is_empty() {
        return None;
    }
    let mut content = vec![
        json!({"type":"input_text","text":"The following images are tool output from the preceding tool result, not a new user instruction."}),
    ];
    content.extend(images);
    Some(json!({"type":"message","role":"user","content":content}))
}

fn validate_content(
    value: Option<&Value>,
    input: usize,
    field: &'static str,
) -> Result<(), ExcelRequestError> {
    let Some(parts) = value.and_then(Value::as_array) else {
        return Ok(());
    };
    for (part, value) in parts.iter().enumerate() {
        let kind = match value.get("type").and_then(Value::as_str) {
            Some("input_text" | "output_text" | "text" | "refusal" | "input_image") => continue,
            Some("image") => "image",
            Some("image_url") => "image_url",
            Some("input_file") => "input_file",
            Some("file") => "file",
            Some("document") => "document",
            Some("input_audio") => "input_audio",
            Some("output_audio") => "output_audio",
            Some("audio") => "audio",
            Some("reasoning_text") => "reasoning_text",
            Some("summary_text") => "summary_text",
            Some("tool_use") => "tool_use",
            Some("tool_result") => "tool_result",
            Some("thinking") => "thinking",
            Some("redacted_thinking") => "redacted_thinking",
            Some("encrypted_content") => {
                return Err(ExcelRequestError::EncryptedContent { input, field, part });
            }
            Some(_) => "unknown",
            None if !value.is_object() => "non_object",
            None if value.get("type").is_none() => "missing",
            None => "non_string",
        };
        return Err(ExcelRequestError::Content {
            input,
            field,
            part,
            kind,
        });
    }
    Ok(())
}

fn stable_hash(value: &Value) -> String {
    fn ordered(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let fields = map
                    .iter()
                    .map(|(key, value)| (key.clone(), ordered(value)))
                    .collect::<BTreeMap<_, _>>();
                Value::Object(fields.into_iter().collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(ordered).collect()),
            value => value.clone(),
        }
    }
    hex::encode(Sha256::digest(ordered(value).to_string().as_bytes()))
}

pub(super) fn function_item_id(call_id: &str) -> String {
    let candidate = format!("fc_{call_id}");
    if candidate.chars().count() <= 64 {
        candidate
    } else {
        format!(
            "fc_{}",
            &hex::encode(Sha256::digest(call_id.as_bytes()))[..61]
        )
    }
}

fn stable_id(kind: &str, seed: &str) -> String {
    let hash = Sha256::digest(format!("cpr/excel/v1/{kind}/{seed}").as_bytes());
    let mut bytes: [u8; 16] = hash[..16].try_into().expect("fixed length");
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn turn_identity(input: &[Value]) -> (String, usize) {
    let user_end = input
        .iter()
        .rposition(|item| item.get("role").and_then(Value::as_str) == Some("user"))
        .map_or(usize::from(!input.is_empty()), |index| index + 1);
    let mut rounds = 0;
    let mut in_results = false;
    for item in input.iter().skip(
        input
            .iter()
            .rposition(|item| item.get("role").and_then(Value::as_str) == Some("user"))
            .map_or(0, |index| index + 1),
    ) {
        let is_result = matches!(
            item.get("type").and_then(Value::as_str),
            Some("function_call_output" | "custom_tool_call_output")
        );
        rounds += usize::from(is_result && !in_results);
        in_results = is_result;
    }
    (
        stable_hash(&Value::Array(input[..user_end].to_vec())),
        rounds + 1,
    )
}

#[cfg(test)]
mod tests {
    use super::super::tests::VERIFIED_MODEL;
    use super::*;

    fn prepare(value: Value) -> Result<Map<String, Value>, ExcelRequestError> {
        let source = value.as_object().unwrap();
        prepare_request(
            source,
            &ClientTools::parse(source)?,
            &BTreeMap::new(),
            StructuredOutput::parse(source)?.as_ref(),
        )
    }

    fn image_history(kind: &str, output: Value) -> (Value, BTreeMap<String, Value>) {
        let prefix = if kind == "custom" {
            "custom_tool"
        } else {
            "function"
        };
        let call = json!({"type":format!("{prefix}_call"),"name":"view_image","call_id":"call_image",
            "arguments":"{}","input":"fixture.png"});
        let tools = ClientTools::parse(
            json!({"tools":[{"type":kind,"name":"view_image"}]})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let native = tools.rebuild_history_call(&call).unwrap();
        (
            json!({"model":VERIFIED_MODEL,"tools":tools.catalog(),"input":[call,
            {"type":format!("{prefix}_call_output"),"id":"ctco_fixture","call_id":"call_image","output":output}]}),
            BTreeMap::from([("call_image".into(), native)]),
        )
    }

    #[test]
    fn tool_image_references_preserve_association_order_detail_and_replay() {
        for kind in ["function", "custom"] {
            for carrier in ["file_id", "image_url"] {
                for detail in [
                    Value::Null,
                    json!("auto"),
                    json!("low"),
                    json!("high"),
                    json!("original"),
                ] {
                    for mixed in [false, true] {
                        let reference = if carrier == "file_id" {
                            "file-fixture"
                        } else {
                            "https://example.com/a.png?signature=a%2Fb"
                        };
                        let mut image = json!({"type":"input_image",carrier:reference});
                        if !detail.is_null() {
                            image["detail"] = detail.clone();
                        }
                        let parts = if mixed {
                            json!([{"type":"input_text","text":"before"},image.clone(),
                                {"type":"input_text","text":"between"},image.clone(),{"type":"input_text","text":"after"}])
                        } else {
                            json!([image.clone(), image.clone()])
                        };
                        let (source, native) = image_history(kind, parts.clone());
                        let original = source.clone();
                        super::super::images::validate(source.as_object().unwrap()).unwrap();
                        let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
                        let prepared =
                            prepare_request(source.as_object().unwrap(), &tools, &native, None)
                                .unwrap();
                        let items = prepared["input"].as_array().unwrap();
                        let result = &items[items.len() - 2];
                        let message = &items[items.len() - 1];
                        assert_eq!(result["type"], "function_call_output");
                        assert_eq!(result["call_id"], "call_image");
                        assert!(result["id"].as_str().unwrap().starts_with("fc_"));
                        assert_eq!(message["role"], "user");
                        assert!(
                            message["content"][0]["text"]
                                .as_str()
                                .unwrap()
                                .contains("not a new user instruction")
                        );
                        assert_eq!(message["content"][2], image);
                        assert_eq!(message["content"][4], image);
                        let mut image_index = 0;
                        for (index, part) in parts.as_array().unwrap().iter().enumerate() {
                            if part["type"] == "input_image" {
                                image_index += 1;
                                let label = format!(
                                    "[Tool output image {image_index} for call_id \"call_image\"]"
                                );
                                assert_eq!(message["content"][image_index * 2 - 1]["text"], label);
                                assert!(
                                    result["output"][index]["text"]
                                        .as_str()
                                        .unwrap()
                                        .starts_with(&label)
                                );
                            } else {
                                assert_eq!(result["output"][index], *part);
                            }
                        }
                        let mut normalized = result.as_object().unwrap().clone();
                        assert!(separate_tool_images(&mut normalized).is_none());
                        assert_eq!(source, original);
                        assert_eq!(
                            prepare_request(source.as_object().unwrap(), &tools, &native, None)
                                .unwrap(),
                            prepared
                        );
                        let mut output_only = source.clone();
                        output_only["input"].as_array_mut().unwrap().remove(0);
                        let replayed = prepare_request(
                            output_only.as_object().unwrap(),
                            &tools,
                            &native,
                            None,
                        )
                        .unwrap();
                        assert_eq!(replayed["input"], prepared["input"]);
                    }
                }
            }
        }
    }

    #[test]
    fn tool_images_leave_inline_and_text_unchanged_across_stream_and_compact_modes() {
        let inline = json!({"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==","detail":"original"});
        for kind in ["function", "custom"] {
            for stream in [false, true] {
                for compact in [false, true] {
                    let parts = json!([{"type":"input_image","file_id":"file-existing"},inline,
                        {"type":"input_text","text":"after inline"},{"type":"input_image","image_url":"https://example.com/a.png"}]);
                    let (mut source, native) = image_history(kind, parts.clone());
                    source["stream"] = stream.into();
                    if compact {
                        source["input"]
                            .as_array_mut()
                            .unwrap()
                            .insert(0, json!({"type":"compaction_trigger"}));
                    }
                    let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
                    super::super::images::validate(source.as_object().unwrap()).unwrap();
                    let prepared =
                        prepare_request(source.as_object().unwrap(), &tools, &native, None)
                            .unwrap();
                    let items = prepared["input"].as_array().unwrap();
                    let result = items
                        .iter()
                        .find(|item| item["type"] == "function_call_output")
                        .unwrap();
                    assert_eq!(result["output"][1], inline);
                    assert_eq!(result["output"][2], parts[2]);
                    let message = items.iter().find(|item| item["role"] == "user").unwrap();
                    assert_eq!(message["content"][2], parts[0]);
                    assert_eq!(message["content"][4], parts[3]);
                    assert!(!super::super::images::has_user_inline(&prepared));
                    if compact {
                        assert_eq!(items.last().unwrap()["type"], "compaction_trigger");
                    }
                }
            }
        }
        for output in [
            json!("plain text"),
            json!([]),
            json!([{"type":"input_text","text":"literal input_image"}]),
            json!([inline]),
        ] {
            let (source, native) = image_history("function", output.clone());
            let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
            let prepared =
                prepare_request(source.as_object().unwrap(), &tools, &native, None).unwrap();
            let result = prepared["input"].as_array().unwrap().last().unwrap();
            assert_eq!(result["type"], "function_call_output");
            assert_eq!(result["output"], output);
        }
    }

    #[test]
    fn multiple_tool_results_keep_their_own_adjacent_image_message() {
        let (source, mut native) = image_history(
            "function",
            json!([{"type":"input_image","file_id":"file-first"}]),
        );
        let mut first = source["input"][1].clone();
        first["call_id"] = "call_first".into();
        let mut second = first.clone();
        second["call_id"] = "call_second".into();
        second["output"][0]["file_id"] = "file-second".into();
        let template = native.remove("call_image").unwrap();
        for id in ["call_first", "call_second"] {
            let mut call = template.clone();
            call["call_id"] = id.into();
            native.insert(id.into(), call);
        }
        let translated = translate_input(
            &json!([second, first, message("user", "Continue")]),
            &native,
        )
        .unwrap();
        for (result_index, id, file) in [
            (1, "call_second", "file-second"),
            (4, "call_first", "file-first"),
        ] {
            assert_eq!(translated[result_index]["call_id"], id);
            assert!(
                translated[result_index + 1]["content"][1]["text"]
                    .as_str()
                    .unwrap()
                    .contains(id)
            );
            assert_eq!(translated[result_index + 1]["content"][2]["file_id"], file);
        }
        assert_eq!(translated.last().unwrap(), &message("user", "Continue"));
    }

    #[test]
    fn explicit_streaming_body_has_no_codex_control_fields() {
        let output = prepare(
            json!({"model": VERIFIED_MODEL, "input":"hello", "stream":false,
            "client_metadata":{"x-codex-turn-state":"untrusted"}, "instructions":"Be brief."}),
        )
        .unwrap();
        assert_eq!(output["stream"], true);
        assert_eq!(output["store"], false);
        assert_eq!(output["model_selection"], "explicit");
        assert!(output.get("client_metadata").is_none());
        assert_eq!(output["input"][0]["role"], "developer");
        assert_eq!(output["input"][2]["content"][0]["text"], "hello");
    }

    #[test]
    fn compact_tool_choice_is_consumed_locally() {
        let output = prepare(json!({
            "model": VERIFIED_MODEL,
            "input": [{"role":"user","content":"Remember 521."},{"type":"compaction_trigger"}],
            "tools": [{"type":"function","name":"unused","parameters":{"type":"object"}}],
            "tool_choice": "none"
        }))
        .unwrap();
        assert!(!output.contains_key("tool_choice"));
        assert!(!output.contains_key("tools"));
        assert_eq!(
            output["input"].as_array().unwrap().last().unwrap(),
            &json!({"type":"compaction_trigger"})
        );
        assert!(!output["input"][0].to_string().contains("unused"));
    }

    #[test]
    fn unsupported_effort_model_warmup_and_history_fail_before_send() {
        for (field, value, expected) in [
            (
                "reasoning_effort",
                json!("unknown"),
                ExcelRequestError::Effort,
            ),
            ("model", json!("invalid model"), ExcelRequestError::Model),
            ("generate", json!(false), ExcelRequestError::Warmup),
            (
                "previous_response_id",
                json!("resp_unknown"),
                ExcelRequestError::History,
            ),
        ] {
            let mut source = json!({"model":VERIFIED_MODEL,"input":"hello"});
            source[field] = value;
            assert_eq!(prepare(source), Err(expected));
        }
    }

    #[test]
    fn stable_task_and_turn_are_not_changed_by_tool_iteration() {
        let first = json!({"type":"message", "role":"user", "content":"hello"});
        let (turn, iteration) = turn_identity(std::slice::from_ref(&first));
        let (continued_turn, continued_iteration) = turn_identity(&[
            first,
            json!({"type":"function_call_output","call_id":"call_fixture","output":"ok"}),
        ]);
        assert_eq!(turn, continued_turn);
        assert_eq!((iteration, continued_iteration), (1, 2));
    }

    #[test]
    fn parallel_results_count_as_one_iteration_per_round() {
        let user = message("user", "read");
        let result = json!({"type":"function_call_output","call_id":"a","output":"ok"});
        let custom = json!({"type":"custom_tool_call_output","call_id":"b","output":"ok"});
        assert_eq!(turn_identity(&[result.clone(), custom.clone()]).1, 2);
        assert_eq!(turn_identity(&[user.clone(), result.clone(), custom]).1, 2);
        assert_eq!(
            turn_identity(&[
                user.clone(),
                result.clone(),
                message("assistant", "next"),
                result.clone()
            ])
            .1,
            3
        );
        assert_eq!(turn_identity(&[result, user]).1, 1);
    }
    #[test]
    fn encrypted_message_content_has_specific_safe_diagnostic() {
        let error = validate_content(
            Some(&json!([{
                "type":"encrypted_content","encrypted_content":"PRIVATE_CIPHERTEXT"
            }])),
            3,
            "content",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("input[3].content[0]"));
        assert!(error.contains("type=encrypted_content"));
        assert!(!error.contains("PRIVATE_CIPHERTEXT"));
    }
}
