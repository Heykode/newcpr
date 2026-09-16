use gateway_protocol::openai::chat::{ChatStreamEncoder, chat_response_from_events};
use serde_json::{Value, json};

// CPA ac02da6c's unconditional current-tool fallback loses a distinct done-only
// call when an earlier call exists. Keep keyless fallback, not cross-association.
#[test]
fn distinct_done_only_tools_are_announced_without_cross_association() {
    for (kind, input) in [
        ("function_call", "arguments"),
        ("custom_tool_call", "input"),
    ] {
        for key in ["item", "item_id", "output_index", "call_id"] {
            let mut encoder = ChatStreamEncoder::new(false);
            for index in 0..2 {
                let mut event = json!({
                    "type":"response.output_item.done",
                    "item":{"type":kind,"call_id":format!("call_{index}"),"name":"run",input:"raw {"}
                });
                match key {
                    "item" => event["item"]["id"] = json!(format!("item_{index}")),
                    "item_id" => event["item_id"] = json!(format!("item_{index}")),
                    "call_id" => {}
                    _ => event["output_index"] = json!(index),
                }
                let chunks = encoder.push(None, &event).unwrap();
                assert_eq!(chunks.len(), 1, "{key}: {event}");
                assert_eq!(
                    chunks[0]["choices"][0]["delta"]["tool_calls"][0],
                    json!({
                        "index":index,"id":format!("call_{index}"),"type":"function",
                        "function":{"name":"run","arguments":"raw {"}
                    })
                );
                assert!(encoder.push(None, &event).unwrap().is_empty());
            }
        }
    }
}

#[test]
fn call_ids_link_interleaved_tools_without_item_or_output_indices() {
    let mut encoder = ChatStreamEncoder::new(false);
    for (index, id) in ["a", "b"].into_iter().enumerate() {
        let chunks = encoder
            .push(
                None,
                &json!({
                    "type":"response.output_item.added",
                    "item":{"type":"function_call","call_id":id,"name":"run"}
                }),
            )
            .unwrap();
        assert_eq!(
            chunks[0]["choices"][0]["delta"]["tool_calls"][0]["index"],
            index
        );
    }
    for (index, id, input) in [(0, "a", "A"), (1, "b", "B")] {
        let chunks = encoder
            .push(
                None,
                &json!({
                    "type":"response.function_call_arguments.delta","call_id":id,"delta":input
                }),
            )
            .unwrap();
        assert_eq!(
            chunks[0]["choices"][0]["delta"],
            json!({
                "tool_calls":[{"index":index,"function":{"arguments":input}}]
            })
        );
    }
    let done = json!({
        "type":"response.output_item.done",
        "item":{"type":"function_call","call_id":"a","name":"run","arguments":"different full"}
    });
    assert!(encoder.push(None, &done).unwrap().is_empty());
    assert!(encoder.push(None, &done).unwrap().is_empty());
    for id in ["a", "unknown"] {
        assert!(encoder.push(None, &json!({
            "type":"response.function_call_arguments.delta","call_id":id,"delta":"ignored"
        })).unwrap().is_empty());
    }
    let chunks = encoder
        .push(
            None,
            &json!({
                "type":"response.function_call_arguments.delta","delta":"current B"
            }),
        )
        .unwrap();
    assert_eq!(
        chunks[0]["choices"][0]["delta"],
        json!({
            "tool_calls":[{"index":1,"function":{"arguments":"current B"}}]
        })
    );
    assert!(!encoder.has_failure());
}

#[test]
fn new_done_only_tool_does_not_close_an_existing_announced_tool() {
    let mut encoder = ChatStreamEncoder::new(false);
    encoder
        .push(
            None,
            &json!({
                "type":"response.output_item.added","output_index":0,
                "item":{"type":"function_call","id":"first","call_id":"call_0","name":"run"}
            }),
        )
        .unwrap();
    let chunks = encoder
        .push(
            None,
            &json!({
                "type":"response.output_item.done","output_index":1,
                "item":{"type":"custom_tool_call","id":"second","call_id":"call_1",
                    "name":"code","input":"print(1)"}
            }),
        )
        .unwrap();
    assert_eq!(
        chunks[0]["choices"][0]["delta"]["tool_calls"][0],
        json!({
            "index":1,"id":"call_1","type":"function",
            "function":{"name":"code","arguments":"print(1)"}
        })
    );
    let chunks = encoder
        .push(None, &json!({
            "type":"response.function_call_arguments.delta","item_id":"first","delta":"still open"
        }))
        .unwrap();
    assert_eq!(
        chunks[0]["choices"][0]["delta"],
        json!({"tool_calls":[{"index":0,"function":{"arguments":"still open"}}]})
    );
}

#[test]
fn unknown_tool_keys_do_not_modify_the_current_call_but_keyless_fallback_remains() {
    let mut encoder = ChatStreamEncoder::new(false);
    encoder
        .push(
            None,
            &json!({
                "type":"response.output_item.added","output_index":0,
                "item":{"type":"function_call","id":"known","call_id":"call","name":"run"}
            }),
        )
        .unwrap();
    for key in [json!({"item_id":"new"}), json!({"output_index":1})] {
        let mut delta = key;
        delta["type"] = json!("response.function_call_arguments.delta");
        delta["delta"] = json!("must not reach another call");
        assert!(encoder.push(None, &delta).unwrap().is_empty());
    }
    let chunks = encoder
        .push(
            None,
            &json!({
                "type":"response.function_call_arguments.delta","delta":"keyless"
            }),
        )
        .unwrap();
    assert_eq!(
        chunks[0]["choices"][0]["delta"],
        json!({"tool_calls":[{"index":0,"function":{"arguments":"keyless"}}]})
    );
    let chunks = encoder
        .push(
            None,
            &json!({
                "type":"response.function_call_arguments.delta","item_id":"new",
                "output_index":0,"delta":"matched output"
            }),
        )
        .unwrap();
    assert_eq!(
        chunks[0]["choices"][0]["delta"],
        json!({"tool_calls":[{"index":0,"function":{"arguments":"matched output"}}]})
    );
    assert!(!encoder.has_failure());
}

#[test]
fn malformed_event_containers_do_not_panic() {
    for value in [Value::Null, json!(false), json!(7), json!("raw"), json!([])] {
        for kind in [
            "response.created",
            "response.output_text.delta",
            "response.output_item.added",
            "response.output_item.done",
            "response.function_call_arguments.delta",
            "response.function_call_arguments.done",
            "response.image_generation_call.partial_image",
            "response.completed",
        ] {
            for event in [
                value.clone(),
                json!({"type":kind,"response":value,"item":value}),
            ] {
                let mut encoder = ChatStreamEncoder::new(true);
                encoder.push(Some(kind), &event).unwrap();
            }
        }
    }
}

#[test]
fn buffered_chat_requires_its_own_terminal_and_skips_unknown_event_snapshots() {
    let valid = json!({
        "type":"response.completed",
        "response":{"status":"completed","output":[{"type":"message","content":[
            {"type":"output_text","text":"real terminal"}
        ]}]}
    });
    let unknown = json!({
        "type":"response.future_event",
        "response":{"status":"completed","output":[{"type":"message","content":[
            {"type":"output_text","text":"not a terminal"}
        ]}]}
    });
    assert!(chat_response_from_events([(Some("response.completed"), &unknown)]).is_err());
    let response = chat_response_from_events([
        (Some("response.future_event"), &valid),
        (Some("response.completed"), &unknown),
    ])
    .unwrap();
    assert_eq!(
        response["choices"][0]["message"]["content"],
        "real terminal"
    );
    assert_eq!(response["choices"][0]["finish_reason"], "stop");
    let failure = json!({"type":"error"});
    assert!(
        chat_response_from_events([
            (Some("response.completed"), &valid),
            (Some("response.future_event"), &failure),
        ])
        .is_err()
    );
}
