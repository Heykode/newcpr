use super::{ClientTools, ExcelPreparedRequest, ExcelRequestError, transform_stream};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{Value, json};

fn parse(source: Value) -> Result<ClientTools, ExcelRequestError> {
    ClientTools::parse(source.as_object().unwrap())
}

fn function(schema: Value) -> Value {
    json!({"type":"function","name":"read","parameters":schema})
}

fn native(name: &str, args: Value) -> Value {
    json!({"type":"function_call","id":"fc_fixture","call_id":"call_fixture",
        "name":"run_officejs","arguments":json!({"code":json!({"name":name,"arguments":args}).to_string()}).to_string()})
}

fn repair_call(id: &str, marker: &str, code: &str) -> Value {
    json!({"type":"function_call","id":format!("fc_{id}"),"call_id":id,
        "name":"run_officejs","status":"completed","arguments":json!({
            "summary":marker,"code":code,"extended_summary":"{\"timeout\":123}",
            "destructive":false,"references":[]
        }).to_string()})
}

fn repair_response(id: &str, calls: Vec<Value>) -> Value {
    json!({"id":id,"status":"completed","model":super::tests::VERIFIED_MODEL,
        "output":calls,"usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12,
            "input_tokens_details":{"cached_tokens":0,"cache_write_tokens":0}}})
}

fn repair_wire(response: Value) -> String {
    let id = response["id"].clone();
    format!(
        "data: {}\n\ndata: {}\n\n",
        json!({"type":"response.created","response":{"id":id,"status":"in_progress","output":[]}}),
        json!({"type":"response.completed","response":response})
    )
}

async fn http_repair(
    responses: Vec<wiremock::ResponseTemplate>,
    function_code: bool,
) -> (
    String,
    Option<crate::transport::CodexClientError>,
    super::usage::ExcelUsagePolicy,
    Option<Value>,
    Vec<wiremock::Request>,
) {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use wiremock::{Mock, MockServer, matchers::path};
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    Mock::given(path(super::RESPONSES_PATH))
        .respond_with(move |_: &wiremock::Request| {
            responses
                .get(count.fetch_add(1, Ordering::SeqCst))
                .cloned()
                .unwrap_or_else(|| wiremock::ResponseTemplate::new(500))
        })
        .mount(&server)
        .await;
    let mut request = super::tests::request(
        format!("{}{}", server.uri(), super::RESPONSES_PATH),
        json!("run"),
    );
    let source = json!({"model":super::tests::VERIFIED_MODEL,"input":"run","reasoning":{"effort":"high"},
        "tools":[if function_code {
            json!({"type":"function","name":"exec","parameters":{"type":"object",
                "properties":{"code":{"type":"string"},"timeout":{"type":"integer"}}}})
        } else { json!({"type":"custom","name":"exec"}) },function(json!({"type":"object"}))]});
    let prepared = request.excel.as_mut().unwrap();
    prepared.tools = parse(source.clone()).unwrap();
    prepared.body = super::prepare_request(
        source.as_object().unwrap(),
        &prepared.tools,
        &Default::default(),
        None,
    )
    .unwrap();
    let usage = prepared.usage.clone();
    let completed = prepared.completed.clone();
    let mut body = super::tests::client(&server.uri())
        .create_response_stream_http_sse(
            &request,
            crate::transport::CodexRequestContext::auxiliary(
                "Bearer fixture",
                Some("workspace"),
                "req",
                None,
            ),
        )
        .await
        .unwrap()
        .body;
    let mut text = String::new();
    let mut error = None;
    while let Some(chunk) = body.next().await {
        match chunk {
            Ok(bytes) => text.push_str(std::str::from_utf8(&bytes).unwrap()),
            Err(failure) => {
                error = Some(failure);
                break;
            }
        }
    }
    drop(body);
    let completed = completed.lock().unwrap().clone();
    (
        text,
        error,
        usage,
        completed,
        server.received_requests().await.unwrap(),
    )
}

#[tokio::test]
async fn excel_correction_preserves_raw_payload_route_and_cumulative_usage() {
    use wiremock::ResponseTemplate;
    for function_code in [false, true] {
        let original = "let price = '$1';\r\n\ttext(price);\n";
        let marker = if function_code {
            "codex2api.function_code/exec"
        } else {
            "cpr.custom/exec"
        };
        let (text, error, usage, completed, requests) = http_repair(
            vec![
                ResponseTemplate::new(200).set_body_raw(
                    repair_wire(repair_response(
                        "resp_first",
                        vec![repair_call("bad", "Run client tool", original)],
                    )),
                    "text/event-stream",
                ),
                ResponseTemplate::new(200).set_body_raw(
                    repair_wire(repair_response(
                        "resp_hidden",
                        vec![repair_call("fixed", marker, "text('rewritten')")],
                    )),
                    "text/event-stream",
                ),
            ],
            function_code,
        )
        .await;
        assert!(error.is_none(), "{error:?}");
        assert!(usage.take_failed_repair_usage().is_none());
        assert_eq!(requests.len(), 2);
        assert!(!text.contains("resp_hidden"));
        assert!(!text.contains("rewritten"));
        let completed = completed.unwrap();
        assert_eq!(completed["id"], "resp_first");
        assert_eq!(completed["usage"]["input_tokens"], 20);
        let args = super::envelope::json_value(&completed["output"][0]["arguments"]).unwrap();
        assert_eq!(args["code"], original);
        for request in &requests {
            assert_eq!(request.headers["authorization"], "Bearer fixture");
            assert_eq!(request.headers["chatgpt-account-id"], "workspace");
            assert!(!request.headers.contains_key("cookie"));
            assert!(!request.headers.contains_key("x-codex-turn-state"));
        }
        let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
        let second: Value = serde_json::from_slice(&requests[1].body).unwrap();
        for field in ["model", "reasoning_effort", "tools", "model_selection"] {
            assert_eq!(first[field], second[field]);
        }
        assert_eq!(first["metadata"]["task_id"], second["metadata"]["task_id"]);
        assert_eq!(first["metadata"]["turn_id"], second["metadata"]["turn_id"]);
        let first_iteration: u64 = first["metadata"]["agent_iteration"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            second["metadata"]["agent_iteration"],
            (first_iteration + 1).to_string()
        );
        assert!(second["input"].as_array().unwrap().iter().any(|item| {
            item["type"] == "function_call_output"
                && item["output"]
                    .as_str()
                    .is_some_and(|text| text.contains("\"executed\":false"))
        }));
        assert_eq!(
            text.matches("\"type\":\"response.output_item.done\"")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn excel_correction_is_bounded_and_never_leaks_a_partial_batch() {
    use wiremock::ResponseTemplate;
    let responses = (0..3)
        .map(|index| {
            ResponseTemplate::new(200).set_body_raw(
                repair_wire(repair_response(
                    &format!("resp_{index}"),
                    vec![
                        native("read", json!({})),
                        repair_call("bad", "Run", "text(1)"),
                    ],
                )),
                "text/event-stream",
            )
        })
        .collect();
    let (text, error, usage, completed, requests) = http_repair(responses, false).await;
    assert!(error.is_some());
    assert_eq!(requests.len(), 3);
    assert!(!text.contains("response.output_item.done"));
    assert!(!text.contains("response.completed"));
    assert!(completed.is_none());
    assert_eq!(
        usage.take_failed_repair_usage().unwrap()["usage"]["total_tokens"],
        36
    );
    assert!(usage.take_failed_repair_usage().is_none());
}

#[tokio::test]
async fn excel_correction_rejects_changed_valid_operation_and_extra_calls() {
    use wiremock::ResponseTemplate;
    for changed in [
        vec![
            native("read", json!({"changed":true})),
            repair_call("fixed", "cpr.custom/exec", "text(1)"),
        ],
        vec![
            native("read", json!({})),
            repair_call("fixed", "cpr.custom/exec", "text(1)"),
            repair_call("extra", "cpr.custom/exec", "text(2)"),
        ],
    ] {
        let (text, error, _, completed, requests) = http_repair(
            vec![
                ResponseTemplate::new(200).set_body_raw(
                    repair_wire(repair_response(
                        "resp_first",
                        vec![
                            native("read", json!({})),
                            repair_call("bad", "Run", "text(1)"),
                        ],
                    )),
                    "text/event-stream",
                ),
                ResponseTemplate::new(200).set_body_raw(
                    repair_wire(repair_response("resp_second", changed)),
                    "text/event-stream",
                ),
            ],
            false,
        )
        .await;
        assert!(error.is_some());
        assert!(completed.is_none());
        assert_eq!(requests.len(), 2);
        assert!(!text.contains("response.output_item.done"));
    }
}

#[tokio::test]
async fn excel_correction_stops_on_http_rejection_without_losing_known_usage() {
    use wiremock::ResponseTemplate;
    for unknown in [false, true] {
        for status in [401, 403, 429, 500] {
            let (_, error, usage, completed, requests) = http_repair(
                vec![
                    ResponseTemplate::new(200).set_body_raw(
                        repair_wire(repair_response(
                            "resp_first",
                            vec![if unknown {
                                native("absent", json!({}))
                            } else {
                                repair_call("bad", "Run", "text(1)")
                            }],
                        )),
                        "text/event-stream",
                    ),
                    ResponseTemplate::new(status).set_body_json(
                        json!({"error":{"code":"rate_limit_exceeded","message":"fixture"}}),
                    ),
                ],
                false,
            )
            .await;
            assert!(
                matches!(error, Some(crate::transport::CodexClientError::Upstream {status: value,..}) if value.as_u16()==status)
            );
            assert_eq!(requests.len(), 2);
            assert!(completed.is_none());
            assert_eq!(
                usage.take_failed_repair_usage().unwrap()["usage"]["total_tokens"],
                12
            );
        }
    }
}

#[tokio::test]
async fn excel_correction_releases_old_stream_and_cancels_without_background_work() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    for unknown in [false, true] {
        let old_released = Arc::new(AtomicBool::new(false));
        let guard = Dropped(old_released.clone());
        let source: crate::transport::CodexBackendSseStream = Box::pin(async_stream::try_stream! {
            let _guard = guard;
            yield Bytes::from(repair_wire(repair_response("resp_first",vec![if unknown { native("absent", json!({})) } else { repair_call("bad","Run","text(1)") }])));
            futures::future::pending::<()>().await;
        });
        let mut prepared = super::tests::request("https://example.invalid".into(), json!("run"))
            .excel
            .unwrap();
        prepared.tools = parse(json!({"tools":[{"type":"custom","name":"exec"}]})).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let sender: super::repair::Sender = Box::new(move |_| {
            assert!(old_released.load(Ordering::SeqCst));
            let guard = Dropped(flag.clone());
            Box::pin(async move {
                let _guard = guard;
                futures::future::pending().await
            })
        });
        let mut stream = super::transform_stream_with_repair(source, &prepared, Some(sender));
        assert!(stream.next().await.unwrap().is_ok());
        assert!(stream.next().await.unwrap().unwrap().is_empty());
        let known_usage = prepared.usage.take_failed_repair_usage().unwrap();
        assert!(prepared.usage.repair_started());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), stream.next())
                .await
                .is_err()
        );
        drop(stream);
        assert!(cancelled.load(Ordering::SeqCst));
        assert_eq!(known_usage["usage"]["total_tokens"], 12);
        assert!(prepared.usage.take_failed_repair_usage().is_none());
    }
}

#[tokio::test]
async fn excel_correction_preserves_sse_error_without_retry_or_completion() {
    use wiremock::ResponseTemplate;
    for unknown in [false, true] {
        for kind in ["response.failed", "error"] {
            let failure = json!({"type":kind,"error":{"code":"token_expired","message":"fixture","status":401},
            "response":{"id":"resp_hidden","status":"failed","output":[],
                "error":{"code":"token_expired","message":"fixture"},
                "usage":{"input_tokens":3,"output_tokens":0,"total_tokens":3}}});
            let (text, error, usage, completed, requests) = http_repair(
                vec![
                    ResponseTemplate::new(200).set_body_raw(
                        repair_wire(repair_response(
                            "resp_first",
                            vec![if unknown {
                                native("absent", json!({}))
                            } else {
                                repair_call("bad", "Run", "text(1)")
                            }],
                        )),
                        "text/event-stream",
                    ),
                    ResponseTemplate::new(200)
                        .set_body_raw(format!("data: {failure}\n\n"), "text/event-stream"),
                ],
                false,
            )
            .await;
            assert!(error.is_none());
            assert_eq!(requests.len(), 2);
            assert!(completed.is_none());
            assert!(text.contains("token_expired"));
            assert!(!text.contains("resp_hidden"));
            assert!(!text.contains("response.completed"));
            assert!(!text.contains("response.output_item.done"));
            assert_eq!(
                usage.take_failed_repair_usage().unwrap()["usage"]["total_tokens"],
                15
            );
            assert!(usage.take_failed_repair_usage().is_none());
        }
    }
}

#[tokio::test]
async fn excel_correction_rejects_incomplete_and_truncated_responses() {
    use wiremock::ResponseTemplate;
    let terminal = json!({"type":"response.incomplete","response":{
        "id":"resp_hidden","status":"incomplete","output":[],
        "usage":{"input_tokens":3,"output_tokens":1,"total_tokens":4}}});
    for (wire, expected) in [
        (format!("data: {terminal}\n\n"), 16),
        (
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hidden\"}\n\n".into(),
            12,
        ),
    ] {
        let (text, error, usage, completed, requests) = http_repair(
            vec![
                ResponseTemplate::new(200).set_body_raw(
                    repair_wire(repair_response(
                        "resp_first",
                        vec![repair_call("bad", "Run", "text(1)")],
                    )),
                    "text/event-stream",
                ),
                ResponseTemplate::new(200).set_body_raw(wire, "text/event-stream"),
            ],
            false,
        )
        .await;
        assert!(error.is_some());
        assert_eq!(requests.len(), 2);
        assert!(completed.is_none());
        assert!(!text.contains("hidden"));
        assert!(!text.contains("response.completed"));
        assert_eq!(
            usage.take_failed_repair_usage().unwrap()["usage"]["total_tokens"],
            expected
        );
    }
}

#[tokio::test]
async fn excel_correction_never_changes_function_metadata_or_invalid_arguments() {
    use wiremock::ResponseTemplate;
    let mut changed_metadata = repair_call("fixed", "codex2api.function_code/exec", "text(1)");
    let mut args = super::envelope::json_value(&changed_metadata["arguments"]).unwrap();
    args["extended_summary"] = "{\"timeout\":999}".into();
    changed_metadata["arguments"] = args.to_string().into();
    let mut invalid_arguments = native("read", json!(["not-an-object"]));
    invalid_arguments["call_id"] = "bad".into();
    for (original, corrected) in [
        (repair_call("bad", "Run", "text(1)"), changed_metadata),
        (invalid_arguments, native("read", json!({"invented":true}))),
    ] {
        let (text, error, _, completed, requests) = http_repair(
            vec![
                ResponseTemplate::new(200).set_body_raw(
                    repair_wire(repair_response("resp_first", vec![original])),
                    "text/event-stream",
                ),
                ResponseTemplate::new(200).set_body_raw(
                    repair_wire(repair_response("resp_second", vec![corrected])),
                    "text/event-stream",
                ),
            ],
            true,
        )
        .await;
        assert!(error.is_some());
        assert_eq!(requests.len(), 2);
        assert!(completed.is_none());
        assert!(!text.contains("response.output_item.done"));
    }
}

#[tokio::test]
async fn excel_unknown_first_tool_regenerates_once_without_fabricating_history() {
    use wiremock::ResponseTemplate;
    let (text, error, usage, completed, requests) = http_repair(
        vec![
            ResponseTemplate::new(200).set_body_raw(
                repair_wire(repair_response(
                    "resp_first",
                    vec![native("absent", json!({}))],
                )),
                "text/event-stream",
            ),
            ResponseTemplate::new(200).set_body_raw(
                repair_wire(repair_response(
                    "resp_hidden",
                    vec![native("read", json!({}))],
                )),
                "text/event-stream",
            ),
        ],
        false,
    )
    .await;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(requests.len(), 2);
    let completed = completed.unwrap();
    assert_eq!(completed["id"], "resp_first");
    assert_eq!(completed["usage"]["total_tokens"], 24);
    assert!(!text.contains("resp_hidden"));
    assert!(!text.contains("absent"));
    assert!(!usage.repair_started());
    let mut first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let mut second: Value = serde_json::from_slice(&requests[1].body).unwrap();
    let original_input = first.as_object_mut().unwrap().remove("input").unwrap();
    let mut corrected_input = second.as_object_mut().unwrap().remove("input").unwrap();
    let reminder = corrected_input.as_array_mut().unwrap().pop().unwrap();
    assert_eq!(reminder["role"], "developer");
    assert_eq!(
        original_input, corrected_input,
        "do not append fake calls or results"
    );
    assert_eq!(
        first, second,
        "keep account-scoped metadata, model and tools"
    );
    for name in [
        "authorization",
        "chatgpt-account-id",
        "x-openai-account-id",
        "x-basispoints-auth-mode",
    ] {
        assert_eq!(requests[0].headers.get(name), requests[1].headers.get(name));
    }
    assert_eq!(requests[0].url, requests[1].url);
}

#[tokio::test]
async fn excel_unknown_tool_second_failure_never_loops_or_releases_tools() {
    use wiremock::ResponseTemplate;
    for corrected in [
        native("absent", json!({})),
        native("read", json!([])),
        repair_call("bad", "Run", "raw_code()"),
    ] {
        let (text, error, usage, completed, requests) = http_repair(
            vec![
                ResponseTemplate::new(200).set_body_raw(
                    repair_wire(repair_response(
                        "resp_first",
                        vec![native("absent", json!({}))],
                    )),
                    "text/event-stream",
                ),
                ResponseTemplate::new(200).set_body_raw(
                    repair_wire(repair_response("resp_second", vec![corrected])),
                    "text/event-stream",
                ),
            ],
            false,
        )
        .await;
        assert!(error.is_some());
        assert_eq!(requests.len(), 2);
        assert!(completed.is_none());
        assert!(!text.contains("response.output_item.done"));
        assert_eq!(
            usage.take_failed_repair_usage().unwrap()["usage"]["total_tokens"],
            24
        );
    }
}

#[test]
fn excel_unknown_repair_rejects_history_bad_shape_and_known_schema_errors() {
    let tools = parse(json!({"tools":[function(json!({"type":"object"}))]})).unwrap();
    let unknown = native("absent", json!({}));
    let valid = repair_response("original", vec![unknown.clone()]);
    assert!(super::repair::unknown_eligible(&tools, &valid));
    for items in [
        vec![unknown.clone(), json!({"type":"message","content":[]})],
        vec![unknown.clone(), unknown.clone()],
        vec![native("read", json!([]))],
        vec![
            json!({"type":"function_call","id":"fc","call_id":"call","name":"absent","arguments":"{}"}),
        ],
    ] {
        assert!(!super::repair::unknown_eligible(
            &tools,
            &repair_response("original", items)
        ));
    }
    let mut incomplete = valid.clone();
    incomplete["status"] = "incomplete".into();
    assert!(!super::repair::unknown_eligible(&tools, &incomplete));
    assert!(!super::repair::unknown_eligible(
        &ClientTools::default(),
        &valid
    ));
    for kind in [
        "function_call",
        "function_call_output",
        "custom_tool_call",
        "custom_tool_call_output",
    ] {
        assert!(!super::repair::no_tool_history(
            json!({"input":[{"type":kind}]}).as_object().unwrap()
        ));
    }
    assert!(super::repair::no_tool_history(
        json!({"input":[{"role":"user","content":"run"}]})
            .as_object()
            .unwrap()
    ));
}

#[tokio::test]
async fn excel_unknown_regeneration_streams_new_message_once() {
    use wiremock::ResponseTemplate;
    let message = |id: &str, text: &str| {
        json!({"type":"message","id":id,"role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":text,"annotations":[]}]})
    };
    let original_message = message("msg_first", "Existing commentary");
    let original = repair_response(
        "original",
        vec![original_message.clone(), native("absent", json!({}))],
    );
    let wire = format!(
        "data: {}\n\n{}",
        json!({"type":"response.output_text.delta","item_id":"msg_first","output_index":0,"content_index":0,"delta":"Existing commentary"}),
        repair_wire(original)
    );
    let (text, error, _, completed, requests) = http_repair(
        vec![
            ResponseTemplate::new(200).set_body_raw(wire, "text/event-stream"),
            ResponseTemplate::new(200).set_body_raw(
                repair_wire(repair_response(
                    "hidden",
                    vec![
                        message("msg_new", "New commentary"),
                        native("read", json!({})),
                    ],
                )),
                "text/event-stream",
            ),
        ],
        false,
    )
    .await;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(requests.len(), 2);
    assert_eq!(completed.unwrap()["output"][0], original_message);
    let deltas: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value["type"] == "response.output_text.delta")
        .collect();
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0]["delta"], "Existing commentary");
    assert_eq!(deltas[1]["delta"], "New commentary");
    assert_eq!(deltas[1]["output_index"], 1);
}

#[tokio::test]
async fn excel_correction_retains_progressive_usage_on_disconnect_without_counting_snapshots_twice()
{
    use wiremock::ResponseTemplate;
    let wire = [3, 7, 5].into_iter().map(|output| format!("data: {}\n\n", json!({"type":"response.in_progress","response":{"id":"hidden","usage":{"input_tokens":20,"output_tokens":output,"total_tokens":20+output}}}))).collect::<String>();
    let (_, error, usage, completed, requests) = http_repair(
        vec![
            ResponseTemplate::new(200).set_body_raw(
                repair_wire(repair_response(
                    "original",
                    vec![native("absent", json!({}))],
                )),
                "text/event-stream",
            ),
            ResponseTemplate::new(200).set_body_raw(wire, "text/event-stream"),
        ],
        false,
    )
    .await;
    assert!(error.is_some());
    assert!(completed.is_none());
    assert_eq!(requests.len(), 2);
    let usage = usage.take_failed_repair_usage().unwrap();
    assert_eq!(usage["usage"]["input_tokens"], 30);
    assert_eq!(usage["usage"]["output_tokens"], 9);
    assert_eq!(usage["usage"]["total_tokens"], 39);
}

#[tokio::test]
async fn excel_unknown_regeneration_blocks_expanded_tool_history_and_keeps_compaction_last() {
    use futures::TryStreamExt;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for history in [false, true] {
        let mut prepared = super::tests::request("https://example.invalid".into(), json!("run"))
            .excel
            .unwrap();
        prepared.tools = parse(json!({"tools":[function(json!({"type":"object"}))]})).unwrap();
        let input = prepared
            .body
            .get_mut("input")
            .unwrap()
            .as_array_mut()
            .unwrap();
        if history {
            input.push(native("read", json!({})));
            input.push(
                json!({"type":"function_call_output","call_id":"call_fixture","output":"done"}),
            );
        }
        input.push(json!({"type":"compaction_trigger"}));
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        let sender: super::repair::Sender = Box::new(move |body| {
            observed.fetch_add(1, Ordering::SeqCst);
            let input = body["input"].as_array().unwrap();
            assert_eq!(input.last().unwrap()["type"], "compaction_trigger");
            assert_eq!(input[input.len() - 2]["role"], "developer");
            Box::pin(async move {
                let stream: crate::transport::CodexBackendSseStream =
                    Box::pin(futures::stream::iter([Ok(Bytes::from(repair_wire(
                        repair_response("corrected", vec![native("read", json!({}))]),
                    )))]));
                Ok(stream)
            })
        });
        let source = Box::pin(futures::stream::iter([Ok(Bytes::from(repair_wire(
            repair_response("original", vec![native("absent", json!({}))]),
        )))]));
        let result = super::transform_stream_with_repair(source, &prepared, Some(sender))
            .try_collect::<Vec<_>>()
            .await;
        assert_eq!(result.is_ok(), !history);
        assert_eq!(count.load(Ordering::SeqCst), usize::from(!history));
    }
}

#[tokio::test]
async fn excel_correction_sums_usage_aliases_without_double_counting() {
    use wiremock::ResponseTemplate;
    let mut first = repair_response("resp_first", vec![repair_call("bad", "Run", "text(1)")]);
    first["usage"] = json!({"prompt_tokens":20,"completion_tokens":4,
        "prompt_tokens_details":{"cached_tokens":3},
        "cache_creation_input_tokens":2,
        "completion_tokens_details":{"reasoning_tokens":1}});
    let mut second = repair_response(
        "resp_second",
        vec![repair_call("fixed", "cpr.custom/exec", "text(1)")],
    );
    second["usage"] = json!({"input_tokens":10,"prompt_tokens":10,"output_tokens":2,"total_tokens":12,
        "input_tokens_details":{"cached_tokens":1,"cache_write_tokens":3},
        "cache_creation_input_tokens":3,"output_tokens_details":{"reasoning_tokens":1}});
    let (_, error, _, completed, requests) = http_repair(
        vec![
            ResponseTemplate::new(200).set_body_raw(repair_wire(first), "text/event-stream"),
            ResponseTemplate::new(200).set_body_raw(repair_wire(second), "text/event-stream"),
        ],
        false,
    )
    .await;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(requests.len(), 2);
    let usage = &completed.unwrap()["usage"];
    assert_eq!(usage["input_tokens"], 30);
    assert_eq!(usage["output_tokens"], 6);
    assert_eq!(usage["total_tokens"], 36);
    assert_eq!(usage["input_tokens_details"]["cached_tokens"], 4);
    assert_eq!(usage["input_tokens_details"]["cache_write_tokens"], 5);
    assert_eq!(usage["output_tokens_details"]["reasoning_tokens"], 2);
}

#[tokio::test]
async fn excel_correction_cache_contains_only_the_actual_delivered_payload() {
    use futures::TryStreamExt;
    use std::sync::Arc;
    for succeeds in [false, true] {
        let code = "  text('$1');\r\n";
        let source = json!({"model":super::tests::VERIFIED_MODEL,"input":"run",
            "tools":[{"type":"custom","name":"exec"}]});
        let store = Arc::new(super::tests::MemoryReplay::default());
        let restored = super::replay::restore(
            store.clone(),
            "owner".into(),
            "thread".into(),
            None,
            source.as_object().unwrap(),
        )
        .await
        .unwrap();
        let mut prepared = super::tests::request("https://example.invalid".into(), json!("run"))
            .excel
            .unwrap();
        prepared.tools = restored.tools;
        prepared.body = super::prepare_request(
            source.as_object().unwrap(),
            &prepared.tools,
            &Default::default(),
            None,
        )
        .unwrap();
        prepared.replay = Some(restored.capture);
        let source: crate::transport::CodexBackendSseStream =
            Box::pin(futures::stream::iter(vec![Ok(Bytes::from(repair_wire(
                repair_response("resp_first", vec![repair_call("bad", "Run", code)]),
            )))]));
        let sender: super::repair::Sender = Box::new(move |_| {
            Box::pin(async move {
                if !succeeds {
                    return Err(super::repair::invalid("fixture"));
                }
                let stream: crate::transport::CodexBackendSseStream = Box::pin(
                    futures::stream::iter(vec![Ok(Bytes::from(repair_wire(repair_response(
                        "resp_hidden",
                        vec![repair_call("fixed", "cpr.custom/exec", "text('changed')")],
                    ))))]),
                );
                Ok(stream)
            })
        });
        let result = super::transform_stream_with_repair(source, &prepared, Some(sender))
            .try_collect::<Vec<_>>()
            .await;
        assert_eq!(result.is_ok(), succeeds);
        let next = super::replay::restore(
            store,
            "owner".into(),
            "thread".into(),
            Some("resp_first"),
            json!({"input":[],"tools":[{"type":"custom","name":"exec"}]})
                .as_object()
                .unwrap(),
        )
        .await;
        if succeeds {
            let next = next.unwrap();
            assert!(!next.native_calls.contains_key("bad"));
            let call = &next.native_calls["fixed"];
            assert_eq!(next.tools.convert_call(call).unwrap()["input"], code);
        } else {
            assert!(next.is_err());
        }
    }
}

#[tokio::test]
async fn excel_structured_output_never_starts_tool_correction() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let source = json!({"model":super::tests::VERIFIED_MODEL,"input":"run",
        "tools":[{"type":"custom","name":"exec"}],"text":{"format":{"type":"json_object"}}});
    let mut prepared = super::tests::request("https://example.invalid".into(), json!("run"))
        .excel
        .unwrap();
    prepared.tools = parse(source.clone()).unwrap();
    prepared.structured = super::StructuredOutput::parse(source.as_object().unwrap()).unwrap();
    let called = Arc::new(AtomicUsize::new(0));
    let count = called.clone();
    let sender: super::repair::Sender = Box::new(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(super::repair::invalid("unexpected correction")) })
    });
    let source: crate::transport::CodexBackendSseStream =
        Box::pin(futures::stream::iter(vec![Ok(Bytes::from(repair_wire(
            repair_response("resp_first", vec![repair_call("bad", "Run", "text(1)")]),
        )))]));
    let mut stream = super::transform_stream_with_repair(source, &prepared, Some(sender));
    let mut failed = false;
    while let Some(chunk) = stream.next().await {
        if chunk.is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed);
    assert_eq!(called.load(Ordering::SeqCst), 0);
    assert!(!prepared.usage.repair_started());
}

#[test]
fn transport_namespace_never_changes_the_declared_client_target() {
    let tools = parse(json!({"tools":[function(json!({"type":"object"})),
        {"type":"namespace","name":"workspace","tools":[function(json!({"type":"object"}))]},
        {"type":"namespace","name":"functions","tools":[function(json!({"type":"object"}))]}
    ]}))
    .unwrap();
    for target in ["read", "workspace.read", "functions.read"] {
        let mut call = native(target, json!({}));
        let expected = tools.convert_call(&call).unwrap();
        for outer_name in ["run_officejs", "functions.run_officejs"] {
            for namespace in ["functions", "unrelated"] {
                call["name"] = outer_name.into();
                call["namespace"] = namespace.into();
                assert_eq!(tools.convert_call(&call), Ok(expected.clone()));
            }
        }
    }
    let direct = json!({"type":"function_call","call_id":"direct","name":"read",
        "namespace":"workspace","arguments":"{}"});
    assert_eq!(
        tools.convert_call(&direct).unwrap()["namespace"],
        "workspace"
    );
    let mut wrong = direct;
    wrong["namespace"] = "unrelated".into();
    assert!(tools.convert_call(&wrong).is_err());
    wrong["name"] = "functions.read".into();
    assert!(tools.convert_call(&wrong).is_err());
}

#[test]
fn function_schemas_enforce_references_composition_and_constraints() {
    let cases = [
        (
            json!({"type":"object","oneOf":[{"required":["path"]},{"required":["uri"]}]}),
            json!({"path":"x"}),
            json!({}),
        ),
        (
            json!({"type":"object","properties":{"path":{"$ref":"#/$defs/path"}},"$defs":{"path":{"type":"string"}},"required":["path"]}),
            json!({"path":"x"}),
            json!({"path":17}),
        ),
        (
            json!({"type":"object","properties":{"limit":{"type":"integer","minimum":0,"maximum":10}}}),
            json!({"limit":5}),
            json!({"limit":-1}),
        ),
        (
            json!({"type":"object","additionalProperties":false}),
            json!({}),
            json!({"extra":1}),
        ),
        (
            json!({"type":"object","properties":{"path":{"type":"string","pattern":"^safe/"}}}),
            json!({"path":"safe/x"}),
            json!({"path":"bad/x"}),
        ),
        (
            json!({"type":"object","properties":{"kind":{"const":"read"}}}),
            json!({"kind":"read"}),
            json!({"kind":"write"}),
        ),
        (
            json!({"type":"object","anyOf":[{"required":["path"]},{"required":["uri"]}]}),
            json!({"uri":"x"}),
            json!({}),
        ),
        (
            json!({"type":"object","allOf":[{"required":["path"]},{"required":["limit"]}]}),
            json!({"path":"x","limit":1}),
            json!({"path":"x"}),
        ),
    ];
    for (schema, valid, invalid) in cases {
        let tools = parse(json!({"tools":[function(schema.clone())]})).unwrap();
        assert!(
            tools.convert_call(&native("read", valid)).is_ok(),
            "{schema}"
        );
        assert!(
            tools.convert_call(&native("read", invalid)).is_err(),
            "{schema}"
        );
    }
    let tools = parse(json!({"tools":[function(json!(false))]})).unwrap();
    assert!(tools.convert_call(&native("read", json!({}))).is_err());
    let tools = parse(json!({"tools":[function(json!(true))]})).unwrap();
    assert!(tools.convert_call(&native("read", json!({}))).is_ok());
}

#[test]
fn function_schemas_reject_invalid_external_and_oversized_schemas() {
    for schema in [
        json!({"type":"unknown"}),
        json!({"required":"path"}),
        json!({"$ref":"https://example.invalid/schema.json"}),
        json!({"$ref":"file:///unavailable-schema.json"}),
        json!({"description":"x".repeat(1024 * 1024)}),
    ] {
        assert!(parse(json!({"tools":[function(schema)]})).is_err());
    }
}

#[test]
fn required_and_named_choices_enforce_cardinality_and_exact_identity() {
    let catalog = json!([function(json!({})), {"type":"function","name":"write"},
        {"type":"custom","name":"patch"},
        {"type":"namespace","name":"workspace","tools":[function(json!({}))]}]);
    let required = parse(json!({"tool_choice":"required","tools":catalog})).unwrap();
    assert!(required.instructions().contains("at least one"));
    assert!(required.reminder().unwrap().contains("at least one"));
    assert!(required.validate_call_count(0).is_err());
    assert!(required.validate_call_count(2).is_ok());
    let serial =
        parse(json!({"tool_choice":"required","parallel_tool_calls":false,"tools":catalog}))
            .unwrap();
    assert!(serial.validate_call_count(1).is_ok());
    assert!(serial.validate_call_count(2).is_err());
    for choice in [
        json!({"type":"function","name":"workspace.read"}),
        json!({"type":"function","name":"read","namespace":"workspace"}),
    ] {
        let tools = parse(json!({"tool_choice":choice,"tools":catalog})).unwrap();
        assert!(
            tools
                .instructions()
                .contains("exactly one call to catalog tool workspace.read")
        );
        assert!(
            tools
                .convert_call(&native("workspace.read", json!({})))
                .is_ok()
        );
        assert!(tools.convert_call(&native("read", json!({}))).is_err());
        assert!(tools.validate_call_count(0).is_err());
        assert!(tools.validate_call_count(1).is_ok());
        assert!(tools.validate_call_count(2).is_err());
    }
    let tools =
        parse(json!({"tool_choice":{"type":"custom","name":"patch"},"tools":catalog})).unwrap();
    let raw = "*** Begin Patch\n*** End Patch\n";
    let call = json!({"type":"function_call","call_id":"patch_fixture","name":"run_officejs",
        "arguments":json!({"summary":"cpr.custom/patch","code":raw}).to_string()});
    assert_eq!(tools.convert_call(&call).unwrap()["input"], raw);
    assert!(tools.convert_call(&native("read", json!({}))).is_err());
}

#[test]
fn tool_choice_never_authorizes_undeclared_or_hosted_tools() {
    for source in [
        json!({"tool_choice":"required"}),
        json!({"tool_choice":"required","tools":[{"type":"web_search"}]}),
        json!({"tool_choice":{"type":"web_search"},"tools":[{"type":"web_search"}]}),
        json!({"tool_choice":{"type":"function","name":"absent"},"tools":[function(json!({}))]}),
        json!({"tool_choice":{"type":"custom","name":"read"},"tools":[function(json!({}))]}),
        json!({"tool_choice":{"type":"function","name":"read","namespace":17},"tools":[function(json!({}))]}),
        json!({"tool_choice":{"type":"function","name":"read","unexpected":true},"tools":[function(json!({}))]}),
    ] {
        assert!(parse(source).is_err());
    }
    let tools = parse(json!({"tool_choice":"none","tools":[function(json!({}))]})).unwrap();
    assert!(tools.convert_call(&native("read", json!({}))).is_err());
    assert!(tools.validate_call_count(0).is_ok());
}

async fn relay(tools: ClientTools, output: Value) -> (String, bool) {
    let prepared = ExcelPreparedRequest {
        body: Default::default(),
        tools,
        structured: None,
        _image_lease: None,
        completed: Default::default(),
        usage: Default::default(),
        replay: None,
        endpoint: "https://example.invalid".into(),
    };
    let event = json!({"type":"response.completed","response":{
        "id":"resp_fixture","status":"completed","tool_choice":"auto","output":output
    }});
    let source = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(format!(
        "data: {event}\n\n"
    )))]));
    let mut stream = transform_stream(source, &prepared);
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => text.push_str(std::str::from_utf8(&bytes).unwrap()),
            Err(_) => return (text, false),
        }
    }
    (text, true)
}

#[tokio::test]
async fn forced_tool_completion_validates_before_delivering_calls() {
    let source = json!({"tool_choice":{"type":"function","name":"read"},"tools":[
        function(json!({"type":"object","required":["limit"],"properties":{"limit":{"type":"integer","minimum":1}}})),
        {"type":"function","name":"write"}
    ]});
    let mut valid = native("read", json!({"limit":1}));
    valid["namespace"] = "functions".into();
    let (text, ok) = relay(parse(source.clone()).unwrap(), json!([valid.clone()])).await;
    assert!(ok);
    assert!(text.contains("response.function_call_arguments.done"));
    assert!(text.contains("\"tool_choice\":{\"type\":\"function\",\"name\":\"read\"}"));
    for output in [
        json!([]),
        json!([native("write", json!({}))]),
        json!([native("read", json!({"limit":0}))]),
        json!([valid.clone(), valid]),
    ] {
        let (text, ok) = relay(parse(source.clone()).unwrap(), output).await;
        assert!(!ok);
        assert!(!text.contains("response.function_call_arguments"));
        assert!(!text.contains("response.completed"));
    }
}

#[tokio::test]
async fn required_tool_allows_explicit_refusal_but_not_silent_text_substitution() {
    let source = json!({"tool_choice":"required","tools":[function(json!({}))]});
    for (content, expected) in [
        (
            json!([{"type":"refusal","refusal":"I cannot help with that request."}]),
            true,
        ),
        (
            json!([{"type":"output_text","text":"No tool was used."}]),
            false,
        ),
        (json!([{"type":"refusal","refusal":""}]), false),
    ] {
        let (_, ok) = relay(
            parse(source.clone()).unwrap(),
            json!([
                {"type":"message","role":"assistant","id":"msg_fixture","content":content}
            ]),
        )
        .await;
        assert_eq!(ok, expected);
    }
}
