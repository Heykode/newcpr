//! One Excel Responses lifecycle containing a genuine compaction and generation.

use super::{ExcelRequestError, image_policy, prepare_request};
use crate::transport::{
    CodexBackendClient, CodexBackendStreamingResponse, CodexClientError, CodexRequestContext,
    protocol::responses::CodexResponsesRequest,
};
use bytes::Bytes;
use futures::StreamExt;
use gateway_protocol::openai::sse::{SseError, SseEventDecoder, encode_sse_event};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

fn invalid(message: &str) -> CodexClientError {
    CodexClientError::InvalidSse(SseError::ParseError(message.into()))
}
fn policy_error(error: ExcelRequestError) -> CodexClientError {
    invalid(&error.to_string())
}

pub(super) fn merge_usage(target: &mut Value, source: &Value) {
    let Some(source) = source.as_object() else {
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    for (key, value) in source {
        if value.is_object() {
            merge_usage(&mut target[key], value);
        } else if let Some(number) = value.as_u64() {
            target[key] = target[key]
                .as_u64()
                .unwrap_or(0)
                .saturating_add(number)
                .into();
        }
    }
}

fn observe_usage(target: &mut Value, source: &Value) {
    let Some(source) = source.as_object() else {
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    for (key, value) in source {
        if value.is_object() {
            observe_usage(&mut target[key], value);
        } else if value.as_u64().is_some_and(|number| number > 0) {
            target[key] = value.clone();
        }
    }
}

fn has_usage(value: &Value) -> bool {
    value.as_object().is_some_and(|fields| {
        fields
            .values()
            .any(|v| v.as_u64().is_some_and(|n| n > 0) || has_usage(v))
    })
}

fn terminal_usage(terminal: &Value, progressive: &Value) -> Value {
    if has_usage(terminal) {
        terminal.clone()
    } else {
        progressive.clone()
    }
}

fn encode(kind: &str, mut payload: Value, sequence: &mut u64) -> Bytes {
    payload["type"] = kind.into();
    payload["sequence_number"] = (*sequence).into();
    *sequence += 1;
    Bytes::from(encode_sse_event(kind, &payload.to_string()))
}

async fn phase_request(
    client: &CodexBackendClient,
    original: &CodexResponsesRequest,
    context: CodexRequestContext<'_>,
    input: Vec<Value>,
    compact: bool,
) -> Result<(CodexResponsesRequest, Option<super::images::UploadedImages>), CodexClientError> {
    let prepared = original.excel.as_ref().expect("Excel image policy");
    let policy = prepared.image_policy.as_ref().expect("Excel image policy");
    let mut source = policy.source.clone();
    source.insert("input".into(), input.into());
    let tools = if compact {
        source.remove("text");
        source.remove("response_format");
        source.insert("tool_choice".into(), "none".into());
        source["input"]
            .as_array_mut()
            .expect("phase input")
            .push(json!({"type":"compaction_trigger"}));
        prepared
            .tools
            .clone()
            .with_choice(Some(&json!("none")))
            .map_err(policy_error)?
    } else {
        prepared.tools.clone()
    };
    let empty = BTreeMap::new();
    let native = prepared
        .replay
        .as_ref()
        .map(|capture| capture.native_calls())
        .unwrap_or(&empty);
    let body = prepare_request(
        &source,
        &tools,
        native,
        if compact {
            None
        } else {
            prepared.structured.as_ref()
        },
    )
    .map_err(policy_error)?;
    super::images::validate_with_limits(&body, true, prepared.image_limits)
        .map_err(policy_error)?;
    let (mut body, image_lease) = policy.stage(body).await.map_err(policy_error)?;
    let images = if super::images::has_user_inline(&body) {
        let images = super::images::upload_inline_with_limits(
            client,
            &client.profile.snapshot(),
            context,
            &prepared.endpoint,
            &body,
            prepared.replay.as_ref(),
            prepared.image_limits,
        )
        .await?;
        body = images.body.clone();
        Some(images)
    } else {
        None
    };
    let mut request = original.clone();
    let phase = request.excel.as_mut().expect("Excel image policy");
    phase.body = body;
    phase.tools = tools;
    if compact {
        phase.structured = None;
    }
    phase._image_lease = image_lease;
    phase.image_policy = None;
    phase.replay = None; // Publish only the combined, delivered response.
    phase.completed = Default::default();
    phase.usage = prepared.usage.detached();
    Ok((request, images))
}

async fn send_phase(
    client: &CodexBackendClient,
    request: &CodexResponsesRequest,
    images: Option<super::images::UploadedImages>,
    context: CodexRequestContext<'_>,
) -> Result<CodexBackendStreamingResponse, CodexClientError> {
    // Neither phase can hop to a different session-proxy lease.
    match client
        .create_prepared_response_stream(request, context, false)
        .await
    {
        Ok(response) => Ok(response),
        Err(error) => {
            if let Some(images) = images {
                images.observe_error(&error).await;
            }
            Err(error)
        }
    }
}

pub(crate) async fn start(
    client: &CodexBackendClient,
    original: &CodexResponsesRequest,
    context: CodexRequestContext<'_>,
) -> Result<CodexBackendStreamingResponse, CodexClientError> {
    let prepared = original.excel.as_ref().expect("Excel image policy").clone();
    let policy = prepared.image_policy.clone().expect("Excel image policy");
    let split = policy.split.expect("automatic image compaction");
    let (compact, images) = phase_request(
        client,
        original,
        context,
        policy.input()[..split].to_vec(),
        true,
    )
    .await?;
    let compact_completed = Arc::clone(&compact.excel.as_ref().expect("compaction").completed);
    let mut response = send_phase(client, &compact, images, context).await?;
    let mut compact_body = Some(response.body);
    let mut compact_request = Some(compact);
    let client = client.clone();
    let original = original.clone();
    let authorization: Arc<str> = context.authorization.into();
    let account: Option<Arc<str>> = context.account_id.map(Into::into);
    let request_id: Arc<str> = context.request_id.into();
    let trace = context.trace.cloned();
    let updates = response.rate_limit_updates.clone();
    response.body = Box::pin(async_stream::try_stream! {
        let mut compact_usage = json!({});
        let mut decoder = SseEventDecoder::default();
        let mut total = 0usize;
        let mut complete = None;
        let mut failure = None;
        let mut saw_terminal = false;
        prepared.usage.record_repair_usage(&json!({"model":original.model()}), &compact_usage);
        while let Some(chunk) = compact_body.as_mut().expect("active compact body").next().await {
            let chunk = chunk?;
            total = total.saturating_add(chunk.len());
            if total > 32 * 1024 * 1024 { Err(invalid("Excel image compaction response exceeded 32 MiB"))?; }
            for event in decoder.push(&chunk)? {
                if event.data == "[DONE]" { continue; }
                let value: Value = serde_json::from_str(&event.data).map_err(|_| invalid("invalid Excel compaction event"))?;
                let kind = value["type"].as_str().or(event.event.as_deref()).unwrap_or("");
                observe_usage(&mut compact_usage, value.pointer("/response/usage").or_else(|| value.get("usage")).unwrap_or(&Value::Null));
                prepared.usage.record_repair_usage(&value["response"], &compact_usage);
                if matches!(kind, "response.completed" | "response.failed" | "response.cancelled" | "response.incomplete" | "error") {
                    saw_terminal = true;
                    if kind == "response.completed" { complete = Some(value["response"].clone()); }
                    else { failure = Some((kind.to_owned(), value)); }
                    break;
                }
            }
            // Internal metering only; no partial compact text or tool execution escapes.
            yield Bytes::new();
            if saw_terminal { break; }
        }
        drop(compact_body.take());
        drop(compact_request.take());
        if let Some((kind, mut value)) = failure {
            if value["response"].is_object() {
                value["response"]["usage"] = terminal_usage(&value["response"]["usage"], &compact_usage);
                prepared.usage.clear_repair_usage();
            }
            yield encode(&kind, value, &mut 0);
            return;
        }
        let mut compact_response = complete.ok_or_else(|| invalid("Excel image compaction did not complete"))?;
        let window = image_policy::compact_window(&compact_response).map_err(policy_error)?;
        compact_response["usage"] = terminal_usage(&compact_response["usage"], &compact_usage);
        let raw_compact_usage = compact_completed.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref().map(|value| value["usage"].clone()).filter(has_usage).unwrap_or_else(|| compact_response["usage"].clone());
        prepared.usage.project(&mut compact_response);
        compact_usage = compact_response["usage"].clone();
        policy.checkpoint(&window).await.map_err(policy_error)?;
        let next: Vec<Value> = window.iter().chain(&policy.input()[split..]).cloned().collect();
        let mut context = CodexRequestContext::auxiliary(&authorization, account.as_deref(), &request_id, None);
        context.trace = trace.as_ref();
        let (generation, images) = phase_request(&client, &original, context, next, false).await?;
        let generation_prepared = generation.excel.as_ref().expect("generation").clone();
        let mut generated = send_phase(&client, &generation, images, context).await?;
        if let Some(target) = &updates
            && let Some(observation) = crate::transport::rate_limits::CodexRateLimitObservation::from_headers(&generated.rate_limit_headers, generated.rate_limit_observed_at) {
            target.lock().await.push(observation);
        }
        let mut decoder = SseEventDecoder::default();
        let mut sequence = 0;
        let mut inserted = false;
        let mut terminal = false;
        let mut progressive = json!({});
        while let Some(chunk) = generated.body.next().await {
            if let (Some(target), Some(source)) = (&updates, &generated.rate_limit_updates) {
                target.lock().await.extend(std::mem::take(&mut *source.lock().await));
            }
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    if let Some(record) = generation_prepared.usage.take_failed_repair_usage() {
                        let mut usage = record["usage"].clone(); merge_usage(&mut usage, &compact_usage);
                        prepared.usage.record_repair_usage(&record, &usage);
                    }
                    Err(error)?
                }
            };
            if chunk.is_empty() {
                if let Some(record) = generation_prepared.usage.take_failed_repair_usage() {
                    let mut usage = record["usage"].clone(); merge_usage(&mut usage, &compact_usage);
                    prepared.usage.record_repair_usage(&record, &usage);
                }
                yield Bytes::new(); continue;
            }
            for event in decoder.push(&chunk)? {
                if event.data == "[DONE]" { continue; }
                let mut value: Value = serde_json::from_str(&event.data).map_err(|_| invalid("invalid Excel generation event"))?;
                let kind = value["type"].as_str().or(event.event.as_deref()).unwrap_or("").to_owned();
                if !inserted && !matches!(kind.as_str(), "response.created" | "response.in_progress") {
                    inserted = true;
                    for (index, item) in window.iter().enumerate() {
                        yield encode("response.output_item.added", json!({"output_index":index,"item":item}), &mut sequence);
                        yield encode("response.output_item.done", json!({"output_index":index,"item":item}), &mut sequence);
                    }
                }
                if let Some(index) = value["output_index"].as_u64() { value["output_index"] = (index + window.len() as u64).into(); }
                observe_usage(&mut progressive, value.pointer("/response/usage").or_else(|| value.get("usage")).unwrap_or(&Value::Null));
                let mut observed = progressive.clone(); merge_usage(&mut observed, &compact_usage);
                prepared.usage.record_repair_usage(&value["response"], &observed);
                terminal = matches!(kind.as_str(), "response.completed" | "response.failed" | "response.cancelled" | "response.incomplete" | "error");
                if terminal && value["response"].is_object() {
                    observed = terminal_usage(&value["response"]["usage"], &progressive);
                    merge_usage(&mut observed, &compact_usage);
                    let output = value["response"]["output"].as_array().cloned().unwrap_or_default();
                    value["response"]["output"] = window.iter().chain(&output).cloned().collect::<Vec<_>>().into();
                    value["response"]["usage"] = observed;
                }
                if kind == "response.completed" {
                    let completed = { generation_prepared.completed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone() };
                    let mut raw = completed.ok_or_else(|| invalid("Excel generation completion missing"))?;
                    let output = raw["output"].as_array().cloned().ok_or_else(|| invalid("Excel generation output missing"))?;
                    raw["output"] = window.iter().chain(&output).cloned().collect::<Vec<_>>().into();
                    raw["usage"] = terminal_usage(&raw["usage"], &progressive);
                    merge_usage(&mut raw["usage"], &raw_compact_usage);
                    prepared.usage.normalize(&mut json!({"usage":raw["usage"]}));
                    if let Some(replay) = &prepared.replay { let _ = replay.commit(&raw, &prepared.tools).await; }
                    *prepared.completed.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(raw);
                }
                if terminal && value["response"].is_object() { prepared.usage.clear_repair_usage(); }
                yield encode(&kind, value, &mut sequence);
                if terminal { break; }
            }
            if terminal { break; }
        }
        if !terminal { Err(invalid("Excel image continuation ended without a terminal event"))?; }
    });
    Ok(response)
}
