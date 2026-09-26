//! Bounded correction of unexecuted Excel tool envelopes, never of source text.

use std::collections::BTreeSet;

use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use gateway_protocol::openai::sse::{SseError, SseEventDecoder};
use serde_json::{Map, Value, json};

use super::{
    ClientTools, ExcelRequestError, envelope, tools::canonical_history_call,
    usage::ExcelUsagePolicy,
};
use crate::transport::{CodexBackendSseStream, CodexClientError};

pub(super) type Sender = Box<
    dyn FnMut(
            Map<String, Value>,
        ) -> BoxFuture<'static, Result<CodexBackendSseStream, CodexClientError>>
        + Send,
>;

const MAX_REPAIRS: usize = 2;
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

pub(super) fn invalid(message: &'static str) -> CodexClientError {
    CodexClientError::InvalidSse(SseError::ParseError(message.into()))
}

fn is_tool(item: &Value) -> bool {
    matches!(
        item["type"].as_str(),
        Some("function_call" | "custom_tool_call")
    )
}

pub(super) fn validate(tools: &ClientTools, response: &Value) -> Result<(), CodexClientError> {
    let items = response["output"]
        .as_array()
        .ok_or_else(|| invalid("Excel completion has no output"))?;
    tools.validate_completion(items).map_err(|_| {
        invalid("Excel output violated the requested tool choice or parallel-call limit")
    })?;
    let mut ids = BTreeSet::new();
    let mut calls = BTreeSet::new();
    for item in items.iter().filter(|item| is_tool(item)) {
        let converted = tools
            .convert_call(item)
            .map_err(|_| invalid("Excel returned an undeclared or malformed tool call"))?;
        if !ids.insert(converted["id"].clone().to_string())
            || !calls.insert(converted["call_id"].clone().to_string())
        {
            return Err(invalid("Excel completion duplicated a tool identity"));
        }
    }
    Ok(())
}

fn arguments(item: &Value) -> Option<Value> {
    envelope::json_value(item.get("arguments")?)
        .ok()
        .filter(Value::is_object)
}

fn marked_target(args: &Value) -> Option<&str> {
    let summary = args["summary"].as_str()?;
    [
        "cpr.custom/",
        "codex2api.custom/",
        envelope::FUNCTION_CODE_PREFIX,
        envelope::FUNCTION_CMD_PREFIX,
    ]
    .into_iter()
    .find_map(|prefix| summary.strip_prefix(prefix))
}

fn named_envelope(args: &Value) -> Option<Value> {
    if marked_target(args).is_some() {
        return None;
    }
    let value = envelope::json_value(args.get("code")?).ok()?;
    envelope::name(&value).ok()?;
    Some(value)
}

fn explicit_target(args: &Value) -> Option<String> {
    marked_target(args).map(str::to_owned).or_else(|| {
        named_envelope(args).and_then(|value| envelope::name(&value).ok().map(str::to_owned))
    })
}

fn batch(response: &Value) -> Option<Vec<Value>> {
    if response["status"] != "completed" {
        return None;
    }
    let mut calls = BTreeSet::new();
    let items: Vec<_> = response["output"]
        .as_array()?
        .iter()
        .filter(|item| is_tool(item))
        .cloned()
        .collect();
    if items.is_empty() || items.len() > 512 {
        return None;
    }
    for item in &items {
        if item["type"] != "function_call"
            || !matches!(
                item["name"].as_str(),
                Some("run_officejs" | "functions.run_officejs")
            )
            || !calls.insert(item["call_id"].as_str().filter(|id| !id.is_empty())?)
            || arguments(item).is_none()
        {
            return None;
        }
    }
    Some(items)
}

pub(super) fn eligible(tools: &ClientTools, response: &Value) -> bool {
    let Some(items) = batch(response) else {
        return false;
    };
    if tools.validate_call_count(items.len()).is_err() {
        return false;
    }
    items.iter().all(|item| {
        arguments(item)
            .and_then(|args| explicit_target(&args))
            .is_none_or(|name| tools.contains(&name))
    })
}

pub(super) fn no_tool_history(body: &Map<String, Value>) -> bool {
    body.get("input")
        .and_then(Value::as_array)
        .is_some_and(|input| {
            !input.iter().any(|item| {
                is_tool(item)
                    || matches!(
                        item["type"].as_str(),
                        Some("function_call_output" | "custom_tool_call_output")
                    )
            })
        })
}

pub(super) fn unknown_eligible(tools: &ClientTools, response: &Value) -> bool {
    let Some(calls) = batch(response).filter(|calls| calls.len() == 1) else {
        return false;
    };
    tools.has_client_tools()
        && tools.validate_call_count(1).is_ok()
        && response["output"].as_array().and_then(|items| items.last()) == calls.first()
        && calls[0]["id"].as_str().is_some_and(|id| !id.is_empty())
        && matches!(
            tools.convert_call(&calls[0]),
            Err(ExcelRequestError::UnknownTool)
        )
}

pub(super) fn correct_unknown<'a>(
    original: &'a Value,
    tools: &'a ClientTools,
    mut body: Map<String, Value>,
    sender: &'a mut Sender,
    usage_policy: &'a ExcelUsagePolicy,
) -> BoxStream<'a, Result<Option<Value>, CodexClientError>> {
    Box::pin(async_stream::try_stream! {
        if !no_tool_history(&body) || !unknown_eligible(tools, original) {
            Err(invalid("Excel unknown tool is not eligible for regeneration"))?;
        }
        let mut usage = json!({});
        add_usage(&mut usage, &original["usage"])?;
        usage_policy.record_repair_usage(original, &usage);
        yield None;
        let input = body.get_mut("input").and_then(Value::as_array_mut)
            .ok_or_else(|| invalid("Excel correction requires complete history"))?;
        let index = input.len() - usize::from(input.last().is_some_and(|item| item["type"] == "compaction_trigger"));
        input.insert(index, super::request::message("developer",
            "The previous response selected a tool absent from the current client catalog. \
             No client tool from that response was executed. Correct this once using exactly \
             one tool declared in the client catalog and its documented run_officejs transport. \
             Do not call executor-internal helpers as standalone tools. Preserve the original \
             task and use only the declared argument schema."));
        if serde_json::to_vec(&body).map_err(CodexClientError::RequestBodyEncode)?.len() > MAX_RESPONSE_BYTES {
            Err(invalid("Excel correction exceeds its request size limit"))?;
        }
        let mut stream = sender(body).await?;
        let mut reader = read_response(&mut stream, original, &usage, usage_policy);
        let mut event = None;
        while let Some(step) = reader.next().await {
            match step? {
                Some(terminal) => event = Some(terminal),
                None => yield None,
            }
        }
        drop(reader);
        drop(stream);
        let mut event = event.ok_or_else(|| invalid("Excel correction has no terminal response"))?;
        let corrected = &event["response"];
        add_usage(&mut usage, &corrected["usage"])?;
        usage_policy.record_repair_usage(original, &usage);
        yield None;
        if matches!(event["type"].as_str(), Some("response.failed" | "error")) {
            if event["response"].is_object() {
                event["response"]["id"] = original["id"].clone();
                event["response"]["output"] = json!([]);
                event["response"]["usage"] = usage;
            }
            yield Some(event);
            return;
        }
        if event["type"] != "response.completed" || corrected["status"] != "completed" {
            Err(invalid("Excel unknown tool regeneration did not complete"))?;
        }
        validate(tools, corrected)?;
        let replacement = corrected["output"].as_array().filter(|items| !items.is_empty())
            .ok_or_else(|| invalid("Excel unknown tool regeneration returned no output"))?;
        let mut result = original.clone();
        let output = result["output"].as_array_mut().expect("validated original output");
        output.pop();
        output.extend(replacement.iter().cloned());
        result["usage"] = usage;
        validate(tools, &result)?;
        yield Some(json!({"type":"response.completed","response":result}));
    })
}

pub(super) fn correct<'a>(
    original: &'a Value,
    tools: &'a ClientTools,
    mut body: Map<String, Value>,
    sender: &'a mut Sender,
    usage_policy: &'a ExcelUsagePolicy,
) -> BoxStream<'a, Result<Option<Value>, CodexClientError>> {
    Box::pin(async_stream::try_stream! {
    let original_calls = batch(original)
        .ok_or_else(|| invalid("Excel tool batch is not eligible for correction"))?;
    let mut result = original.clone();
    let mut failed = original.clone();
    let mut usage = json!({});
    add_usage(&mut usage, &original["usage"])?;
    usage_policy.record_repair_usage(original, &usage);
    // Let the existing Core metering owner observe known usage before any new await.
    yield None;
    for _ in 0..MAX_REPAIRS {
        extend_request(&mut body, &failed, original_calls.len())?;
        let mut stream = sender(body.clone()).await?;
        let mut reader = read_response(&mut stream, original, &usage, usage_policy);
        let mut event = None;
        while let Some(step) = reader.next().await {
            match step? {
                Some(terminal) => event = Some(terminal),
                None => yield None,
            }
        }
        drop(reader);
        drop(stream);
        let mut event = event.ok_or_else(|| invalid("Excel correction has no terminal response"))?;
        let corrected = &event["response"];
        add_usage(&mut usage, &corrected["usage"])?;
        usage_policy.record_repair_usage(original, &usage);
        yield None;
        if matches!(event["type"].as_str(), Some("response.failed" | "error")) {
            // Preserve SSE rejection identity; it is not an HTTP handshake rejection.
            if event["response"].is_object() {
                event["response"]["id"] = original["id"].clone();
                event["response"]["output"] = json!([]);
                event["response"]["usage"] = usage;
            }
            yield Some(event);
            return;
        }
        if event["type"] != "response.completed" {
            Err(invalid("Excel tool correction did not complete"))?;
        }
        let mut calls = batch(corrected)
            .filter(|calls| calls.len() == original_calls.len())
            .ok_or_else(|| {
                invalid("Excel tool correction changed the batch or did not complete")
            })?;
        if validate(tools, corrected).is_ok() {
            bind_original_code(tools, &original_calls, &mut calls)?;
            if !preserves_operations(tools, &original_calls, &calls) {
                Err(invalid(
                    "Excel tool correction changed an operation; no tool was executed",
                ))?;
            }
            let mut next = calls.into_iter();
            for item in result["output"].as_array_mut().expect("validated output") {
                if is_tool(item) {
                    *item = next.next().expect("matching tool count");
                }
            }
            result["usage"] = usage;
            validate(tools, &result)?;
            yield Some(json!({"type":"response.completed","response":result}));
            return;
        }
        if !eligible(tools, corrected) {
            Err(invalid(
                "Excel corrected tool is outside the permitted batch",
            ))?;
        }
        failed = corrected.clone();
    }
    Err(invalid(
        "Excel tool transport remains invalid after two corrections; no tool was executed",
    ))?;
    })
}

fn bind_original_code(
    tools: &ClientTools,
    original: &[Value],
    corrected: &mut [Value],
) -> Result<(), CodexClientError> {
    for (before, after) in original.iter().zip(corrected) {
        if tools.convert_call(before).is_ok() {
            continue;
        }
        let Some(args) = arguments(before) else {
            continue;
        };
        if named_envelope(&args).is_some() {
            continue;
        }
        let Some(code) = args["code"].as_str() else {
            continue;
        };
        let Some(mut bound) = arguments(after) else {
            continue;
        };
        if marked_target(&bound).is_some() {
            bound["code"] = code.into();
            after["arguments"] = bound.to_string().into();
            tools
                .convert_call(after)
                .map_err(|_| invalid("Excel corrected raw payload is invalid or oversized"))?;
        }
    }
    Ok(())
}

fn operation(call: &Value) -> Option<Value> {
    let mut value = canonical_history_call(call).ok()?;
    value["call_id"] = "compare".into();
    Some(value)
}

fn preserves_operations(tools: &ClientTools, original: &[Value], corrected: &[Value]) -> bool {
    original.iter().zip(corrected).all(|(before, after)| {
        let raw_cmd = arguments(after).is_some_and(|args| {
            args["summary"]
                .as_str()
                .is_some_and(|summary| summary.starts_with(envelope::FUNCTION_CMD_PREFIX))
        });
        let Ok(after) = tools.convert_call(after) else {
            return false;
        };
        if let Ok(before) = tools.convert_call(before) {
            return operation(&before) == operation(&after);
        }
        let Some(args) = arguments(before) else {
            return false;
        };
        if let Some(target) = explicit_target(&args) {
            let namespace = after["namespace"].as_str().unwrap_or_default();
            let name = after["name"].as_str().unwrap_or_default();
            let after_target = if namespace.is_empty() {
                name.to_owned()
            } else {
                format!("{namespace}.{name}")
            };
            if target != after_target {
                return false;
            }
        }
        if let Some(envelope) = named_envelope(&args) {
            return if after["type"] == "custom_tool_call" {
                envelope.get("input").or_else(|| envelope.get("args")) == after.get("input")
                    && after["input"].is_string()
            } else {
                envelope::arguments_field(&envelope)
                    .ok()
                    .and_then(|args| envelope::json_value(args).ok())
                    .zip(envelope::json_value(&after["arguments"]).ok())
                    .is_some_and(|(before, after)| before.is_object() && before == after)
            };
        }
        let Some(code) = args["code"].as_str() else {
            return false;
        };
        if after["type"] == "custom_tool_call" {
            after["input"].as_str() == Some(code)
        } else {
            let Some(mut actual) = envelope::json_value(&after["arguments"]).ok() else {
                return false;
            };
            let raw_field = if raw_cmd { "cmd" } else { "code" };
            if actual[raw_field].as_str() != Some(code) {
                return false;
            }
            let Some(actual) = actual.as_object_mut() else {
                return false;
            };
            actual.remove(raw_field);
            let expected = args
                .get("extended_summary")
                .map(envelope::json_value)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_else(|| json!({}));
            expected.as_object() == Some(actual)
        }
    })
}

fn extend_request(
    body: &mut Map<String, Value>,
    failed: &Value,
    count: usize,
) -> Result<(), CodexClientError> {
    let calls = batch(failed).ok_or_else(|| invalid("Excel correction lacks a complete batch"))?;
    let input = body
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid("Excel correction requires complete history"))?;
    let trigger = input
        .last()
        .is_some_and(|item| item["type"] == "compaction_trigger")
        .then(|| input.pop().unwrap());
    input.extend(
        failed["output"]
            .as_array()
            .expect("validated output")
            .iter()
            .cloned(),
    );
    for call in calls {
        let id = call["call_id"].as_str().expect("validated call");
        input.push(json!({"type":"function_call_output","id":super::request::function_item_id(id),
            "call_id":id,"output":json!({"executed":false,"error":{"code":"invalid_client_tool_transport",
                "message":"Correct the tool transport only; no tool in this batch was executed."}}).to_string()}));
    }
    input.push(super::request::message("developer", &format!(
        "The preceding batch failed transport validation before any client tool executed. \
         Return exactly {count} run_officejs calls in the same order, correcting transport only. \
         Preserve operations and exact source text. CUSTOM requires summary=cpr.custom/CATALOG_NAME \
         and raw input in code; FUNCTION_CODE requires summary=codex2api.function_code/CATALOG_NAME, \
         raw code and remaining arguments as JSON in extended_summary. FUNCTION_CMD requires \
         summary=codex2api.function_cmd/CATALOG_NAME, raw command in code and other arguments \
         as JSON in extended_summary. Ordinary FUNCTION uses \
         one JSON envelope with name and object arguments in code. Use the existing catalog only. \
         Do not add operations, execute Office code, or repeat commentary."
    )));
    if let Some(trigger) = trigger {
        input.push(trigger);
    }
    let iteration = body
        .get_mut("metadata")
        .and_then(|value| value.get_mut("agent_iteration"))
        .ok_or_else(|| invalid("Excel correction lacks iteration metadata"))?;
    let next = iteration
        .as_str()
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| invalid("Excel correction has invalid iteration metadata"))?;
    *iteration = next.to_string().into();
    if serde_json::to_vec(body)
        .map_err(CodexClientError::RequestBodyEncode)?
        .len()
        > MAX_RESPONSE_BYTES
    {
        return Err(invalid("Excel correction exceeds its request size limit"));
    }
    Ok(())
}

fn read_response<'a>(
    stream: &'a mut CodexBackendSseStream,
    original: &'a Value,
    previous_usage: &'a Value,
    usage_policy: &'a ExcelUsagePolicy,
) -> BoxStream<'a, Result<Option<Value>, CodexClientError>> {
    Box::pin(async_stream::try_stream! {
    let mut decoder = SseEventDecoder::default();
    let mut bytes = 0usize;
    let mut pending = BTreeSet::new();
    let mut observed_usage = json!({});
    loop {
        let chunk = stream.next().await.transpose()?;
        let finished = chunk.is_none();
        let events = if let Some(chunk) = chunk {
            bytes = bytes.saturating_add(chunk.len());
            if bytes > MAX_RESPONSE_BYTES {
                Err(invalid("Excel correction exceeded its response size limit"))?;
            }
            decoder.push(&chunk)?
        } else {
            decoder.finish()?
        };
        for event in events {
            if event.data == "[DONE]" {
                continue;
            }
            let mut value: Value = serde_json::from_str(&event.data)
                .map_err(|_| invalid("Excel correction returned invalid JSON"))?;
            if !value.is_object() {
                Err(invalid("Excel correction returned a non-object event"))?;
            }
            let mut reported = false;
            for usage in [value.pointer("/response/usage"), value.get("usage")].into_iter().flatten() {
                let mut canonical = json!({});
                add_usage(&mut canonical, usage)?;
                if canonical.as_object().is_some_and(|usage| !usage.is_empty()) {
                    max_usage(&mut observed_usage, &canonical);
                    reported = true;
                }
            }
            if reported {
                let mut cumulative = previous_usage.clone();
                add_usage(&mut cumulative, &observed_usage)?;
                usage_policy.record_repair_usage(original, &cumulative);
                yield None;
            }
            let kind = value["type"]
                .as_str()
                .or(event.event.as_deref())
                .unwrap_or_default()
                .to_owned();
            if matches!(
                kind.as_str(),
                "response.output_item.added" | "response.output_item.done"
            ) && is_tool(&value["item"])
            {
                pending.insert((
                    value["item"]["id"].clone().to_string(),
                    value["item"]["call_id"].clone().to_string(),
                ));
                if pending.len() > 512 {
                    Err(invalid("Excel correction returned too many tools"))?;
                }
            }
            if matches!(
                kind.as_str(),
                "response.completed" | "response.failed" | "response.incomplete" | "error"
            ) {
                value["type"] = kind.clone().into();
                if !value["response"]["usage"].is_object() && observed_usage != json!({}) {
                    if !value["response"].is_object() {
                        value["response"] = json!({});
                    }
                    value["response"]["usage"] = observed_usage.clone();
                }
                if kind != "response.completed" {
                    yield Some(value);
                    return;
                }
                let response = &mut value["response"];
                let Some(output) = response["output"].as_array() else {
                    response["status"] = "failed".into();
                    yield Some(value);
                    return;
                };
                for item in output.iter().filter(|item| is_tool(item)) {
                    pending.remove(&(
                        item["id"].clone().to_string(),
                        item["call_id"].clone().to_string(),
                    ));
                }
                if !pending.is_empty() {
                    response["status"] = "failed".into();
                }
                yield Some(value);
                return;
            }
        }
        if finished {
            Err(invalid(
                "Excel correction ended without a terminal response",
            ))?;
        }
    }
    })
}

fn max_usage(total: &mut Value, snapshot: &Value) {
    for (key, value) in snapshot.as_object().expect("canonical usage") {
        if value.is_object() {
            if !total[key].is_object() {
                total[key] = json!({});
            }
            max_usage(&mut total[key], value);
        } else if let Some(count) = value.as_u64() {
            total[key] = count.max(total[key].as_u64().unwrap_or(0)).into();
        }
    }
}

fn add_usage(total: &mut Value, usage: &Value) -> Result<(), CodexClientError> {
    if !usage.is_object() {
        return Ok(());
    }
    let mut normalized = json!({"usage":usage});
    ExcelUsagePolicy::default().normalize(&mut normalized);
    let usage = &normalized["usage"];
    let mut canonical = json!({"input_tokens_details":{},"output_tokens_details":{}});
    for (path, aliases) in [
        ("/input_tokens", &["/input_tokens", "/prompt_tokens"][..]),
        (
            "/output_tokens",
            &["/output_tokens", "/completion_tokens"][..],
        ),
        ("/total_tokens", &["/total_tokens"][..]),
        (
            "/input_tokens_details/cached_tokens",
            &[
                "/input_tokens_details/cached_tokens",
                "/prompt_tokens_details/cached_tokens",
                "/cached_tokens",
            ][..],
        ),
        (
            "/input_tokens_details/cache_write_tokens",
            &["/input_tokens_details/cache_write_tokens"][..],
        ),
        (
            "/output_tokens_details/reasoning_tokens",
            &[
                "/output_tokens_details/reasoning_tokens",
                "/completion_tokens_details/reasoning_tokens",
                "/reasoning_tokens",
            ][..],
        ),
    ] {
        if let Some(value) = aliases
            .iter()
            .find_map(|alias| usage.pointer(alias).and_then(Value::as_u64))
        {
            let (parent, field) = path.rsplit_once('/').expect("usage path");
            canonical.pointer_mut(parent).expect("usage parent")[field] = value.into();
        }
    }
    if canonical.get("total_tokens").is_none()
        && let Some(total) = canonical["input_tokens"]
            .as_u64()
            .zip(canonical["output_tokens"].as_u64())
            .and_then(|(input, output)| input.checked_add(output))
    {
        canonical["total_tokens"] = total.into();
    }
    sum_usage(total, &canonical)
}

fn sum_usage(total: &mut Value, usage: &Value) -> Result<(), CodexClientError> {
    let values = usage.as_object().expect("canonical usage object");
    for (key, value) in values {
        if value.as_object().is_some_and(Map::is_empty) {
            continue;
        }
        if value.is_object() {
            if total.get(key).is_none() {
                total[key] = json!({});
            }
            if total[key].is_object() {
                sum_usage(&mut total[key], value)?;
            }
        } else if let Some(value) = value.as_u64() {
            total[key] = total[key]
                .as_u64()
                .unwrap_or(0)
                .checked_add(value)
                .ok_or_else(|| invalid("Excel correction usage exceeded its numeric limit"))?
                .into();
        }
    }
    Ok(())
}
