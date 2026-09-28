use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use bytes::Bytes;
use futures::StreamExt;
use gateway_protocol::openai::sse::{SseError, SseEvent, SseEventDecoder, encode_sse_event};
use serde_json::{Value, json};

use super::{ClientTools, ExcelPreparedRequest, StructuredOutput, structured};
use crate::transport::{CodexBackendSseStream, CodexClientError};

const MAX_TOOL_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn transform_stream(
    source: CodexBackendSseStream,
    prepared: &ExcelPreparedRequest,
) -> CodexBackendSseStream {
    transform_stream_with_repair(source, prepared, None)
}

pub(crate) fn transform_stream_with_repair(
    source: CodexBackendSseStream,
    prepared: &ExcelPreparedRequest,
    mut sender: Option<super::repair::Sender>,
) -> CodexBackendSseStream {
    let replay = prepared.replay.clone();
    let exit_lease = prepared.exit_lease.clone();
    let request_body = sender.as_ref().map(|_| prepared.body.clone());
    let mut transform = Relay {
        tools: prepared.tools.clone(),
        structured: prepared.structured.clone(),
        completed: Arc::clone(&prepared.completed),
        usage: prepared.usage.clone(),
        pending_tools: BTreeSet::new(),
        held_bytes: 0,
        terminal: false,
        sequence: 0,
        effort: prepared.body.get("reasoning_effort").cloned(),
        allow_unknown_repair: super::repair::no_tool_history(&prepared.body),
        regenerated_from: None,
    };
    Box::pin(async_stream::try_stream! {
        let mut source = Some(source);
        let mut decoder = SseEventDecoder::default();
        let mut recorded = false;
        loop {
            let chunk = match &mut source {
                Some(source) => match source.next().await {
                    Some(Err(error)) => {
                        if let Some(lease)=&exit_lease {lease.report(gateway_core::provider_ports::session_proxy::SessionProxyOutcome::StreamFailure);}
                        Err(error)?
                    },
                    value=>value.transpose()?,
                },
                None => None,
            };
            let finished = chunk.is_none();
            let events = match chunk {
                Some(chunk) => decoder.push(&chunk)?,
                None => decoder.finish()?,
            };
            for mut event in events {
                if let Some(sender) = sender.as_mut()
                    && let Some((original, unknown)) = transform.repair_candidate(&event)
                {
                    // The old response must not occupy a socket while its correction starts.
                    drop(source.take());
                    let mut correction = if unknown {
                        transform.regenerated_from = Some(original["output"].as_array().expect("validated output").len() - 1);
                        super::repair::correct_unknown(
                            &original, &transform.tools, request_body.clone().expect("repair sender body"), sender, &transform.usage,
                        )
                    } else { super::repair::correct(
                        &original, &transform.tools, request_body.clone().expect("repair sender body"), sender, &transform.usage,
                    ) };
                    while let Some(step) = correction.next().await {
                        match step? {
                            Some(corrected) => event.data = corrected.to_string(),
                            // Internal checkpoint only; the provider consumes its usage snapshot.
                            None => yield Bytes::new(),
                        }
                    }
                    drop(correction);
                    // IDs belong to the rejected model turn; only the corrected batch is emitted.
                    transform.pending_tools.clear();
                }
                let events = transform.event(event)?;
                persist_completion(&transform, replay.as_ref(), &mut recorded).await?;
                // A consumer may stop polling as soon as it receives completion.
                // Observe the successful upstream before yielding that last frame.
                if transform.terminal
                    && transform.completed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
                    && let Some(lease)=&exit_lease {lease.report(gateway_core::provider_ports::session_proxy::SessionProxyOutcome::Completed);}
                let last = events.len().saturating_sub(1);
                for (index, bytes) in events.into_iter().enumerate() {
                    if transform.terminal && index == last && transform.completed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some() {
                        transform.usage.clear_repair_usage();
                    }
                    yield bytes;
                }
                if transform.terminal {
                    return;
                }
            }
            if finished { break; }
        }
        // No synthetic completion: the canonical decoder owns truncated-stream errors.
        if let Some(lease)=&exit_lease {lease.report(gateway_core::provider_ports::session_proxy::SessionProxyOutcome::StreamFailure);}
    })
}

async fn persist_completion(
    relay: &Relay,
    replay: Option<&super::replay::ReplayCapture>,
    recorded: &mut bool,
) -> Result<(), CodexClientError> {
    if *recorded || !relay.terminal {
        return Ok(());
    }
    *recorded = true;
    let raw = relay
        .completed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let (Some(replay), Some(raw)) = (replay, raw)
        && replay.commit(&raw, &relay.tools).await.is_err()
    {
        tracing::warn!("Excel optional history cache write failed; completion preserved");
    }
    Ok(())
}

struct Relay {
    tools: ClientTools,
    structured: Option<StructuredOutput>,
    completed: Arc<Mutex<Option<Value>>>,
    usage: super::usage::ExcelUsagePolicy,
    pending_tools: BTreeSet<(String, String)>,
    held_bytes: usize,
    terminal: bool,
    sequence: u64,
    effort: Option<Value>,
    allow_unknown_repair: bool,
    regenerated_from: Option<usize>,
}

impl Relay {
    fn repair_candidate(&self, event: &SseEvent) -> Option<(Value, bool)> {
        let data: Value = serde_json::from_str(&event.data).ok()?;
        if data["type"].as_str().or(event.event.as_deref()) != Some("response.completed") {
            return None;
        }
        let response = &data["response"];
        let output = response["output"].as_array()?;
        if !self.pending_tools.iter().all(|(id, call)| {
            output.iter().any(|item| {
                item["id"].as_str() == Some(id) && item["call_id"].as_str() == Some(call)
            })
        }) || super::repair::validate(&self.tools, response).is_ok()
        {
            return None;
        }
        let unknown =
            self.allow_unknown_repair && super::repair::unknown_eligible(&self.tools, response);
        (unknown || self.structured.is_none() && super::repair::eligible(&self.tools, response))
            .then(|| (response.clone(), unknown))
    }

    fn event(&mut self, event: SseEvent) -> Result<Vec<Bytes>, CodexClientError> {
        if event.data == "[DONE]" {
            return Ok(vec![Bytes::from_static(b"data: [DONE]\n\n")]);
        }
        let mut data: Value = serde_json::from_str(&event.data)
            .map_err(|_| protocol("Excel returned malformed event JSON"))?;
        if !data.is_object() {
            return Err(protocol("Excel returned a non-object event"));
        }
        let kind = data
            .get("type")
            .and_then(Value::as_str)
            .or(event.event.as_deref())
            .unwrap_or("")
            .to_owned();
        if self.terminal {
            return Ok(Vec::new());
        }
        let tool_item = data
            .get("item")
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
            .is_some_and(|kind| matches!(kind, "function_call" | "custom_tool_call"));
        if self.structured.is_some() && structured::is_message_event(&kind, data.get("item")) {
            return Ok(Vec::new());
        }
        if matches!(
            kind.as_str(),
            "response.function_call_arguments.delta"
                | "response.function_call_arguments.done"
                | "response.custom_tool_call_input.delta"
                | "response.custom_tool_call_input.done"
        ) || (tool_item
            && matches!(
                kind.as_str(),
                "response.output_item.added" | "response.output_item.done"
            ))
        {
            if tool_item {
                let item = &data["item"];
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| protocol("Excel tool item has no identity"))?;
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| protocol("Excel tool item has no call identity"))?;
                self.pending_tools.insert((id.into(), call_id.into()));
                if self.pending_tools.len() > 512 {
                    return Err(protocol("Excel returned too many tool items"));
                }
            }
            self.held_bytes = self.held_bytes.saturating_add(event.data.len());
            if self.held_bytes > MAX_TOOL_BYTES {
                return Err(protocol("Excel tool relay exceeded its buffer limit"));
            }
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        if let Some(response) = data.get_mut("response").filter(|value| value.is_object()) {
            self.tools.project_choice(response);
            response["parallel_tool_calls"] = self.tools.parallel_allowed().into();
            if let Some(effort) = &self.effort {
                response["reasoning"] = json!({"effort":effort});
            }
        }
        if let Some(format) = &self.structured
            && let Some(response) = data.get_mut("response").filter(|value| value.is_object())
        {
            format.project(response);
            if kind != "response.completed" {
                response["output"] = json!([]);
            }
        }
        if kind == "response.completed" {
            let raw = data
                .get("response")
                .cloned()
                .ok_or_else(|| protocol("Excel completion has no response"))?;
            if raw.get("status").and_then(Value::as_str) != Some("completed") {
                return Err(protocol("Excel completion has an invalid status"));
            }
            if let Some(format) = &self.structured {
                format.validate(&raw).map_err(|_| {
                    protocol("Excel final answer failed structured output validation")
                })?;
            }
            let items = data
                .pointer_mut("/response/output")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| protocol("Excel completion has no output"))?;
            self.tools.validate_completion(items).map_err(|_| {
                protocol("Excel output violated the requested tool choice or parallel-call limit")
            })?;
            let mut completed_tools = BTreeSet::new();
            let mut completed_item_ids = BTreeSet::new();
            for (index, item) in items.iter_mut().enumerate() {
                if matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call" | "custom_tool_call")
                ) {
                    let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
                    let call_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if !completed_tools.insert(call_id.to_owned()) {
                        return Err(protocol("Excel completion duplicated a tool call"));
                    }
                    self.pending_tools.remove(&(id.into(), call_id.into()));
                    let converted = self.tools.convert_call(item).map_err(|_| {
                        protocol("Excel returned an undeclared or malformed tool call")
                    })?;
                    if !completed_item_ids.insert(converted["id"].to_string()) {
                        return Err(protocol("Excel completion duplicated a tool identity"));
                    }
                    result.extend(tool_events(&converted, index));
                    *item = converted;
                } else if (self.structured.is_some()
                    || self.regenerated_from.is_some_and(|start| index >= start))
                    && item.get("type").and_then(Value::as_str) == Some("message")
                {
                    result.extend(structured::message_events(item, index));
                }
            }
            if !self.pending_tools.is_empty() {
                return Err(protocol("Excel completion omitted an original tool item"));
            }
            *self
                .completed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(raw);
            self.terminal = true;
        } else if matches!(
            kind.as_str(),
            "response.failed" | "response.incomplete" | "error"
        ) {
            self.terminal = true;
        }
        if kind != "response.completed" {
            // Unfinished tools and unvalidated structured text never reach clients.
            if let Some(items) = data
                .pointer_mut("/response/output")
                .and_then(Value::as_array_mut)
            {
                items.retain(|item| {
                    !matches!(
                        item.get("type").and_then(Value::as_str),
                        Some("function_call" | "custom_tool_call")
                    ) && !(self.structured.is_some()
                        && item.get("type").and_then(Value::as_str) == Some("message"))
                });
            }
        }
        self.usage.normalize(&mut data);
        data["type"] = kind.into();
        result.push(data);
        Ok(result
            .into_iter()
            .map(|mut value| {
                value["sequence_number"] = self.sequence.into();
                self.sequence += 1;
                encode(value["type"].as_str().unwrap_or(""), &value)
            })
            .collect())
    }
}

fn protocol(message: &str) -> CodexClientError {
    CodexClientError::InvalidSse(SseError::ParseError(message.to_owned()))
}

fn encode(kind: &str, value: &Value) -> Bytes {
    Bytes::from(encode_sse_event(kind, &value.to_string()))
}

fn tool_events(item: &Value, index: usize) -> Vec<Value> {
    let custom = item["type"] == "custom_tool_call";
    let field = if custom { "input" } else { "arguments" };
    let prefix = if custom {
        "response.custom_tool_call_input"
    } else {
        "response.function_call_arguments"
    };
    let mut started = item.clone();
    started["status"] = "in_progress".into();
    started[field] = "".into();
    let added = json!({"type":"response.output_item.added","output_index":index,"item":started});
    let delta = json!({"type":format!("{prefix}.delta"),"output_index":index,"item_id":item["id"],"delta":item[field]});
    let mut done =
        json!({"type":format!("{prefix}.done"),"output_index":index,"item_id":item["id"]});
    done[field] = item[field].clone();
    let finished = json!({"type":"response.output_item.done","output_index":index,"item":item});
    vec![added, delta, done, finished]
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::TryStreamExt;

    #[tokio::test]
    async fn billing_policy_is_identical_for_stream_usage_and_compact_projection() {
        for enabled in [false, true] {
            let raw = json!({"id":"resp_billing","status":"completed","output":[],
                "usage":{"input_tokens":1000,"output_tokens":50,"total_tokens":1050,
                    "input_tokens_details":{"cached_tokens":100},"cache_creation_input_tokens":200}});
            let prepared = ExcelPreparedRequest {
                exit_lease: None,
                body: Default::default(),
                tools: ClientTools::default(),
                structured: None,
                _image_lease: None,
                image_limits: Default::default(),
                completed: Default::default(),
                usage: super::super::usage::ExcelUsagePolicy::new(enabled),
                replay: None,
                endpoint: super::super::RESPONSES_URL.into(),
            };
            let wire = encode(
                "response.completed",
                &json!({"type":"response.completed","response":raw}),
            );
            let chunks = wire
                .chunks(7)
                .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
                .collect::<Vec<_>>();
            let chunks = transform_stream(Box::pin(futures::stream::iter(chunks)), &prepared)
                .try_collect::<Vec<_>>()
                .await
                .unwrap();
            let mut decoder = SseEventDecoder::default();
            let events = decoder.push(&chunks.concat()).unwrap();
            let downstream: Value = serde_json::from_str(&events.last().unwrap().data).unwrap();
            let usage =
                gateway_protocol::openai::events::extract_usage(&downstream["response"]).unwrap();
            assert_eq!(usage.input_tokens, 1000);
            assert_eq!(usage.cached_tokens, 100);
            assert_eq!(usage.cache_write_tokens, if enabled { 0 } else { 200 });
            assert_eq!(usage.total_tokens, 1050);
            let mut expected_completion = raw.clone();
            expected_completion["parallel_tool_calls"] = true.into();
            assert_eq!(
                *prepared.completed.lock().unwrap(),
                Some(expected_completion)
            );
            let mut compact = raw;
            prepared.usage.project(&mut compact);
            assert_eq!(compact["usage"], downstream["response"]["usage"]);
            assert_eq!(
                prepared.usage.metadata()["upstreamUsage"]["/cache_creation_input_tokens"],
                200
            );
        }
    }

    #[tokio::test]
    async fn text_is_incremental_and_truncation_does_not_invent_completion() {
        let source = futures::stream::iter(vec![Ok(Bytes::from_static(
            b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n"
        ))]);
        let prepared = ExcelPreparedRequest {
            exit_lease: None,
            body: Default::default(),
            tools: ClientTools::default(),
            structured: None,
            _image_lease: None,
            image_limits: Default::default(),
            completed: Default::default(),
            usage: Default::default(),
            replay: None,
            endpoint: super::super::RESPONSES_URL.into(),
        };
        let chunks = transform_stream(Box::pin(source), &prepared)
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let text = String::from_utf8(chunks.concat()).unwrap();
        assert!(text.contains("hello"));
        assert!(!text.contains("response.completed"));
        assert!(prepared.completed.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn native_call_is_held_until_authorized_completion() {
        let source_body =
            json!({"tools":[{"type":"function","name":"read","parameters":{"type":"object"}}]});
        let native = json!({"type":"function_call","id":"fc_fixture","call_id":"call_fixture","name":"run_officejs",
            "arguments":json!({"code":json!({"name":"read","arguments":{"path":"sample.txt"}}).to_string()}).to_string()});
        let mut bytes = encode(
            "response.output_item.added",
            &json!({"type":"response.output_item.added","item":native,"output_index":0}),
        )
        .to_vec();
        bytes.extend_from_slice(&encode(
            "response.completed",
            &json!({"type":"response.completed","response":{
                "id":"resp_fixture","status":"completed","output":[native]
            }}),
        ));
        let prepared = ExcelPreparedRequest {
            exit_lease: None,
            body: Default::default(),
            tools: ClientTools::parse(source_body.as_object().unwrap()).unwrap(),
            structured: None,
            _image_lease: None,
            image_limits: Default::default(),
            completed: Default::default(),
            usage: Default::default(),
            replay: None,
            endpoint: super::super::RESPONSES_URL.into(),
        };
        let chunks = transform_stream(
            Box::pin(async_stream::stream! {
                let mut bytes = Bytes::from(bytes);
                while !bytes.is_empty() {
                    let size = bytes.len().min(7);
                    yield Ok(bytes.split_to(size));
                }
            }),
            &prepared,
        )
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
        let output = String::from_utf8(chunks.concat()).unwrap();
        assert!(!output.contains("run_officejs"));
        assert!(output.contains("read"));
        let events = SseEventDecoder::default().push(output.as_bytes()).unwrap();
        assert_eq!(events.len(), 5);
        for (index, event) in events.iter().enumerate() {
            let value: Value = serde_json::from_str(&event.data).unwrap();
            assert_eq!(value["sequence_number"], index as u64);
        }
        assert_eq!(
            prepared.completed.lock().unwrap().as_ref().unwrap()["output"][0]["name"],
            "run_officejs"
        );
    }

    #[tokio::test]
    async fn completed_tool_batches_validate_all_identities_before_emitting_calls() {
        for case in [
            "valid",
            "custom",
            "schema",
            "duplicate_call",
            "duplicate_item",
            "omitted",
            "serial",
        ] {
            let mut source_body = json!({
                "tools":[{"type":"function","name":"lookup","parameters":{
                    "type":"object","properties":{"value":{"type":"integer"}},
                    "required":["value"],"additionalProperties":false
                }}],
                "parallel_tool_calls":case != "serial",
                "text":{"format":{"type":"json_object"}}
            });
            let mut calls: Vec<Value> = (0..2)
                .map(|index| {
                    json!({
                        "type":"function_call","id":format!("fc_fixture_{index}"),
                        "call_id":format!("call_fixture_{index}"),"name":"run_officejs",
                        "arguments":json!({"code":json!({
                            "name":"lookup","arguments":{"value":index}
                        }).to_string()}).to_string()
                    })
                })
                .collect();
            match case {
                "schema" => {
                    calls[1]["arguments"] = json!({"code":json!({
                    "name":"lookup","arguments":{"value":"invalid"}
                }).to_string()})
                    .to_string()
                    .into()
                }
                "duplicate_call" => calls[1]["call_id"] = calls[0]["call_id"].clone(),
                "duplicate_item" => calls[1]["id"] = calls[0]["id"].clone(),
                "custom" => {
                    source_body["tools"] = json!([{"type":"custom","name":"lookup"}]);
                    for (index, call) in calls.iter_mut().enumerate() {
                        call["id"] = "fc_shared_native_item".into();
                        call["arguments"] = json!({"code":json!({
                            "name":"lookup","input":format!("payload {index}")
                        }).to_string()})
                        .to_string()
                        .into();
                    }
                }
                _ => {}
            }
            let mut wire = Vec::new();
            for (index, item) in calls.iter().enumerate() {
                wire.extend_from_slice(&encode(
                    "response.output_item.added",
                    &json!({
                        "type":"response.output_item.added","output_index":index,"item":item
                    }),
                ));
            }
            if case == "omitted" {
                calls.pop();
            }
            wire.extend_from_slice(&encode(
                "response.completed",
                &json!({
                    "type":"response.completed","response":{
                        "id":"resp_batch_fixture","status":"completed","output":calls
                    }
                }),
            ));
            let prepared = ExcelPreparedRequest {
                exit_lease: None,
                body: Default::default(),
                tools: ClientTools::parse(source_body.as_object().unwrap()).unwrap(),
                structured: StructuredOutput::parse(source_body.as_object().unwrap()).unwrap(),
                _image_lease: None,
                image_limits: Default::default(),
                completed: Default::default(),
                usage: Default::default(),
                replay: None,
                endpoint: super::super::RESPONSES_URL.into(),
            };
            let chunks = wire
                .chunks(7)
                .map(|part| Ok(Bytes::copy_from_slice(part)))
                .collect::<Vec<_>>();
            let results = transform_stream(Box::pin(futures::stream::iter(chunks)), &prepared)
                .collect::<Vec<_>>()
                .await;
            if !matches!(case, "valid" | "custom") {
                assert_eq!(
                    results.len(),
                    1,
                    "{case}: no executable events before rejection"
                );
                assert!(
                    matches!(results[0], Err(CodexClientError::InvalidSse(_))),
                    "{case}"
                );
                assert!(prepared.completed.lock().unwrap().is_none(), "{case}");
                continue;
            }
            let bytes = results
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .concat();
            let events = SseEventDecoder::default().push(&bytes).unwrap();
            assert_eq!(events.len(), 9);
            for (sequence, event) in events.iter().enumerate() {
                let value: Value = serde_json::from_str(&event.data).unwrap();
                assert_eq!(value["sequence_number"], sequence as u64);
            }
            let final_event: Value = serde_json::from_str(&events.last().unwrap().data).unwrap();
            assert_eq!(final_event["type"], "response.completed");
            for index in 0..2 {
                let call = &final_event["response"]["output"][index];
                assert_eq!(call["name"], "lookup");
                assert_eq!(call["call_id"], format!("call_fixture_{index}"));
                if case == "custom" {
                    assert_eq!(call["type"], "custom_tool_call");
                    assert_eq!(call["input"], format!("payload {index}"));
                } else {
                    assert_eq!(
                        serde_json::from_str::<Value>(call["arguments"].as_str().unwrap()).unwrap(),
                        json!({"value":index})
                    );
                }
            }
            assert_ne!(
                final_event["response"]["output"][0]["id"],
                final_event["response"]["output"][1]["id"]
            );
        }
    }

    #[tokio::test]
    async fn cache_write_failure_preserves_success_and_usage_without_publishing_history() {
        let source = json!({"input":"hello"});
        let restored = super::super::replay::restore(
            Arc::new(gateway_core::provider_ports::UnavailableProviderReplay),
            "owner".into(),
            "thread".into(),
            None,
            source.as_object().unwrap(),
        )
        .await
        .unwrap();
        let capture = restored.capture.clone();
        let prepared = ExcelPreparedRequest {
            exit_lease: None,
            body: Default::default(),
            tools: restored.tools,
            structured: None,
            _image_lease: None,
            image_limits: Default::default(),
            completed: Default::default(),
            usage: Default::default(),
            replay: Some(restored.capture),
            endpoint: super::super::RESPONSES_URL.into(),
        };
        let event = encode(
            "response.completed",
            &json!({
                "type":"response.completed","response":{
                    "id":"resp_cache_failure","status":"completed","output":[],
                    "usage":{"input_tokens":100,"output_tokens":1,"total_tokens":101,"input_tokens_details":{"cached_tokens":90}}
                }
            }),
        );
        let output = transform_stream(Box::pin(futures::stream::iter([Ok(event)])), &prepared)
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
            .concat();
        let events = SseEventDecoder::default().push(&output).unwrap();
        let final_event: Value = serde_json::from_str(&events.last().unwrap().data).unwrap();
        assert_eq!(final_event["type"], "response.completed");
        assert_eq!(
            final_event["response"]["usage"]["input_tokens_details"]["cached_tokens"],
            90
        );
        assert!(!capture.is_persisted());
    }
}
