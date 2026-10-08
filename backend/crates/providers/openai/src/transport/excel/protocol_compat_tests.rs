use super::*;
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

fn declarations() -> Value {
    json!([
        {"type":"function","name":"read","parameters":{"type":"object"}},
        {"type":"namespace","name":"files","tools":[
            {"type":"function","name":"read","parameters":{"type":"object"}},
            {"type":"custom","name":"patch"}
        ]}
    ])
}

fn allowed(mode: &str) -> Value {
    json!({"type":"allowed_tools","mode":mode,"tools":[
        {"type":"function","name":"read"},
        {"type":"custom","namespace":"files","name":"patch"}
    ]})
}

fn source_with_choice(mode: &str) -> Value {
    json!({"tools":declarations(),"tool_choice":allowed(mode),"input":"fixture"})
}

fn call(name: &str) -> Value {
    json!({"type":"function_call","name":name,"call_id":"call_fixture","arguments":"{}"})
}

#[test]
fn discovered_tools_are_collected_without_forwarding_discovery_history() {
    for status in [None, Some(json!(null)), Some(json!("completed"))] {
        let mut discovery = json!({"type":"tool_search_output","tools":declarations()});
        if let Some(status) = status {
            discovery["status"] = status;
        }
        let source =
            json!({"model":"gpt-5.6-sol","input":[discovery,{"role":"user","content":"fixture"}]});
        let original = source.clone();
        let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
        assert!(tools.contains("read"));
        assert!(tools.contains("files.patch"));
        let wire =
            prepare_request(source.as_object().unwrap(), &tools, &BTreeMap::new(), None).unwrap();
        assert!(
            !wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "tool_search_output")
        );
        assert_eq!(source, original);
    }
}

#[tokio::test]
async fn discovered_catalog_persists_only_within_its_owner_and_session() {
    let store = tests::MemoryReplay::default();
    let source = json!({"input":[{"type":"tool_search_output","status":"completed","tools":declarations()}]});
    let (tools, catalog) = catalog::resolve(
        &store,
        "owner",
        Some("session"),
        None,
        source.as_object().unwrap(),
    )
    .await
    .unwrap();
    assert!(tools.contains("files.patch"));
    for (owner, session, found) in [
        ("owner", "session", true),
        ("other", "session", false),
        ("owner", "other", false),
    ] {
        let (tools, stored) = catalog::resolve(
            &store,
            owner,
            Some(session),
            None,
            json!({"input":"next"}).as_object().unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(tools.contains("files.patch"), found);
        if found {
            assert_eq!(stored, catalog);
        }
    }
}

#[test]
fn failed_discovery_never_registers_tools_and_is_not_forwarded() {
    for status in [
        json!("failed"),
        json!("in_progress"),
        json!("incomplete"),
        json!(1),
    ] {
        let source = json!({"model":"gpt-5.6-sol","input":[
            {"type":"tool_search_output","status":status,"tools":declarations()},
            {"role":"user","content":"fixture"}
        ]});
        let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
        assert!(!tools.has_client_tools());
        let wire =
            prepare_request(source.as_object().unwrap(), &tools, &BTreeMap::new(), None).unwrap();
        assert!(
            !wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "tool_search_output")
        );
    }
}

#[test]
fn allowed_tools_enforce_exact_names_types_and_required_mode() {
    for mode in ["auto", "required"] {
        let source = source_with_choice(mode);
        let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
        assert!(tools.convert_call(&call("read")).is_ok());
        let mut other = call("read");
        other["namespace"] = "files".into();
        assert_eq!(tools.convert_call(&other), Err(ExcelRequestError::ToolCall));
        let custom = json!({"type":"custom_tool_call","name":"patch","namespace":"files","call_id":"patch_fixture","input":" raw\n patch "});
        assert_eq!(
            tools.convert_call(&custom).unwrap()["input"],
            custom["input"]
        );
        assert_eq!(tools.validate_call_count(0).is_ok(), mode == "auto");
        assert!(tools.validate_call_count(2).is_ok());
        assert!(tools.rebuild_history_call(&other).is_ok());
        assert!(tools.contains("files.read"));
        let mut response = json!({});
        tools.project_choice(&mut response);
        assert_eq!(response["tool_choice"], allowed(mode));
        let mut serial = source.clone();
        serial["parallel_tool_calls"] = false.into();
        assert!(
            ClientTools::parse(serial.as_object().unwrap())
                .unwrap()
                .validate_call_count(2)
                .is_err()
        );
    }
}

#[test]
fn malformed_or_undeclared_allowed_choices_are_rejected() {
    let mut choices = vec![
        json!({"type":"allowed_tools","mode":"required","tools":[]}),
        json!({"type":"allowed_tools","mode":"unknown","tools":[]}),
        json!({"type":"allowed_tools","mode":"auto","tools":null}),
        json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"missing"}]}),
        json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"custom","name":"read"}]}),
    ];
    let mut unknown = allowed("auto");
    unknown["unrecognized"] = true.into();
    choices.push(unknown);
    for choice in choices {
        let source = json!({"tools":declarations(),"tool_choice":choice});
        assert!(matches!(
            ClientTools::parse(source.as_object().unwrap()),
            Err(ExcelRequestError::Tool)
        ));
    }
}

fn prepared(source: Value) -> ExcelPreparedRequest {
    ExcelPreparedRequest {
        image_policy: None,
        exit_lease: None,
        body: Default::default(),
        tools: ClientTools::parse(source.as_object().unwrap()).unwrap(),
        structured: None,
        _image_lease: None,
        image_limits: Default::default(),
        completed: Default::default(),
        usage: Default::default(),
        replay: None,
        endpoint: RESPONSES_URL.into(),
    }
}

fn completed() -> Value {
    json!({"id":"resp_fixture","model":"gpt-5.6-sol","status":"completed","output":[],
        "usage":{"input_tokens":12,"output_tokens":3}})
}

#[tokio::test]
async fn terminal_aliases_and_json_complete_before_upstream_eof() {
    let mut inputs = vec![completed().to_string()];
    for alias in ["response.completed", "response.done", "done", "completed"] {
        inputs.push(format!(
            "data: {}\n\n",
            json!({"type":alias,"response":completed()})
        ));
        inputs.push(format!(
            "event: {alias}\r\ndata: {}\r\n\r\n",
            json!({"response":completed()})
        ));
    }
    for input in inputs {
        let prepared = prepared(json!({}));
        let chunks: Vec<_> = input
            .as_bytes()
            .chunks(3)
            .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
            .collect();
        let source = futures::stream::iter(chunks).chain(futures::stream::pending());
        let output = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            transform_stream(Box::pin(source), &prepared).try_collect::<Vec<_>>(),
        )
        .await
        .expect("a complete response must not wait for connection close")
        .unwrap();
        let output = String::from_utf8(output.concat()).unwrap();
        assert!(output.contains("response.completed"));
        let mut expected = completed();
        expected["parallel_tool_calls"] = true.into();
        assert_eq!(*prepared.completed.lock().unwrap(), Some(expected));
    }
}

#[tokio::test]
async fn terminal_compatibility_never_turns_failures_or_truncation_into_success() {
    let mut failed = completed();
    failed["error"] = json!({"code":"rate_limit_exceeded"});
    let inputs = [
        ": heartbeat\n\ndata: [DONE]\n\n".into(),
        "{\"id\":\"resp_fixture\",\"output\":[".into(),
        json!({"id":"resp_fixture","status":"in_progress","output":[]}).to_string(),
        json!({"id":"resp_fixture","status":"completed"}).to_string(),
        format!(
            "data: {}\n\n",
            json!({"type":"response.done","response":failed})
        ),
        format!(
            "event: error\ndata: {}\n\n",
            json!({"type":"response.done","response":completed()})
        ),
    ];
    for input in inputs {
        let prepared = prepared(json!({}));
        let output = transform_stream(
            Box::pin(futures::stream::iter([Ok(Bytes::from(input))])),
            &prepared,
        )
        .try_collect::<Vec<_>>()
        .await;
        assert!(prepared.completed.lock().unwrap().is_none());
        if let Ok(chunks) = output {
            assert!(
                !String::from_utf8(chunks.concat())
                    .unwrap()
                    .contains("\"type\":\"response.completed\"")
            );
        }
    }
}

#[tokio::test]
async fn allowed_selection_does_not_erase_the_next_turn_catalog() {
    let store = Arc::new(tests::MemoryReplay::default());
    let source = source_with_choice("auto");
    let (_, expected) = catalog::resolve(
        store.as_ref(),
        "owner",
        Some("session"),
        None,
        source.as_object().unwrap(),
    )
    .await
    .unwrap();
    let (tools, stored) = catalog::resolve(
        store.as_ref(),
        "owner",
        Some("session"),
        None,
        json!({"input":"next"}).as_object().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(stored, expected);
    let mut next = call("read");
    next["namespace"] = "files".into();
    assert!(tools.convert_call(&next).is_ok());
}

#[test]
fn allowed_selection_checks_wrapped_calls_and_preserves_schema_and_refusal_rules() {
    let mut source = source_with_choice("required");
    source["tools"][0]["parameters"] =
        json!({"type":"object","properties":{"count":{"type":"integer"}},"required":["count"]});
    let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
    for (name, args, accepted) in [
        ("read", json!({"count":1}), true),
        ("read", json!({"count":"bad"}), false),
        ("files.read", json!({"count":1}), false),
    ] {
        let wrapped = json!({"type":"function_call","name":"run_officejs","call_id":"wrapped",
            "arguments":json!({"code":json!({"name":name,"arguments":args}).to_string()}).to_string()});
        assert_eq!(tools.convert_call(&wrapped).is_ok(), accepted);
    }
    let custom =
        tool_compat_tests::repair_call("patch", "codex2api.custom/files.patch", " raw\npatch ");
    assert_eq!(
        tools.convert_call(&custom).unwrap()["input"],
        " raw\npatch "
    );
    let refusal = json!({"type":"message","content":[{"type":"refusal","refusal":"Cannot perform this request"}]});
    assert!(tools.validate_completion(&[refusal]).is_ok());
    assert!(tools.validate_completion(&[]).is_err());
    source["tool_choice"] = json!({"type":"allowed_tools","mode":"auto","tools":[]});
    let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
    assert!(tools.validate_completion(&[]).is_ok());
    assert!(tools.convert_call(&custom).is_err());
    assert!(tools.contains("files.patch"));
}

#[tokio::test]
async fn allowed_tool_batches_fail_atomically_even_when_alias_is_completed() {
    let prepared = prepared(source_with_choice("auto"));
    let mut denied = call("read");
    denied["namespace"] = "files".into();
    denied["call_id"] = "denied".into();
    let mut response = completed();
    response["output"] = json!([call("read"), denied]);
    let wire = format!(
        "data: {}\n\n",
        json!({"type":"response.done","response":response})
    );
    let mut stream = transform_stream(
        Box::pin(futures::stream::iter([Ok(Bytes::from(wire))])),
        &prepared,
    );
    assert!(stream.next().await.unwrap().is_err());
    assert!(prepared.completed.lock().unwrap().is_none());
}

#[tokio::test]
async fn terminal_stops_before_duplicate_or_malformed_following_events() {
    let prepared = prepared(json!({}));
    let wire = format!(
        "data: {}\n\ndata: {{invalid}}\n\n",
        json!({"type":"response.done","response":completed()})
    );
    let chunks = transform_stream(
        Box::pin(futures::stream::iter([Ok(Bytes::from(wire))])),
        &prepared,
    )
    .try_collect::<Vec<_>>()
    .await
    .unwrap();
    assert_eq!(
        String::from_utf8(chunks.concat())
            .unwrap()
            .matches("\"type\":\"response.completed\"")
            .count(),
        1
    );
}

#[tokio::test]
async fn http_discovery_allowed_choice_and_completion_compatibility_work_together() {
    use tool_compat_tests::{http_repair_source, repair_response};
    for json_body in [false, true] {
        let source = json!({"model":tests::VERIFIED_MODEL,"input":[
            {"type":"tool_search_output","status":"completed","tools":declarations()},
            {"role":"user","content":"run"}],"tool_choice":allowed("required")});
        let response = repair_response("resp_fixture", vec![call("read")]);
        let wire = if json_body {
            response.to_string()
        } else {
            format!(
                "event: response.done\ndata: {}\n\n",
                json!({"response":response})
            )
        };
        let (text, error, _, completed, requests) = http_repair_source(
            vec![wiremock::ResponseTemplate::new(200).set_body_raw(
                wire,
                if json_body {
                    "application/json"
                } else {
                    "text/event-stream"
                },
            )],
            source,
        )
        .await;
        assert!(error.is_none(), "{error:?}");
        assert_eq!(requests.len(), 1);
        let body: Value = requests[0].body_json().unwrap();
        assert!(
            !body["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["type"] == "tool_search_output")
        );
        assert!(body["input"][0].to_string().contains("files.patch"));
        assert!(text.contains("response.function_call_arguments.done"));
        assert_eq!(completed.unwrap()["id"], "resp_fixture");
    }
}

#[tokio::test]
async fn http_aliases_and_json_also_work_in_correction_without_double_usage() {
    use tool_compat_tests::{http_repair_source, repair_call, repair_response};
    for json_correction in [false, true] {
        let source = json!({"model":tests::VERIFIED_MODEL,"input":"run","tools":[{"type":"custom","name":"exec"}],
            "tool_choice":{"type":"allowed_tools","mode":"required","tools":[{"type":"custom","name":"exec"}]}});
        let first = repair_response("resp_first", vec![repair_call("first", "Run", "text(1)")]);
        let fixed = repair_response(
            "resp_second",
            vec![repair_call("fixed", "codex2api.custom/exec", "text(1)")],
        );
        let wire = if json_correction {
            fixed.to_string()
        } else {
            format!("event: completed\ndata: {}\n\n", json!({"response":fixed}))
        };
        let responses = vec![
            wiremock::ResponseTemplate::new(200).set_body_raw(
                format!(
                    "data: {}\n\n",
                    json!({"type":"response.done","response":first})
                ),
                "text/event-stream",
            ),
            wiremock::ResponseTemplate::new(200).set_body_raw(
                wire,
                if json_correction {
                    "application/json"
                } else {
                    "text/event-stream"
                },
            ),
        ];
        let (text, error, _, completed, requests) = http_repair_source(responses, source).await;
        assert!(error.is_none(), "{error:?}");
        assert_eq!(requests.len(), 2);
        for header in [
            "authorization",
            "chatgpt-account-id",
            "x-openai-account-id",
            "user-agent",
            "x-openai-internal-basispoints-client-agent-profile",
        ] {
            assert!(requests[0].headers.get(header).is_some());
            assert_eq!(
                requests[0].headers.get(header),
                requests[1].headers.get(header)
            );
        }
        assert_eq!(requests[0].url, requests[1].url);
        assert_eq!(text.matches("\"type\":\"response.completed\"").count(), 1);
        assert_eq!(
            text.matches("\"type\":\"response.output_item.done\"")
                .count(),
            1
        );
        let completed = completed.unwrap();
        assert_eq!(completed["id"], "resp_first");
        assert_eq!(completed["usage"]["total_tokens"], 24);
    }
}

#[tokio::test]
async fn http_correction_cannot_expand_allowed_subset_or_hide_failure_aliases() {
    use tool_compat_tests::{http_repair_source, repair_call, repair_response};
    for failure_alias in [false, true] {
        let source = json!({"model":tests::VERIFIED_MODEL,"input":"run","tools":[
            {"type":"custom","name":"exec"},{"type":"custom","name":"other"}],
            "tool_choice":{"type":"allowed_tools","mode":"required","tools":[{"type":"custom","name":"exec"}]}});
        let first = repair_response("resp_first", vec![repair_call("first", "Run", "text(1)")]);
        let mut fixed = repair_response(
            "resp_second",
            vec![repair_call("fixed", "codex2api.custom/other", "text(1)")],
        );
        let wire = if failure_alias {
            fixed["error"] =
                json!({"code":"rate_limit_exceeded","message":"private upstream text"});
            format!(
                "event: failed\ndata: {}\n\n",
                json!({"type":"response.done","response":fixed})
            )
        } else {
            fixed.to_string()
        };
        let responses = vec![
            wiremock::ResponseTemplate::new(200)
                .set_body_raw(first.to_string(), "application/json"),
            wiremock::ResponseTemplate::new(200).set_body_raw(wire.clone(), "text/event-stream"),
            wiremock::ResponseTemplate::new(200).set_body_raw(wire, "text/event-stream"),
        ];
        let (text, error, _, completed, requests) = http_repair_source(responses, source).await;
        assert_eq!(requests.len(), if failure_alias { 2 } else { 3 });
        assert!(completed.is_none());
        assert!(!text.contains("response.completed"));
        assert!(!text.contains("response.output_item.done"));
        assert!(!text.contains("private upstream text"));
        if failure_alias {
            assert!(text.contains("rate_limit_exceeded"));
        } else {
            assert!(error.is_some());
        }
    }
}
