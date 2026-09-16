use gateway_protocol::openai::chat::chat_response;
use serde_json::{Value, json};

fn completed(output: Vec<Value>) -> Value {
    let mut response = json!({
        "id":"resp_cpa","model":"gpt-test","created_at":123,"status":"completed"
    });
    response["output"] = Value::Array(output);
    response
}

fn assistant(text: &str) -> Value {
    json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]})
}

fn image(result: Value, format: Value) -> Value {
    let mut item = json!({"type":"image_generation_call","id":"img_same","output_format":format});
    item["result"] = result;
    item
}

#[test]
fn annotations_are_ignored_without_editing_model_text() {
    let native_text = "{\"answer\":\"see [1] and \\u4e2d\\u6587\"}";
    for annotations in [
        Value::Null,
        json!(true),
        json!("malformed"),
        json!({"type":"url_citation"}),
        json!([null, false, 42]),
        json!([{"type":"file_citation","file_id":"file_1"}]),
        json!([{
            "type":"url_citation","url":{"unexpected":true},"title":false,
            "start_index":18446744073709551615_u64,"end_index":-1
        }]),
        json!([{
            "type":"url_citation","url":"https://example.test/source","title":"Source",
            "start_index":0,"end_index":3
        }]),
    ] {
        let mut item = assistant(native_text);
        item["content"][0]["annotations"] = annotations;
        item["content"][0]["logprobs"] = json!({"future":"ignored"});
        let native = completed(vec![
            json!({"type":"web_search_call","action":{"sources":"malformed"}}),
            item,
        ]);
        let output = chat_response(&native).unwrap();
        let message = &output["choices"][0]["message"];
        assert_eq!(message["content"], native_text);
        assert!(message.get("annotations").is_none());
        assert!(message.get("citations").is_none());
        assert!(message.get("sources").is_none());
        assert_eq!(
            serde_json::from_str::<Value>(message["content"].as_str().unwrap()).unwrap(),
            serde_json::from_str::<Value>(native_text).unwrap()
        );
    }
}

#[test]
fn unknown_items_parts_and_noncritical_message_fields_are_skipped() {
    let native = completed(vec![
        Value::Null,
        json!([]),
        json!(17),
        json!({"type":"future_output","payload":{"secret":"ignored"}}),
        json!({"type":"message","role":"user","status":false,"content":[
            null, 12,
            {"type":"output_audio","text":"ignored"},
            {"type":"output_text","text":"first","logprobs":[{"token":"ignored"}]},
            {"type":"future_part","text":"ignored"},
            {"type":"refusal","refusal":"cannot "},
            {"type":"output_text","text":" second"},
            {"type":"refusal","refusal":"comply"}
        ]}),
        json!({"type":"message","content":{"unexpected":"ignored"}}),
        assistant(" third"),
    ]);
    let output = chat_response(&native).unwrap();
    let message = &output["choices"][0]["message"];
    assert_eq!(message["role"], "assistant");
    assert_eq!(message["content"], "first second third");
    assert_eq!(message["refusal"], "cannot comply");
    assert_eq!(output["choices"][0]["finish_reason"], "stop");
}

#[test]
fn normal_terminal_does_not_require_output_or_nonempty_content() {
    let output = chat_response(&json!({"status":"completed"})).unwrap();
    assert_eq!(output["id"], "chatcmpl-");
    assert_eq!(output["model"], "");
    assert_eq!(output["created"], 0);
    assert!(output["choices"][0]["message"]["content"].is_null());
    assert_eq!(output["choices"][0]["finish_reason"], "stop");
    assert!(output.get("usage").is_none());

    for value in [
        Value::Null,
        json!(false),
        json!({"future":"output shape"}),
        json!([]),
        json!([{"type":"future_output"}]),
        json!([{"type":"message","content":null}]),
        json!([assistant("")]),
    ] {
        let mut native = completed(Vec::new());
        native["output"] = value;
        native["created_at"] = json!({"unknown":"timestamp"});
        let output = chat_response(&native).unwrap();
        assert!(output["choices"][0]["message"]["content"].is_null());
        assert_eq!(output["choices"][0]["finish_reason"], "stop");
        assert_eq!(output["created"], 0);
    }
}

#[test]
fn reasoning_accepts_summary_and_content_and_skips_unmapped_parts() {
    let native = completed(vec![
        json!({"type":"reasoning","summary":[
            {"type":"future_summary","text":"ignored"},
            {"type":"summary_text","text":"summary"},
            {"type":"summary_text","text":" continuation"}
        ],"content":[
            {"type":"future_reasoning","text":"ignored"},
            {"type":"reasoning_text","text":" reasoning"},
            {"type":"reasoning_text","text":" continuation"}
        ]}),
        json!({"type":"reasoning","summary":true,"content":{"ignored":true}}),
    ]);
    let output = chat_response(&native).unwrap();
    assert_eq!(
        output["choices"][0]["message"]["reasoning_content"],
        "summary continuation reasoning continuation"
    );
    assert!(output["choices"][0]["message"]["content"].is_null());
    assert_eq!(output["choices"][0]["finish_reason"], "stop");
}

#[test]
fn tools_preserve_raw_inputs_order_duplicates_and_optional_identity() {
    let arguments = "  {\"x\":1, \"unfinished\": \n";
    let custom_input = "*** Begin Patch\n+print(\"raw \\\\ input\")\n*** End Patch";
    let native = completed(vec![
        json!({"type":"function_call","call_id":"same","name":"lookup",
            "namespace":{"unknown":true},"arguments":arguments}),
        json!({"type":"custom_tool_call","call_id":"same","name":"code",
            "namespace":"custom","input":custom_input}),
        json!({"type":"function_call"}),
    ]);
    let output = chat_response(&native).unwrap();
    let tools = output["choices"][0]["message"]["tool_calls"]
        .as_array()
        .unwrap();
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0]["id"], "same");
    assert_eq!(tools[0]["function"]["arguments"], arguments);
    assert_eq!(tools[1]["id"], "same");
    assert_eq!(tools[1]["function"]["arguments"], custom_input);
    assert_eq!(tools[2]["id"], "");
    assert_eq!(tools[2]["function"], json!({"name":"","arguments":""}));
    assert!(tools.iter().all(|tool| tool["type"] == "function"));
    assert!(tools.iter().all(|tool| tool.get("custom").is_none()));
    assert!(output["choices"][0]["message"]["content"].is_null());
    assert_eq!(output["choices"][0]["finish_reason"], "tool_calls");
}

#[test]
fn tolerant_string_fields_keep_numbers_exact_and_do_not_parse_raw_strings() {
    let large_number: Value =
        serde_json::from_str("184467440737095516160000000000000000001").unwrap();
    for (value, expected) in [
        (Value::Null, String::new()),
        (json!(" raw\nnot JSON "), " raw\nnot JSON ".to_owned()),
        (json!(false), "false".to_owned()),
        (json!(37), "37".to_owned()),
        (json!([1, null, true]), "[1,null,true]".to_owned()),
        (json!({"x":1}), "{\"x\":1}".to_owned()),
        (
            large_number,
            "184467440737095516160000000000000000001".to_owned(),
        ),
    ] {
        let mut native = completed(vec![
            json!({"type":"message","content":[{"type":"output_text","text":value}]}),
            json!({"type":"function_call","call_id":value,"name":value,"arguments":value}),
            json!({"type":"custom_tool_call","call_id":value,"name":value,"input":value}),
        ]);
        native["id"] = value.clone();
        native["model"] = value;
        let output = chat_response(&native).unwrap();
        assert_eq!(output["id"], format!("chatcmpl-{expected}"));
        assert_eq!(output["model"], expected);
        let message = &output["choices"][0]["message"];
        if expected.is_empty() {
            assert!(message["content"].is_null());
        } else {
            assert_eq!(message["content"], expected);
        }
        for tool in message["tool_calls"].as_array().unwrap() {
            assert_eq!(tool["id"], expected);
            assert_eq!(tool["function"]["name"], expected);
            assert_eq!(tool["function"]["arguments"], expected);
        }
    }
}

#[test]
fn image_payloads_are_not_validated_as_base64() {
    for result in ["!", "AA", "AA=", "A===", "AB==", "AA-_\n raw"] {
        let output = chat_response(&completed(vec![image(json!(result), Value::Null)])).unwrap();
        assert_eq!(
            output["choices"][0]["message"]["images"][0]["image_url"]["url"],
            format!("data:image/png;base64,{result}")
        );
        assert!(output["choices"][0]["message"]["content"].is_null());
    }
}

#[test]
fn image_mime_uses_cpa_defaults_aliases_and_verbatim_custom_values() {
    for (format, expected) in [
        (Value::Null, "image/png"),
        (json!(""), "image/png"),
        (json!("PNG"), "image/png"),
        (json!("JpG"), "image/jpeg"),
        (json!("JPEG"), "image/jpeg"),
        (json!("WebP"), "image/webp"),
        (json!("GIF"), "image/gif"),
        (json!("avif"), "image/png"),
        (json!("not-a-known-format"), "image/png"),
        (json!("image/jpg"), "image/jpg"),
        (json!("IMAGE/AVIF"), "IMAGE/AVIF"),
        (
            json!("image/vnd.example.arbitrarily-long-custom-format;version=2"),
            "image/vnd.example.arbitrarily-long-custom-format;version=2",
        ),
        (json!(["image/avif"]), "[\"image/avif\"]"),
        (json!(true), "image/png"),
        (json!({"unknown":true}), "image/png"),
    ] {
        let output = chat_response(&completed(vec![image(json!("raw"), format)])).unwrap();
        assert_eq!(
            output["choices"][0]["message"]["images"][0]["image_url"]["url"],
            format!("data:{expected};base64,raw")
        );
    }
}

#[test]
fn image_results_skip_empty_values_and_keep_nonempty_coerced_values_in_order() {
    let mut first = image(json!("first"), json!("png"));
    first["status"] = json!("in_progress");
    let native = completed(vec![
        image(Value::Null, Value::Null),
        image(json!(""), Value::Null),
        json!({"type":"image_generation_call"}),
        first,
        image(json!("second"), json!("gif")),
        image(json!(17), Value::Null),
        image(json!({"raw":true}), Value::Null),
    ]);
    let output = chat_response(&native).unwrap();
    let images = output["choices"][0]["message"]["images"]
        .as_array()
        .unwrap();
    assert_eq!(images.len(), 4);
    for (index, expected) in [
        "data:image/png;base64,first",
        "data:image/gif;base64,second",
        "data:image/png;base64,17",
        "data:image/png;base64,{\"raw\":true}",
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(images[index]["index"], index);
        assert_eq!(images[index]["image_url"]["url"], expected);
    }
}

fn assert_image_sizes(sizes: &[usize]) {
    let native = completed(
        sizes
            .iter()
            .map(|size| image(Value::String("A".repeat(*size)), Value::Null))
            .collect(),
    );
    let output = chat_response(&native).unwrap();
    let images = output["choices"][0]["message"]["images"]
        .as_array()
        .unwrap();
    assert_eq!(images.len(), sizes.len());
    for (index, size) in sizes.iter().enumerate() {
        assert_eq!(images[index]["index"], index);
        let payload = images[index]["image_url"]["url"]
            .as_str()
            .unwrap()
            .strip_prefix("data:image/png;base64,")
            .unwrap();
        assert_eq!(payload.len(), *size);
        assert!(payload.bytes().all(|byte| byte == b'A'));
    }
}

#[test]
fn image_converter_has_no_64_mib_single_or_aggregate_result_limit() {
    // Keep both large cases sequential and move payloads into Value without cloning.
    assert_image_sizes(&[4]);
    assert_image_sizes(&[64 * 1024 * 1024 + 4]);
    assert_image_sizes(&[1024 * 1024; 65]);
}

#[test]
fn explicit_failures_and_missing_terminal_status_still_return_safe_errors() {
    for status in ["failed", "cancelled"] {
        let mut native = completed(vec![assistant("partial")]);
        native["status"] = json!(status);
        let error = chat_response(&native).unwrap_err();
        assert_eq!(error.code(), "upstream_error");
        assert!(!error.to_string().contains("partial"));
    }
    for error_value in [
        json!({"message":"private upstream failure","code":"secret"}),
        json!("private upstream failure"),
    ] {
        let mut native = completed(vec![assistant("partial")]);
        native["error"] = error_value;
        let error = chat_response(&native).unwrap_err();
        assert_eq!(error.code(), "upstream_error");
        assert!(!error.to_string().contains("private"));
        assert!(!error.to_string().contains("secret"));
    }
    for status in [
        Value::Null,
        json!("queued"),
        json!("in_progress"),
        json!("future_status"),
        json!(true),
    ] {
        let mut native = completed(vec![assistant("partial")]);
        native["status"] = status;
        let error = chat_response(&native).unwrap_err();
        assert_eq!(error.code(), "invalid_upstream_response");
        assert_eq!(error.param(), Some("status"));
    }
    assert!(chat_response(&json!({"output":[assistant("partial")]})).is_err());
}

#[test]
fn incomplete_reasons_use_cpa_mapping_without_requiring_known_details() {
    for (reason, expected) in [
        (json!("max_tokens"), "length"),
        (json!("max_output_tokens"), "length"),
        (json!("content_filter"), "content_filter"),
        (json!("future_reason"), "stop"),
        (Value::Null, "stop"),
        (json!({"unexpected":true}), "stop"),
    ] {
        let mut native = completed(Vec::new());
        native["status"] = json!("incomplete");
        native["incomplete_details"] = json!({"reason":reason});
        assert_eq!(
            chat_response(&native).unwrap()["choices"][0]["finish_reason"],
            expected
        );
    }
    assert_eq!(
        chat_response(&json!({"status":"incomplete"})).unwrap()["choices"][0]["finish_reason"],
        "stop"
    );
}

#[test]
fn absent_or_nonobject_wire_usage_never_creates_zero_counts() {
    for usage in [
        Value::Null,
        json!(false),
        json!(42),
        json!("malformed"),
        json!([]),
    ] {
        let mut native = completed(vec![assistant("answer")]);
        native["usage"] = usage;
        let output = chat_response(&native).unwrap();
        assert!(output.get("usage").is_none());
        assert_eq!(output["choices"][0]["message"]["content"], "answer");
    }
    let output = chat_response(&completed(vec![assistant("answer")])).unwrap();
    assert!(output.get("usage").is_none());
}

#[test]
fn wire_usage_filters_invalid_fields_without_mutating_the_upstream_response() {
    let mut native = completed(vec![assistant("answer")]);
    native["usage"] = json!({
        "input_tokens":20,"output_tokens":"8","total_tokens":-1,
        "input_tokens_details":{
            "cached_tokens":12,"cache_write_tokens":3,"unknown_count":7,
            "invalid_count":"9","negative":-1,"nested":{"count":2}
        },
        "output_tokens_details":{"reasoning_tokens":null,"future":true},
        "future_usage":{"ignored":true}
    });
    let original = native.clone();
    let output = chat_response(&native).unwrap();
    assert_eq!(output["choices"][0]["message"]["content"], "answer");
    assert_eq!(
        output["usage"],
        json!({
            "prompt_tokens":20,
            "prompt_tokens_details":{"cached_tokens":12,"cache_write_tokens":3,"unknown_count":7}
        })
    );
    // This is only the Chat wire projection, not canonical metering or accounting.
    assert_eq!(native, original);
}

#[test]
fn wire_usage_keeps_exact_integers_reported_totals_and_legitimate_zeroes() {
    let mut native = completed(Vec::new());
    native["usage"] = json!({
        "input_tokens":9007199254740993_u64,"output_tokens":8,
        "input_tokens_details":{"cache_write_tokens":9007199254740993_u64},
        "output_tokens_details":{"reasoning_tokens":0}
    });
    assert_eq!(
        chat_response(&native).unwrap()["usage"],
        json!({
            "prompt_tokens":9007199254740993_u64,"completion_tokens":8,
            "total_tokens":9007199254741001_u64,
            "prompt_tokens_details":{"cache_write_tokens":9007199254740993_u64},
            "completion_tokens_details":{"reasoning_tokens":0}
        })
    );
    native["usage"] = json!({"input_tokens":3,"output_tokens":5,"total_tokens":999});
    assert_eq!(
        chat_response(&native).unwrap()["usage"]["total_tokens"],
        999
    );
    native["usage"] = json!({"output_tokens":0});
    assert_eq!(
        chat_response(&native).unwrap()["usage"],
        json!({"completion_tokens":0})
    );
}

#[test]
fn wire_usage_overflow_and_malformed_details_do_not_discard_content() {
    let mut native = completed(vec![assistant("answer")]);
    native["usage"] = json!({
        "input_tokens":u64::MAX,"output_tokens":1,
        "input_tokens_details":"invalid","output_tokens_details":[1,2]
    });
    let output = chat_response(&native).unwrap();
    assert_eq!(output["choices"][0]["message"]["content"], "answer");
    assert_eq!(
        output["usage"],
        json!({"prompt_tokens":u64::MAX,"completion_tokens":1})
    );
    native["usage"] = json!({
        "input_tokens":false,"output_tokens":-1,"total_tokens":1.5,
        "input_tokens_details":{},"output_tokens_details":{"invalid":null}
    });
    assert_eq!(chat_response(&native).unwrap()["usage"], json!({}));
}
