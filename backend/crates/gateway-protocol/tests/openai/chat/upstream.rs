use super::*;

#[test]
fn upstream_cache_options_and_noop_extensions_preserve_the_native_payload() {
    let mut input = request();
    input["messages"][0]["content"] = json!([
        {"type":"text","text":"stable","prompt_cache_breakpoint":{"type":"ephemeral"}},
        {"type":"image_url","image_url":{"url":"https://example.test/image.png"},
         "prompt_cache_breakpoint":{"type":"ephemeral"}}
    ]);
    input["prompt_cache_options"] = json!({"retention":"24h"});
    input["moderation"] = json!({"input":true,"output":true});
    input["stream"] = json!(true);
    input["stream_options"] = json!({"include_usage":true,"include_obfuscation":false});
    input["max_tokens"] = json!(12);
    input["max_completion_tokens"] = json!(24);
    input["functions"] = json!([]);
    input["stop"] = json!([]);
    input["logit_bias"] = json!({});
    input["future_client_option"] = json!({"local":true});
    let decoded = decode_chat_request(input).unwrap();
    assert_eq!(decoded.responses["max_output_tokens"], 24);
    assert_eq!(
        decoded.responses["prompt_cache_options"],
        json!({"retention":"24h"})
    );
    assert_eq!(
        decoded.responses["moderation"],
        json!({"input":true,"output":true})
    );
    for part in decoded.responses["input"][0]["content"].as_array().unwrap() {
        assert_eq!(part["prompt_cache_breakpoint"], json!({"type":"ephemeral"}));
    }
    for key in [
        "future_client_option",
        "functions",
        "stop",
        "logit_bias",
        "stream_options",
    ] {
        assert!(!decoded.responses.contains_key(key));
    }
    assert!(decoded.include_usage);
}

#[test]
fn cache_options_reject_malformed_values_and_assistant_breakpoints() {
    for field in ["prompt_cache_options", "moderation"] {
        let mut input = request();
        input[field] = json!([]);
        assert_eq!(decode_chat_request(input).unwrap_err().param(), Some(field));
    }
    let mut input = request();
    input["messages"] = json!([
        {"role":"assistant","content":[{"type":"text","text":"x","prompt_cache_breakpoint":{}}]}
    ]);
    assert!(decode_chat_request(input).is_err());
}

#[test]
fn reordered_terminal_items_use_tool_and_message_identity_not_array_position() {
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    push(
        &mut encoder,
        json!({"type":"response.output_item.added","output_index":0,"item":message("")}),
    );
    push(
        &mut encoder,
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"hel"}),
    );
    push(
        &mut encoder,
        json!({"type":"response.output_item.added","output_index":1,"item":function("a","run","")}),
    );
    push(
        &mut encoder,
        json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{"}),
    );
    let chunks = push(
        &mut encoder,
        json!({"type":"response.completed","response":response(json!([
            function("a","run","{}"), message("hello")
        ]))}),
    );
    assert_eq!(text(&chunks, "content"), "lo");
    let arguments: String = chunks
        .iter()
        .filter_map(|chunk| {
            chunk
                .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                .and_then(Value::as_str)
        })
        .collect();
    assert_eq!(arguments, "}");
    assert!(encoder.is_completed());
}

#[test]
fn conflicting_tool_snapshots_fail_without_replaying_arguments() {
    let mut encoder = ChatStreamEncoder::new(false);
    push(
        &mut encoder,
        json!({"type":"response.output_item.added","output_index":0,"item":function("a","run","{\"x\":1}")}),
    );
    let result = encoder.push(None, &json!({"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"x\":2}"}));
    assert_eq!(result.unwrap_err().code(), "invalid_upstream_response");
    assert!(encoder.has_failure());
    assert!(!encoder.is_completed());
}

#[test]
fn moderation_and_service_tier_project_to_buffered_and_terminal_chat_output() {
    let mut source = response(json!([message("ok")]));
    source["service_tier"] = json!("priority");
    source["moderation"] = json!({
        "input":{"type":"moderation_result","model":"test-moderation","flagged":false},
        "output":{"type":"error","message":"unavailable"}
    });
    let buffered = chat_response(&source).unwrap();
    let mut encoder = ChatStreamEncoder::new(false);
    let chunks = push(
        &mut encoder,
        json!({"type":"response.completed","response":source}),
    );
    let terminal = chunks.last().unwrap();
    assert_eq!(terminal["moderation"], buffered["moderation"]);
    assert_eq!(terminal["service_tier"], "priority");
    assert_eq!(
        terminal["moderation"]["input"]["type"],
        "moderation_results"
    );
    assert_eq!(
        terminal["moderation"]["input"]["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn metadata_accepts_object_values_without_rewriting_them() {
    let mut input = request();
    input["metadata"] = json!({"count":2,"nested":{"enabled":true},"tags":["a"]});
    let decoded = decode_chat_request(input.clone()).unwrap();
    assert_eq!(decoded.responses["metadata"], input["metadata"]);
    input["metadata"] = json!([]);
    assert_eq!(
        decode_chat_request(input).unwrap_err().param(),
        Some("metadata")
    );
}

#[test]
fn conflicting_tool_name_call_id_or_alias_fails_without_completing() {
    for field in ["name", "call_id"] {
        let mut encoder = ChatStreamEncoder::new(false);
        let tool = function("a", "run", "");
        push(
            &mut encoder,
            json!({"type":"response.output_item.added","output_index":0,"item":tool}),
        );
        let mut changed = function("a", "run", "{}");
        changed[field] = json!("different");
        assert!(
            encoder
                .push(
                    None,
                    &json!({"type":"response.output_item.done","output_index":0,"item":changed})
                )
                .is_err()
        );
        assert!(encoder.has_failure());
        assert!(!encoder.is_completed());
    }
    let mut encoder = ChatStreamEncoder::new(false);
    let mut first = function("a", "run", "");
    first["id"] = json!("item_a");
    let mut second = function("b", "run", "");
    second["id"] = json!("item_b");
    push(
        &mut encoder,
        json!({"type":"response.output_item.added","output_index":0,"item":first}),
    );
    push(
        &mut encoder,
        json!({"type":"response.output_item.added","output_index":1,"item":second}),
    );
    assert!(encoder.push(None, &json!({"type":"response.function_call_arguments.delta","item_id":"item_a","output_index":1,"delta":"{}"})).is_err());
    assert!(encoder.has_failure());
}

#[test]
fn text_identity_conflicts_fail_but_reordered_terminal_messages_keep_their_own_suffixes() {
    let start = || {
        let mut encoder = ChatStreamEncoder::new(false);
        for (index, id, text) in [(0, "msg_a", "A"), (1, "msg_b", "B")] {
            push(
                &mut encoder,
                json!({"type":"response.output_text.delta",
                "item_id":id,"output_index":index,"content_index":0,"delta":text}),
            );
        }
        encoder
    };
    for id in ["msg_b", "msg_other"] {
        let mut encoder = start();
        assert!(
            encoder
                .push(
                    None,
                    &json!({"type":"response.output_text.delta",
            "item_id":id,"output_index":0,"content_index":0,"delta":"wrong"})
                )
                .is_err()
        );
        assert!(encoder.has_failure());
        assert!(!encoder.is_completed());
    }
    let mut encoder = start();
    let mut first = message("A1");
    first["id"] = json!("msg_a");
    let mut second = message("B2");
    second["id"] = json!("msg_b");
    let chunks = push(
        &mut encoder,
        json!({"type":"response.completed",
        "response":response(json!([second, first]))}),
    );
    assert_eq!(text(&chunks, "content"), "21");
    assert!(encoder.is_completed());
}
