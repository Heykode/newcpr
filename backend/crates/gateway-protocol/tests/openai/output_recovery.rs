use gateway_protocol::openai::{
    chat::{ChatStreamEncoder, chat_response_from_events},
    output_recovery::recover_response_output,
};
use serde_json::{Value, json};

fn terminal(output: Value) -> Value {
    json!({
        "id":"resp_recovery","model":"model-a","created_at":7,"status":"completed",
        "output":output,"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5},
        "future_response":{"keep":true}
    })
}

fn done(index: Option<u64>, item: Value) -> Value {
    let mut event = json!({"type":"response.output_item.done","item":item});
    if let Some(index) = index {
        event["output_index"] = json!(index);
    }
    event
}

fn message(id: &str, text: &str) -> Value {
    json!({"type":"message","id":id,"role":"assistant","status":"completed",
        "phase":"final_answer","content":[{"type":"output_text","text":text,
            "annotations":[{"type":"future_annotation","opaque":1}]}]})
}

fn function(id: &str, call: &str, name: &str, arguments: &str) -> Value {
    json!({"type":"function_call","id":id,"call_id":call,"name":name,
        "arguments":arguments,"future_tool":{"keep":true}})
}

fn recover(response: &mut Value, events: &[Value]) {
    recover_response_output(
        response,
        events
            .iter()
            .map(|event| (event["type"].as_str().expect("fixture event type"), event)),
    );
}

#[test]
fn buffered_recovery_keeps_raw_done_items_sorted_with_unindexed_fallback() {
    let items = [
        json!({"type":"reasoning","id":"r","encrypted_content":"opaque","summary":[]}),
        message("m", "answer"),
        function("f", "call-a", "lookup", "{\"q\":1}"),
        json!({"type":"web_search_call","id":"s","action":{"type":"search","query":"q"},
            "future_search":true}),
        json!({"type":"image_generation_call","id":"i","result":"opaque-image!",
            "output_format":"image/future","future_image":[1,2]}),
        json!({"type":"future_item","payload":{"keep":true}}),
        message("fallback", "unindexed"),
    ];
    let events = vec![
        done(Some(4), items[4].clone()),
        done(None, items[5].clone()),
        done(Some(2), items[2].clone()),
        done(Some(1), items[1].clone()),
        done(Some(0), items[0].clone()),
        done(None, items[6].clone()),
        done(Some(3), items[3].clone()),
        done(Some(4), items[4].clone()),
        done(None, items[6].clone()),
        json!({"type":"response.output_text.delta","item_id":"m","output_index":1,
            "content_index":0,"delta":"must not duplicate the raw message"}),
    ];
    for output in [json!([]), Value::Null] {
        let mut response = terminal(output);
        recover(&mut response, &events);
        assert_eq!(response["output"], json!(items));
        assert_eq!(response["usage"]["total_tokens"], 5);
        assert_eq!(response["future_response"], json!({"keep":true}));
    }
    let mut absent = terminal(json!([]));
    absent.as_object_mut().unwrap().remove("output");
    recover(&mut absent, &events);
    assert_eq!(absent["output"], json!(items));
}

#[test]
fn buffered_recovery_preserves_authoritative_terminal_and_unknown_fields() {
    let output = json!([
        message("terminal", "authoritative"),
        function("f", "call-a", "lookup", " authoritative raw input "),
        {"type":"image_generation_call","id":"i","result":"terminal-image",
            "opaque":true}
    ]);
    let mut response = terminal(output);
    let original = response.clone();
    recover(
        &mut response,
        &[
            done(Some(0), message("other", "not authoritative")),
            done(Some(1), function("f", "call-a", "lookup", "different")),
            done(Some(3), message("extra", "do not append")),
        ],
    );
    assert_eq!(response, original);
}

#[test]
fn buffered_recovery_keeps_delta_only_items_alongside_raw_done_items() {
    let complete = message("complete", "raw");
    let events = [
        done(Some(1), complete.clone()),
        json!({"type":"response.output_text.delta","item_id":"partial","output_index":0,
            "content_index":0,"delta":"delta-only"}),
        json!({"type":"response.output_text.delta","item_id":"complete","output_index":1,
            "content_index":0,"delta":"do not duplicate raw"}),
    ];
    let mut response = terminal(json!([]));
    recover(&mut response, &events);
    assert_eq!(response["output"].as_array().unwrap().len(), 2);
    assert_eq!(response["output"][0]["id"], "partial");
    assert_eq!(response["output"][0]["content"][0]["text"], "delta-only");
    assert_eq!(response["output"][1], complete);

    let mut missing_status = terminal(json!([]));
    missing_status.as_object_mut().unwrap().remove("status");
    recover(&mut missing_status, &events);
    assert_eq!(missing_status["output"], response["output"]);
    assert!(missing_status.get("status").is_none());
}

#[test]
fn buffered_recovery_keeps_unindexed_done_separate_from_identified_deltas() {
    let unindexed = json!({"type":"message","role":"assistant",
        "content":[{"type":"output_text","text":"second"}]});
    let events = [
        json!({"type":"response.output_text.delta","item_id":"m1","output_index":0,
            "content_index":0,"delta":"first"}),
        done(None, unindexed.clone()),
    ];
    let mut response = terminal(json!([]));
    recover(&mut response, &events);
    assert_eq!(response["output"].as_array().unwrap().len(), 2);
    assert_eq!(response["output"][0]["id"], "m1");
    assert_eq!(response["output"][0]["content"][0]["text"], "first");
    assert_eq!(response["output"][1], unindexed);
}

#[test]
fn buffered_recovery_uses_only_received_deltas_and_preserves_part_order_and_ids() {
    let events = vec![
        json!({"type":"response.output_text.delta","item_id":"m","output_index":2,
            "content_index":1,"delta":"second"}),
        json!({"type":"response.reasoning_summary_text.delta","item_id":"r","output_index":0,
            "summary_index":0,"delta":"reason"}),
        json!({"type":"response.output_text.delta","item_id":"m","output_index":2,
            "content_index":0,"delta":"hel"}),
        json!({"type":"response.output_text.delta","item_id":"m","output_index":2,
            "content_index":0,"delta":"lo"}),
        json!({"type":"response.refusal.delta","item_id":"m","output_index":2,
            "content_index":2,"delta":"no"}),
        json!({"type":"response.output_item.added","output_index":1,
            "item":function("f","call-a","lookup","")}),
        json!({"type":"response.function_call_arguments.delta","item_id":"f","output_index":1,
            "delta":"{\"q\":"}),
        json!({"type":"response.function_call_arguments.delta","item_id":"f","output_index":1,
            "delta":"1}"}),
        json!({"type":"response.output_text.done","item_id":"m","output_index":2,
            "content_index":0,"text":"hello"}),
        json!({"type":"response.image_generation_call.partial_image","item_id":"preview",
            "output_index":3,"partial_image_b64":"not-a-final-image"}),
    ];
    let mut response = terminal(json!([]));
    recover(&mut response, &events);
    let output = response["output"].as_array().unwrap();
    assert_eq!(output.len(), 3);
    assert_eq!(output[0]["id"], "r");
    assert_eq!(output[0]["summary"][0]["text"], "reason");
    assert_eq!(output[1]["call_id"], "call-a");
    assert_eq!(output[1]["arguments"], "{\"q\":1}");
    assert_eq!(output[1]["future_tool"], json!({"keep":true}));
    assert_eq!(output[2]["id"], "m");
    assert_eq!(output[2]["content"][0]["text"], "hello");
    assert_eq!(output[2]["content"][1]["text"], "second");
    assert_eq!(output[2]["content"][2]["refusal"], "no");
    assert!(output.iter().all(|item| item.get("status").is_none()));
}

#[test]
fn buffered_recovery_arguments_follow_call_identity_not_reused_output_index() {
    let events = vec![
        json!({"type":"response.output_item.added","output_index":0,
            "item":function("f-a","call-a","lookup","")}),
        json!({"type":"response.function_call_arguments.delta","item_id":"f-a",
            "output_index":0,"delta":"args-a"}),
        json!({"type":"response.function_call_arguments.done","item_id":"f-a",
            "output_index":0,"arguments":"final-a"}),
        json!({"type":"response.output_item.added","output_index":1,
            "item":function("f-b","call-b","lookup","")}),
        json!({"type":"response.function_call_arguments.delta","item_id":"f-b",
            "output_index":1,"delta":"args-b"}),
    ];
    let mut response = terminal(json!([
        function("f-b", "call-b", "lookup", ""),
        function("f-a", "call-a", "lookup", ""),
        function("f-c", "call-c", "lookup", "")
    ]));
    recover(&mut response, &events);
    assert_eq!(response["output"][0]["arguments"], "args-b");
    assert_eq!(response["output"][1]["arguments"], "final-a");
    assert_eq!(response["output"][2]["arguments"], "");

    let mut conflicting = terminal(json!([function("f-c", "call-c", "lookup", "")]));
    recover(&mut conflicting, &events);
    assert_eq!(conflicting["output"][0]["arguments"], "");

    let mut raw_events = events;
    raw_events.push(done(Some(0), function("f-c", "call-c", "lookup", "")));
    let mut empty = terminal(json!([]));
    recover(&mut empty, &raw_events);
    assert_eq!(
        empty["output"],
        json!([
            function("f-c", "call-c", "lookup", ""),
            function("f-b", "call-b", "lookup", "args-b")
        ])
    );
}

#[test]
fn buffered_recovery_supports_custom_input_and_safe_missing_identity_fallback() {
    let events = vec![
        json!({"type":"response.output_item.added","output_index":0,
            "item":{"type":"custom_tool_call","id":"custom","call_id":"custom-call",
                "name":"code","input":""}}),
        json!({"type":"response.custom_tool_call_input.delta","item_id":"custom",
            "output_index":0,"delta":"print("}),
        json!({"type":"response.custom_tool_call_input.done","item_id":"custom",
            "output_index":0,"input":" print(1)\n"}),
        json!({"type":"response.output_item.added","output_index":1,
            "item":{"type":"function_call","name":"lookup","arguments":""}}),
        json!({"type":"response.function_call_arguments.delta","output_index":1,
            "delta":"unparsed input"}),
    ];
    let mut response = terminal(json!([
        {"type":"custom_tool_call","id":"custom","call_id":"custom-call","name":"code","input":""},
        {"type":"function_call","call_id":"later-id","name":"lookup","arguments":""},
        {"type":"function_call","name":"different","arguments":""}
    ]));
    recover(&mut response, &events);
    assert_eq!(response["output"][0]["input"], " print(1)\n");
    assert_eq!(response["output"][1]["arguments"], "unparsed input");
    assert_eq!(response["output"][2]["arguments"], "");
}

#[test]
fn buffered_recovery_does_not_invent_success_or_reject_empty_and_unknown_output() {
    let events = [done(Some(0), message("m", "received"))];
    for mut response in [Value::Null, json!(7), json!([])] {
        let original = response.clone();
        recover(&mut response, &events);
        assert_eq!(response, original);
    }
    for status in ["failed", "cancelled", "in_progress"] {
        let mut response = terminal(json!([]));
        response["status"] = json!(status);
        let original = response.clone();
        recover(&mut response, &events);
        assert_eq!(response, original);
    }
    let mut empty = terminal(json!([]));
    let original = empty.clone();
    recover(
        &mut empty,
        &[json!({"type":"response.output_text.delta","delta":""})],
    );
    assert_eq!(empty, original);

    let mut failure = terminal(json!([]));
    let failure_events = [
        events[0].clone(),
        json!({"type":"response.failed","response":{"status":"failed"}}),
    ];
    recover(&mut failure, &failure_events);
    assert_eq!(failure["output"], json!([]));
}

#[test]
fn buffered_chat_recovers_once_without_rewriting_streamed_text_or_images() {
    let image = json!({"type":"image_generation_call","id":"img","result":"opaque-image!",
        "output_format":"png"});
    let events = vec![
        json!({"type":"response.created","response":{"id":"resp_recovery","model":"model-a"}}),
        json!({"type":"response.output_text.delta","item_id":"m","output_index":0,
            "content_index":0,"delta":"answer"}),
        done(Some(0), message("m", "answer")),
        done(Some(1), image),
        json!({"type":"response.completed","response":terminal(json!([]))}),
    ];
    let response = chat_response_from_events(events.iter().map(|event| (None, event))).unwrap();
    assert_eq!(response["choices"][0]["message"]["content"], "answer");
    assert_eq!(
        response["choices"][0]["message"]["images"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(response["usage"]["total_tokens"], 5);

    let mut encoder = ChatStreamEncoder::new(true);
    let chunks: Vec<_> = events
        .iter()
        .flat_map(|event| encoder.push(None, event).unwrap())
        .collect();
    assert_eq!(
        chunks
            .iter()
            .filter_map(|chunk| chunk["choices"][0]["delta"]["content"].as_str())
            .collect::<String>(),
        "answer"
    );
    assert_eq!(
        chunks
            .iter()
            .filter(|chunk| chunk["choices"][0]["delta"]["images"].is_array())
            .count(),
        1
    );
    assert_eq!(
        chunks
            .iter()
            .filter(|chunk| chunk["usage"].is_object())
            .count(),
        1
    );
    assert!(encoder.is_completed());

    assert!(chat_response_from_events(events[..4].iter().map(|event| (None, event))).is_err());
    let mut failed = events;
    failed.insert(
        4,
        json!({"type":"response.failed","response":{"status":"failed"}}),
    );
    assert!(chat_response_from_events(failed.iter().map(|event| (None, event))).is_err());
}
