use gateway_protocol::openai::chat::{ChatStreamEncoder, chat_response, decode_chat_request};
use serde_json::{Value, json};

// Output contracts follow CPA ac02da6c05e18f465aa7e3ed5b0a65a2f060917d,
// internal/translator/codex/openai/chat-completions/codex_openai_response.go.
// CPR retains refusal, Chat IDs, usage projection and explicit failure/EOF safety.

fn request() -> Value {
    json!({"model":"gpt-test","messages":[{"role":"user","content":"hello"}]})
}

fn message(text: &str) -> Value {
    json!({"type":"message","id":"msg_1","role":"assistant","content":[
        {"type":"output_text","text":text,"annotations":[]}
    ]})
}

fn function(id: &str, name: &str, arguments: &str) -> Value {
    json!({"type":"function_call","id":format!("fc_{id}"),"call_id":id,"name":name,"arguments":arguments})
}

fn response(output: Value) -> Value {
    json!({"id":"resp_test","model":"gpt-test","created_at":123,"status":"completed","output":output,
        "usage":{"input_tokens":20,"output_tokens":8,"total_tokens":28,
            "input_tokens_details":{"cached_tokens":12},"output_tokens_details":{"reasoning_tokens":3}}
    })
}

fn push(encoder: &mut ChatStreamEncoder, event: Value) -> Vec<Value> {
    encoder.push(None, &event).expect("valid event")
}

fn created(encoder: &mut ChatStreamEncoder) -> Vec<Value> {
    push(
        encoder,
        json!({"type":"response.created","response":{"id":"resp_test","model":"gpt-test","created_at":123}}),
    )
}

fn text(chunks: &[Value], field: &str) -> String {
    chunks
        .iter()
        .filter_map(|v| v["choices"][0]["delta"][field].as_str())
        .collect()
}

#[test]
fn request_defaults_and_transport_remain_independent() {
    let decoded = decode_chat_request(request()).unwrap();
    assert!(!decoded.stream);
    assert!(!decoded.include_usage);
    assert_eq!(decoded.responses["stream"], false);
    assert_eq!(
        decoded.responses["input"][0]["content"][0],
        json!({"type":"input_text","text":"hello"})
    );
    for stream in [false, true] {
        for transport in [false, true] {
            let mut input = request();
            input["stream"] = json!(stream);
            input["use_websocket"] = json!(transport);
            let decoded = decode_chat_request(input).unwrap();
            assert_eq!(decoded.stream, stream);
            assert_eq!(decoded.responses["use_websocket"], transport);
        }
    }
}

#[test]
fn request_preserves_user_files_without_fetching_or_reinterpreting_them() {
    for file in [
        json!({"file_id":"file_synthetic"}),
        json!({"file_data":"data:application/pdf;base64,c3ludGhldGlj","filename":"example.pdf"}),
    ] {
        let mut input = request();
        input["messages"][0]["content"] = json!([
            {"type":"text","text":"summarize"},
            {"type":"file","file":file}
        ]);
        let decoded = decode_chat_request(input.clone()).unwrap();
        let mut expected = file;
        expected["type"] = json!("input_file");
        assert_eq!(decoded.responses["input"][0]["content"][1], expected);
        assert_eq!(input["messages"][0]["content"][1]["type"], "file");
    }
    for file in [
        json!({}),
        json!({"filename":"example.pdf"}),
        json!({"file_id":""}),
        json!({"file_id":12}),
        json!({"file_id":"file_synthetic","file_data":"ambiguous"}),
        json!({"file_id":"file_synthetic","unknown":"not dropped"}),
    ] {
        let mut input = request();
        input["messages"][0]["content"] = json!([{"type":"file","file":file}]);
        assert!(decode_chat_request(input).is_err());
    }
    for role in ["assistant", "system", "developer"] {
        let mut input = request();
        input["messages"][0] = json!({"role":role,"content":[
            {"type":"file","file":{"file_id":"file_synthetic"}}
        ]});
        assert!(decode_chat_request(input).is_err());
    }
}

#[test]
fn request_public_reasoning_is_plain_assistant_history_not_encrypted_state() {
    for field in ["reasoning_content", "reasoning"] {
        for content in [Value::Null, json!("answer")] {
            let mut input = request();
            let mut assistant = json!({"role":"assistant","content":content});
            assistant[field] = json!("public explanation");
            input["messages"] = json!([assistant, {"role":"user","content":"continue"}]);
            let decoded = decode_chat_request(input).unwrap();
            let history = &decoded.responses["input"][0];
            assert_eq!(history["type"], "message");
            assert_eq!(history["status"], "completed");
            assert_eq!(
                history["content"][0],
                json!({"type":"output_text","text":"public explanation"})
            );
            assert!(history.get("encrypted_content").is_none());
            if !content.is_null() {
                assert_eq!(history["content"][1]["text"], "answer");
            }
        }
    }
    for message in [
        json!({"role":"user","content":"hi","reasoning_content":"invalid role"}),
        json!({"role":"assistant","content":"hi","reasoning_content":{}}),
        json!({"role":"assistant","content":"hi","reasoning_content":"a","reasoning":"b"}),
    ] {
        let mut input = request();
        input["messages"] = json!([message]);
        assert!(decode_chat_request(input).is_err());
    }
}

#[test]
fn request_maps_parameters_schema_and_stream_options() {
    let mut input = request();
    input["stream"] = json!(true);
    input["stream_options"] = json!({"include_usage":true});
    input["max_completion_tokens"] = json!(512);
    input["reasoning_effort"] = json!("high");
    input["parallel_tool_calls"] = json!(false);
    input["temperature"] = json!(0.7);
    input["top_p"] = json!(0.9);
    input["metadata"] = json!({"trace":"test"});
    input["response_format"] = json!({"type":"json_schema","json_schema":{
        "name":"result","schema":{"type":"object","properties":{}},"strict":true
    }});
    input["verbosity"] = json!("low");
    let decoded = decode_chat_request(input).unwrap();
    assert!(decoded.stream && decoded.include_usage);
    assert!(!decoded.responses.contains_key("stream_options"));
    assert_eq!(decoded.responses["max_output_tokens"], 512);
    assert_eq!(decoded.responses["reasoning"], json!({"effort":"high"}));
    assert_eq!(decoded.responses["text"]["format"]["name"], "result");
    assert_eq!(decoded.responses["text"]["format"]["type"], "json_schema");
    assert_eq!(decoded.responses["text"]["verbosity"], "low");
    assert_eq!(decoded.responses["parallel_tool_calls"], false);
    assert_eq!(decoded.responses["metadata"]["trace"], "test");
}

#[test]
fn request_keeps_all_roles_text_with_calls_and_newapi_tool_name() {
    let input = json!({"model":"gpt-test","messages":[
        {"role":"system","content":"system"},
        {"role":"developer","content":[{"type":"text","text":"developer"}]},
        {"role":"user","content":"question"},
        {"role":"assistant","content":"checking","tool_calls":[
            {"id":"call_1","type":"function","function":{"name":"one","arguments":"{\"x\":1}"}},
            {"id":"call_2","type":"function","function":{"name":"two","arguments":"{}"}}
        ]},
        {"role":"tool","tool_call_id":"call_2","name":"two","content":"second"},
        {"role":"tool","tool_call_id":"call_1","content":"first"},
        {"role":"assistant","content":"done"}
    ]});
    let decoded = decode_chat_request(input).unwrap();
    let items = decoded.responses["input"].as_array().unwrap();
    assert_eq!(items.len(), 9);
    assert_eq!(items[0]["role"], "system");
    assert_eq!(items[1]["role"], "developer");
    assert_eq!(items[3]["content"][0]["text"], "checking");
    assert_eq!(items[4]["call_id"], "call_1");
    assert_eq!(items[4]["arguments"], "{\"x\":1}");
    assert_eq!(
        items[6],
        json!({"type":"function_call_output","call_id":"call_2","output":"second"})
    );
}

#[test]
fn request_converts_images_and_function_tools_without_strict_default_change() {
    let input = json!({"model":"gpt-test","messages":[{"role":"user","content":[
        {"type":"text","text":"describe"},
        {"type":"image_url","image_url":{"url":"data:image/png;base64,AA==","detail":"high"}}
    ]}],"tools":[{"type":"function","function":{"name":"inspect","parameters":{
        "type":"object","properties":{"label":{"type":"string"}}
    }}}],"tool_choice":{"type":"function","function":{"name":"inspect"}}});
    let decoded = decode_chat_request(input).unwrap();
    assert_eq!(
        decoded.responses["input"][0]["content"][1],
        json!({
            "type":"input_image","image_url":"data:image/png;base64,AA==","detail":"high"
        })
    );
    assert_eq!(decoded.responses["tools"][0]["strict"], false);
    assert_eq!(decoded.responses["tools"][0]["name"], "inspect");
    assert_eq!(
        decoded.responses["tool_choice"],
        json!({"type":"function","name":"inspect"})
    );
}

#[test]
fn request_maps_newapi_web_search_options_and_direct_text_format() {
    let mut input = request();
    input["web_search_options"] = json!({"search_context_size":"low","user_location":{
        "type":"approximate","approximate":{"country":"US","city":"New York"}
    }});
    input["text"] = json!({"format":{"type":"json_object"}});
    let decoded = decode_chat_request(input).unwrap();
    assert_eq!(decoded.responses["tools"][0]["type"], "web_search");
    assert_eq!(
        decoded.responses["tools"][0]["user_location"],
        json!({
            "type":"approximate","country":"US","city":"New York"
        })
    );
    assert_eq!(decoded.responses["text"]["format"]["type"], "json_object");
}

#[test]
fn request_rejects_semantic_loss_and_returns_safe_parameter_errors() {
    for (key, value) in [
        ("n", json!(2)),
        ("audio", json!({"voice":"secret-body"})),
        ("modalities", json!(["text", "audio"])),
        ("logprobs", json!(true)),
        ("top_logprobs", json!(3)),
        ("frequency_penalty", json!(1.0)),
        ("presence_penalty", json!(1.0)),
        ("stop", json!(["secret-body"])),
        ("seed", json!(1)),
        ("prediction", json!({"content":"secret-body"})),
        ("logit_bias", json!({"123":1})),
        ("top_k", json!(40)),
    ] {
        let mut input = request();
        input[key] = value;
        let error = decode_chat_request(input).unwrap_err();
        assert_eq!(error.param(), Some(key));
        assert_eq!(error.code(), "unsupported_parameter");
        assert!(!error.to_string().contains("secret-body"));
    }
    for input in [
        json!([]),
        json!({"model":"gpt-test","messages":[]}),
        json!({"model":"gpt-test","messages":[{"role":"user","content":[{"type":"input_audio","input_audio":{"data":"secret-body"}}]}]}),
        json!({"model":"gpt-test","messages":[{"role":"tool","tool_call_id":"missing","content":"secret-body"}]}),
    ] {
        let error = decode_chat_request(input).unwrap_err();
        assert!(error.param().is_some());
        assert!(!error.to_string().contains("secret-body"));
    }
}

#[test]
fn request_rejects_malformed_common_parameters_and_ambiguous_mapping() {
    for (key, value) in [
        ("stream", json!("true")),
        ("temperature", json!(3)),
        ("top_p", json!(-1)),
        ("max_tokens", json!(0)),
        ("max_completion_tokens", json!("512")),
        ("parallel_tool_calls", json!("false")),
        ("reasoning_effort", json!("unknown")),
        (
            "tool_choice",
            json!({"type":"function","function":{"name":"missing"}}),
        ),
        ("stream_options", json!({"include_usage":true})),
        (
            "response_format",
            json!({"type":"json_schema","json_schema":{"name":"x"}}),
        ),
    ] {
        let mut input = request();
        input[key] = value;
        assert!(decode_chat_request(input).is_err(), "{key}");
    }
    let mut input = request();
    input["max_tokens"] = json!(10);
    input["max_completion_tokens"] = json!(10);
    assert_eq!(
        decode_chat_request(input).unwrap_err().param(),
        Some("max_completion_tokens")
    );
}

#[test]
fn response_preserves_text_refusal_tools_usage_and_finish_reasons() {
    let complete = chat_response(&response(json!([message("hello")]))).unwrap();
    assert_eq!(complete["id"], "chatcmpl-test");
    assert_eq!(complete["choices"][0]["message"]["content"], "hello");
    assert_eq!(complete["choices"][0]["finish_reason"], "stop");
    assert_eq!(
        complete["usage"]["prompt_tokens_details"]["cached_tokens"],
        12
    );
    assert_eq!(
        complete["usage"]["completion_tokens_details"]["reasoning_tokens"],
        3
    );
    let tools = chat_response(&response(json!([function("call_a", "run", "{}")]))).unwrap();
    assert!(tools["choices"][0]["message"]["content"].is_null());
    assert_eq!(tools["choices"][0]["finish_reason"], "tool_calls");
    let refusal = json!({"type":"message","role":"assistant","content":[{"type":"refusal","refusal":"cannot comply"}]});
    let output = chat_response(&response(json!([refusal]))).unwrap();
    assert_eq!(output["choices"][0]["message"]["refusal"], "cannot comply");
    for (reason, finish) in [
        ("max_output_tokens", "length"),
        ("content_filter", "content_filter"),
    ] {
        let mut input = response(json!([]));
        input["status"] = json!("incomplete");
        input["incomplete_details"] = json!({"reason":reason});
        assert_eq!(
            chat_response(&input).unwrap()["choices"][0]["finish_reason"],
            finish
        );
    }
}

#[test]
fn response_citations_are_ignored_without_changing_model_text() {
    let mut cited = message("source");
    cited["content"][0]["annotations"] = json!([{
        "type":"url_citation","url":"https://example.test/source","title":"Source","start_index":0,"end_index":6
    }]);
    let output = chat_response(&response(json!([
        message("prefix "), {"type":"web_search_call","action":{"type":"search","query":"test"}}, cited
    ]))).unwrap();
    let message = &output["choices"][0]["message"];
    assert_eq!(message["content"], "prefix source");
    assert!(message.get("annotations").is_none());
}

#[test]
fn response_skips_unknown_outputs_and_accepts_empty_completion_but_not_failure() {
    for item in [
        json!({"type":"future_output","payload":"secret-body"}),
        json!({"type":"message","role":"assistant","content":[{"type":"output_audio","data":"secret-body"}]}),
        json!({"type":"web_search_call","action":{"sources":[{"url":"javascript:invalid"}]}}),
    ] {
        let output = chat_response(&response(json!([item]))).unwrap();
        assert!(output["choices"][0]["message"]["content"].is_null());
        assert_eq!(output["choices"][0]["finish_reason"], "stop");
        assert!(!output.to_string().contains("secret-body"));
    }
    let image = chat_response(&response(json!([
        {"type":"image_generation_call","result":"image-data"}
    ])))
    .unwrap();
    assert_eq!(
        image["choices"][0]["message"]["images"][0]["image_url"]["url"],
        "data:image/png;base64,image-data"
    );
    assert_eq!(
        chat_response(&response(json!([]))).unwrap()["choices"][0]["finish_reason"],
        "stop"
    );
    for status in ["in_progress", "failed", "cancelled"] {
        let mut input = response(json!([message("partial")]));
        input["status"] = json!(status);
        assert!(chat_response(&input).is_err());
    }
}

#[test]
fn stream_structural_events_update_metadata_without_emitting_role_or_usage() {
    for event in [
        "response.created",
        "response.in_progress",
        "response.queued",
    ] {
        let mut encoder = ChatStreamEncoder::new(false);
        let chunks = push(
            &mut encoder,
            json!({"type":event,"response":{"id":"resp_test","model":"gpt-test"}}),
        );
        assert!(chunks.is_empty());
        assert_eq!(encoder.response_id(), Some("resp_test"));
        assert!(!encoder.is_completed());
        let chunks = push(
            &mut encoder,
            json!({
                "type":"response.output_text.delta","delta":"hello"
            }),
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(chunks[0]["choices"][0]["delta"]["content"], "hello");
        assert_eq!(chunks[0]["id"], "chatcmpl-test");
        assert_eq!(chunks[0]["model"], "gpt-test");
        assert!(chunks[0].get("usage").is_none());
    }
}

#[test]
fn stream_text_is_immediate_and_done_and_terminal_never_replay_snapshots() {
    let mut encoder = ChatStreamEncoder::new(true);
    let mut chunks = created(&mut encoder);
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.output_item.added","output_index":0,
        "item":{"type":"message","id":"msg_1","role":"assistant","content":[]}}),
    ));
    let delta = push(
        &mut encoder,
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"hel"}),
    );
    assert_eq!(text(&delta, "content"), "hel");
    chunks.extend(delta);
    assert!(push(&mut encoder, json!({"type":"response.output_text.done","output_index":0,"content_index":0,"text":"hello"})).is_empty());
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.completed","response":response(json!([message("hello!")]))}),
    ));
    assert_eq!(text(&chunks, "content"), "hel");
    assert!(encoder.is_completed());
    assert!(!encoder.has_failure());
    assert_eq!(chunks.last().unwrap()["choices"], json!([]));
    assert_eq!(
        chunks.last().unwrap()["usage"]["completion_tokens_details"]["reasoning_tokens"],
        3
    );
    assert_eq!(
        chunks[chunks.len() - 2]["choices"][0]["finish_reason"],
        "stop"
    );
}

#[test]
fn stream_parallel_function_arguments_have_stable_indices_and_ids() {
    let mut encoder = ChatStreamEncoder::new(false);
    let mut chunks = created(&mut encoder);
    for (index, id, name) in [(1, "a", "one"), (3, "b", "two")] {
        chunks.extend(push(&mut encoder, json!({"type":"response.output_item.added","output_index":index,"item":function(id,name,"")})));
    }
    for (index, delta) in [(3, "{\"b\":"), (1, "{\"a\":"), (3, "2}"), (1, "1}")] {
        chunks.extend(push(&mut encoder, json!({"type":"response.function_call_arguments.delta","output_index":index,"delta":delta})));
    }
    let output = json!([
        {"type":"reasoning","summary":[]}, function("a","one","{\"a\":1}"),
        {"type":"reasoning","summary":[]}, function("b","two","{\"b\":2}")
    ]);
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.completed","response":response(output)}),
    ));
    let mut args = [String::new(), String::new()];
    let mut ids = Vec::new();
    for chunk in &chunks {
        if let Some(tool) = chunk.pointer("/choices/0/delta/tool_calls/0") {
            let index = tool["index"].as_u64().unwrap() as usize;
            args[index].push_str(tool["function"]["arguments"].as_str().unwrap());
            if let Some(id) = tool["id"].as_str() {
                ids.push((index, id));
            }
        }
    }
    assert_eq!(ids, vec![(0, "a"), (1, "b")]);
    assert_eq!(args, ["{\"a\":1}", "{\"b\":2}"]);
    assert_eq!(
        chunks.last().unwrap()["choices"][0]["finish_reason"],
        "tool_calls"
    );
}

#[test]
fn stream_arguments_before_identity_are_dropped_and_terminal_does_not_replay_tools() {
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    assert!(push(&mut encoder, json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"x\":"})).is_empty());
    let chunks = push(
        &mut encoder,
        json!({"type":"response.completed","response":response(json!([function("a","run","{\"x\":1}")]))}),
    );
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.pointer("/choices/0/delta/tool_calls").is_none())
    );
    assert_eq!(
        chunks.last().unwrap()["choices"][0]["finish_reason"],
        "stop"
    );
    assert!(encoder.is_completed());
}

#[test]
fn stream_refusal_and_reasoning_deltas_are_preserved_without_terminal_replay() {
    let mut encoder = ChatStreamEncoder::new(false);
    let mut chunks = created(&mut encoder);
    chunks.extend(push(&mut encoder, json!({"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":0,"delta":"think"})));
    assert_eq!(text(&chunks, "reasoning_content"), "think");
    chunks.extend(push(
        &mut encoder,
        json!({
            "type":"response.reasoning_summary_text.done","text":"thinking"
        }),
    ));
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.refusal.delta","output_index":1,"content_index":0,"delta":"no"}),
    ));
    let output = json!([
        {"type":"reasoning","summary":[{"type":"summary_text","text":"thinking"}]},
        {"type":"message","role":"assistant","content":[{"type":"refusal","refusal":"not possible"}]}
    ]);
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.completed","response":response(output)}),
    ));
    assert_eq!(text(&chunks, "reasoning_content"), "think\n\n");
    assert_eq!(text(&chunks, "refusal"), "no");
    let mut encoder = ChatStreamEncoder::new(false);
    let chunks = push(
        &mut encoder,
        json!({"type":"response.completed","response":response(json!([message("only terminal")]))}),
    );
    assert_eq!(text(&chunks, "content"), "");
    assert_eq!(
        chunks.last().unwrap()["choices"][0]["finish_reason"],
        "stop"
    );
}

#[test]
fn stream_skips_annotations_in_events_and_terminal_without_changing_text() {
    let citation = json!({"type":"url_citation","url":"https://example.test","title":"test","start_index":0,"end_index":4});
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    let mut chunks = push(
        &mut encoder,
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"text"}),
    );
    assert!(push(
        &mut encoder,
        json!({"type":"response.output_text.annotation.added","output_index":0,"content_index":0,"annotation_index":0,"annotation":citation}),
    ).is_empty());
    let mut item = message("text");
    item["content"][0]["annotations"] = json!([citation]);
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.completed","response":response(json!([item]))}),
    ));
    assert!(
        chunks
            .iter()
            .all(|v| v.pointer("/choices/0/delta/annotations").is_none())
    );
    assert_eq!(text(&chunks, "content"), "text");
}

#[test]
fn stream_missing_terminal_cannot_report_success_and_failures_are_latched() {
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    push(
        &mut encoder,
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"partial"}),
    );
    assert!(!encoder.is_completed());
    for event in [
        json!({"type":"response.failed","response":{"error":{"message":"secret-body"}}}),
        json!({"type":"response.cancelled","response":{"error":{"message":"secret-body"}}}),
        json!({"type":"error","message":"secret-body"}),
        json!({"type":"response.completed","response":{"status":"failed","error":{"message":"secret-body"}}}),
    ] {
        let mut encoder = ChatStreamEncoder::new(false);
        let error = encoder.push(None, &event).unwrap_err();
        assert!(!error.to_string().contains("secret-body"));
        assert!(encoder.has_failure());
        assert!(!encoder.is_completed());
        assert!(encoder.push(None, &json!({"type":"response.completed","response":response(json!([message("ignored")]))})).is_err());
    }
}

#[test]
fn stream_ignores_conflicting_or_missing_terminal_text_but_retains_failure_safety() {
    for output in [json!([message("changed")]), json!([])] {
        let mut encoder = ChatStreamEncoder::new(false);
        created(&mut encoder);
        push(
            &mut encoder,
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"original"}),
        );
        let chunks = push(
            &mut encoder,
            json!({"type":"response.completed","response":response(output)}),
        );
        assert_eq!(text(&chunks, "content"), "");
        assert_eq!(
            chunks.last().unwrap()["choices"][0]["finish_reason"],
            "stop"
        );
        assert!(encoder.is_completed());
        assert!(!encoder.has_failure());
    }
    let mut encoder = ChatStreamEncoder::new(false);
    assert!(
        encoder
            .push(Some("response.created"), &json!({"type":"response.failed"}))
            .is_err()
    );
}

#[test]
fn stream_incomplete_emits_length_but_absent_usage_never_becomes_fake_zero() {
    let mut encoder = ChatStreamEncoder::new(true);
    let mut input = response(json!([message("partial")]));
    input["status"] = json!("incomplete");
    input["incomplete_details"] = json!({"reason":"max_output_tokens"});
    input.as_object_mut().unwrap().remove("usage");
    let chunks = push(
        &mut encoder,
        json!({"type":"response.incomplete","response":input}),
    );
    assert_eq!(
        chunks[chunks.len() - 2]["choices"][0]["finish_reason"],
        "length"
    );
    assert!(chunks.last().unwrap()["usage"].is_null());
    assert!(encoder.is_completed());
}

#[test]
fn request_accepts_noop_options_and_function_response_round_trip() {
    let mut input = request();
    for (key, value) in [
        ("n", json!(1)),
        ("logprobs", json!(false)),
        ("top_logprobs", json!(0)),
        ("presence_penalty", json!(0)),
        ("frequency_penalty", json!(0)),
        ("modalities", json!(["text"])),
        ("tool_choice", json!("auto")),
    ] {
        input[key] = value;
    }
    let chat = chat_response(&response(json!([function(
        "call_1",
        "lookup",
        "{\"x\":1}"
    )])))
    .unwrap();
    input["messages"]
        .as_array_mut()
        .unwrap()
        .push(chat["choices"][0]["message"].clone());
    input["messages"].as_array_mut().unwrap().push(json!({
        "role":"tool","tool_call_id":"call_1","name":"lookup","content":[{"type":"text","text":"result"}]
    }));
    let decoded = decode_chat_request(input).unwrap();
    assert_eq!(decoded.responses["input"][1]["call_id"], "call_1");
    assert_eq!(
        decoded.responses["input"][2]["output"],
        json!([{"type":"input_text","text":"result"}])
    );
}

#[test]
fn request_rejects_conflicting_tool_identity_and_format() {
    let mut input = json!({"model":"gpt-test","messages":[
        {"role":"assistant","content":null,"tool_calls":[
            {"id":"a","type":"function","function":{"name":"run","arguments":"{}"}}
        ]},
        {"role":"tool","name":"different","tool_call_id":"a","content":"result"}
    ]});
    assert_eq!(
        decode_chat_request(input.clone()).unwrap_err().param(),
        Some("messages[1].name")
    );
    input["messages"][1]["name"] = json!("run");
    input["text"] = json!({"format":{"type":"text"}});
    input["response_format"] = json!({"type":"json_object"});
    assert_eq!(
        decode_chat_request(input).unwrap_err().param(),
        Some("response_format")
    );
}

#[test]
fn response_ignores_annotations_and_invalid_usage_fields_but_retains_valid_counts_and_failure() {
    let mut item = message("text");
    item["content"][0]["annotations"] = json!([{"type":"file_citation","file_id":"file_1"}]);
    let output = chat_response(&response(json!([item]))).unwrap();
    assert_eq!(output["choices"][0]["message"]["content"], "text");
    assert!(output["choices"][0]["message"].get("annotations").is_none());
    let mut input = response(json!([message("text")]));
    input["usage"]["output_tokens"] = json!(-1);
    let output = chat_response(&input).unwrap();
    assert!(output["usage"].get("completion_tokens").is_none());
    assert_eq!(output["usage"]["prompt_tokens"], 20);
    assert_eq!(output["usage"]["total_tokens"], 28);
    assert_eq!(
        output["usage"]["prompt_tokens_details"]["cached_tokens"],
        12
    );
    assert_eq!(
        output["usage"]["completion_tokens_details"]["reasoning_tokens"],
        3
    );
    assert_eq!(
        chat_response(&json!({"status":"failed","error":{"message":"secret"}}))
            .unwrap_err()
            .code(),
        "upstream_error"
    );
}

#[test]
fn stream_tool_lookup_falls_back_to_output_index_and_ignores_terminal_content_changes() {
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    push(
        &mut encoder,
        json!({"type":"response.output_item.added","output_index":0,"item":function("a","run","")}),
    );
    let chunks = push(
        &mut encoder,
        json!({
            "type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_other","delta":"{}"
        }),
    );
    assert_eq!(
        chunks[0]["choices"][0]["delta"]["tool_calls"][0],
        json!({
            "index":0,"function":{"arguments":"{}"}
        })
    );

    for replacement in [
        json!({"type":"web_search_call","action":{"type":"search"}}),
        json!({"type":"reasoning","summary":[]}),
        json!({"type":"message","role":"assistant","content":[]}),
    ] {
        let mut encoder = ChatStreamEncoder::new(false);
        created(&mut encoder);
        push(
            &mut encoder,
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"lost text"}),
        );
        let chunks = push(
            &mut encoder,
            json!({"type":"response.completed","response":response(json!([replacement,message("other")]))}),
        );
        assert_eq!(text(&chunks, "content"), "");
        assert!(encoder.is_completed());
    }
}

#[test]
fn stream_done_item_and_content_part_do_not_duplicate_text_or_arguments() {
    let mut encoder = ChatStreamEncoder::new(false);
    let mut chunks = created(&mut encoder);
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.content_part.added","output_index":0,"content_index":0,
        "part":{"type":"output_text","text":"","annotations":[]}}),
    ));
    chunks.extend(push(&mut encoder, json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"abc"})));
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.content_part.done","output_index":0,"content_index":0,
        "part":{"type":"output_text","text":"abc","annotations":[]}}),
    ));
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.output_item.done","output_index":0,"item":message("abc")}),
    ));
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.output_item.added","output_index":1,"item":function("a","run","")}),
    ));
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{"}),
    ));
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.function_call_arguments.done","output_index":1,"arguments":"{}"}),
    ));
    chunks.extend(push(&mut encoder, json!({"type":"response.output_item.done","output_index":1,"item":function("a","run","{}")})));
    chunks.extend(push(&mut encoder, json!({"type":"response.completed","response":response(json!([message("abc"),function("a","run","{}")]))})));
    assert_eq!(text(&chunks, "content"), "abc");
    let args: String = chunks
        .iter()
        .filter_map(|chunk| {
            chunk
                .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                .and_then(Value::as_str)
        })
        .collect();
    assert_eq!(args, "{");
}

#[test]
fn response_accepts_completed_empty_text_even_with_reasoning_or_annotations() {
    let mut annotated_empty = message("");
    annotated_empty["content"][0]["annotations"] = json!([{
        "type":"url_citation","url":"https://example.test","title":"test","start_index":0,"end_index":0
    }]);
    for output in [
        json!([message("")]),
        json!([message(""), message("")]),
        json!([{"type":"reasoning","summary":[{"type":"summary_text","text":"internal"}]}, message("")]),
        json!([annotated_empty]),
    ] {
        let output = chat_response(&response(output)).unwrap();
        assert!(output["choices"][0]["message"]["content"].is_null());
        assert!(output["choices"][0]["message"].get("annotations").is_none());
        assert_eq!(output["choices"][0]["finish_reason"], "stop");
    }
}

#[test]
fn stream_accepts_explicit_empty_completion_without_treating_preamble_as_completion() {
    for include_usage in [false, true] {
        for send_prefix in [false, true] {
            let mut encoder = ChatStreamEncoder::new(include_usage);
            let mut emitted = Vec::new();
            if send_prefix {
                emitted.extend(created(&mut encoder));
                emitted.extend(push(
                    &mut encoder,
                    json!({
                        "type":"response.output_item.added","output_index":0,
                        "item":{"type":"message","id":"msg_1","role":"assistant","content":[]}
                    }),
                ));
                emitted.extend(push(&mut encoder, json!({
                    "type":"response.output_text.delta","output_index":0,"content_index":0,"delta":""
                })));
                emitted.extend(push(&mut encoder, json!({
                    "type":"response.output_text.done","output_index":0,"content_index":0,"text":""
                })));
            }
            assert!(emitted.is_empty());
            assert!(!encoder.is_completed());
            let chunks = push(
                &mut encoder,
                json!({
                    "type":"response.completed","response":response(json!([message("")]))
                }),
            );
            assert_eq!(chunks.len(), if include_usage { 2 } else { 1 });
            assert_eq!(chunks[0]["choices"][0]["finish_reason"], "stop");
            assert_eq!(text(&chunks, "content"), "");
            assert_eq!(
                chunks
                    .iter()
                    .filter(|chunk| chunk.get("usage").is_some_and(|v| !v.is_null()))
                    .count(),
                usize::from(include_usage)
            );
            assert!(!encoder.has_failure());
            assert!(encoder.is_completed());
        }
    }
}

#[test]
fn buffered_empty_text_does_not_hide_content_but_stream_terminal_does_not_replay_it() {
    for output in [
        json!([message("answer"), message("")]),
        json!([message(""), message("answer")]),
    ] {
        let complete = chat_response(&response(output.clone())).unwrap();
        assert_eq!(complete["choices"][0]["message"]["content"], "answer");
        let mut encoder = ChatStreamEncoder::new(false);
        let chunks = push(
            &mut encoder,
            json!({"type":"response.completed","response":response(output)}),
        );
        assert_eq!(text(&chunks, "content"), "");
        assert!(encoder.is_completed());
    }
    let output = json!([message(""), function("call_1", "run", "{}")]);
    let complete = chat_response(&response(output.clone())).unwrap();
    assert!(complete["choices"][0]["message"]["content"].is_null());
    assert_eq!(complete["choices"][0]["finish_reason"], "tool_calls");
    let mut encoder = ChatStreamEncoder::new(false);
    let chunks = push(
        &mut encoder,
        json!({"type":"response.completed","response":response(output)}),
    );
    assert_eq!(
        chunks.last().unwrap()["choices"][0]["finish_reason"],
        "stop"
    );
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.pointer("/choices/0/delta/tool_calls").is_none())
    );
    assert!(encoder.is_completed());

    let output = json!([message(""), {
        "type":"message","role":"assistant","content":[{"type":"refusal","refusal":"cannot comply"}]
    }]);
    let complete = chat_response(&response(output)).unwrap();
    assert_eq!(
        complete["choices"][0]["message"]["refusal"],
        "cannot comply"
    );
}

#[test]
fn newapi_empty_image_metadata_is_allowed_but_nonempty_semantics_are_rejected() {
    for mime_type in [Value::Null, json!(""), json!("image/png"), json!(true)] {
        let input = json!({"model":"gpt-test","messages":[{"role":"user","content":[{
            "type":"image_url","image_url":{
                "url":"data:image/png;base64,AA==","MimeType":mime_type
            }
        }]}]});
        let decoded = decode_chat_request(input);
        if mime_type.is_null() || mime_type == "" {
            let decoded = decoded.unwrap();
            assert!(
                decoded.responses["input"][0]["content"][0]
                    .get("MimeType")
                    .is_none()
            );
        } else {
            assert_eq!(
                decoded.unwrap_err().param(),
                Some("messages[0].content[0].image_url.MimeType")
            );
        }
    }
}
mod cpa_response;
mod cpa_review;
mod cpa_stream;
mod newapi;
mod newapi_extensions;
mod output_extensions;
mod request_extensions;
