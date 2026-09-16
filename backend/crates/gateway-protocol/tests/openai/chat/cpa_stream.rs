use gateway_protocol::openai::chat::ChatStreamEncoder;
use serde_json::{Value, json};

use super::{created, function, message, push, response, text};

// Hand-authored event goldens from CPA ac02da6c05e18f465aa7e3ed5b0a65a2f060917d,
// internal/translator/codex/openai/chat-completions/codex_openai_response.go.
// These are converter fixtures, not captured upstream traffic. CPR's Chat ID,
// refusal, usage opt-in and executor-level failure/EOF protections are retained.
// CPR also avoids CPA's cross-tool fallback when explicit tool keys are unknown.

fn chunk(delta: Value) -> Value {
    json!({
        "id":"chatcmpl-test","object":"chat.completion.chunk","created":123,"model":"gpt-test",
        "choices":[{"index":0,"delta":delta,"finish_reason":null,"logprobs":null}]
    })
}

fn expect_delta(encoder: &mut ChatStreamEncoder, event: Value, delta: Value) {
    assert_eq!(push(encoder, event), vec![chunk(delta)]);
    assert!(!encoder.is_completed());
    assert!(!encoder.has_failure());
}

fn arguments(index: usize, value: &str) -> Value {
    json!({"tool_calls":[{"index":index,"function":{"arguments":value}}]})
}

fn finish(encoder: &mut ChatStreamEncoder, output: Value, reason: &str) {
    let mut expected = chunk(json!({}));
    expected["choices"][0]["finish_reason"] = json!(reason);
    assert_eq!(
        push(
            encoder,
            json!({"type":"response.completed","response":response(output)})
        ),
        vec![expected]
    );
    assert!(encoder.is_completed());
    assert!(!encoder.has_failure());
}

#[test]
fn cpa_metadata_never_emits_and_text_and_refusal_are_delivered_one_event_at_a_time() {
    let mut encoder = ChatStreamEncoder::new(false);
    for kind in [
        "response.created",
        "response.in_progress",
        "response.queued",
    ] {
        assert!(
            push(
                &mut encoder,
                json!({
                    "type":kind,"response":{"id":"resp_test","model":"gpt-test","created_at":123}
                })
            )
            .is_empty()
        );
        assert_eq!(encoder.response_id(), Some("resp_test"));
        assert!(!encoder.is_completed());
    }
    for delta in ["A", "A", "\u{4e2d}\n"] {
        expect_delta(
            &mut encoder,
            json!({
                "type":"response.output_text.delta","delta":delta,
                "output_index":null,"content_index":"future","item_id":"unannounced",
                "logprobs":[{"future":"metadata"}]
            }),
            json!({"role":"assistant","content":delta}),
        );
    }
    expect_delta(
        &mut encoder,
        json!({
            "type":"response.refusal.delta","delta":"cannot "
        }),
        json!({"role":"assistant","refusal":"cannot "}),
    );
    expect_delta(
        &mut encoder,
        json!({
            "type":"response.refusal.delta","delta":"comply"
        }),
        json!({"role":"assistant","refusal":"comply"}),
    );
    for event in [
        json!({"type":"response.refusal.done","refusal":"changed snapshot"}),
        json!({"type":"response.output_text.done","text":"changed snapshot"}),
        json!({"type":"response.content_part.added","part":{"type":"output_text","text":"snapshot"}}),
        json!({"type":"response.content_part.done","part":{"type":"output_text","text":"snapshot"}}),
        json!({"type":"response.output_item.done","item":message("snapshot")}),
    ] {
        assert!(push(&mut encoder, event).is_empty());
    }
    finish(
        &mut encoder,
        json!([message("unrelated terminal text")]),
        "stop",
    );
}

#[test]
fn cpa_reasoning_done_emits_two_newlines_without_replaying_any_snapshot() {
    for prefix in ["response.reasoning_summary_text", "response.reasoning_text"] {
        let mut encoder = ChatStreamEncoder::new(false);
        assert!(created(&mut encoder).is_empty());
        expect_delta(
            &mut encoder,
            json!({
                "type":format!("{prefix}.delta"),"delta":"thinking"
            }),
            json!({"role":"assistant","reasoning_content":"thinking"}),
        );
        for _ in 0..2 {
            expect_delta(
                &mut encoder,
                json!({
                    "type":format!("{prefix}.done"),"text":"different full reasoning"
                }),
                json!({"role":"assistant","reasoning_content":"\n\n"}),
            );
        }
        finish(
            &mut encoder,
            json!([{
                "type":"reasoning","summary":[{"type":"summary_text","text":"terminal only"}]
            }]),
            "stop",
        );
    }
}

#[test]
fn cpa_empty_deltas_skip_and_nonempty_values_use_loose_string_extraction() {
    for (kind, field) in [
        ("response.output_text.delta", "content"),
        ("response.refusal.delta", "refusal"),
        ("response.reasoning_text.delta", "reasoning_content"),
        ("response.reasoning_summary_text.delta", "reasoning_content"),
    ] {
        let mut encoder = ChatStreamEncoder::new(false);
        created(&mut encoder);
        assert!(push(&mut encoder, json!({"type":kind})).is_empty());
        for value in [Value::Null, json!("")] {
            assert!(push(&mut encoder, json!({"type":kind,"delta":value})).is_empty());
        }
        for (value, expected) in [
            (json!(0), "0"),
            (json!(false), "false"),
            (json!({"key":"value"}), "{\"key\":\"value\"}"),
            (json!(["raw"]), "[\"raw\"]"),
        ] {
            expect_delta(
                &mut encoder,
                json!({"type":kind,"delta":value}),
                json!({"role":"assistant",field:expected}),
            );
        }
        assert!(!encoder.is_completed());
    }
}

#[test]
fn cpa_nonfailure_body_type_takes_precedence_then_header_then_missing_type_skips() {
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    for (header, body, delta, expected) in [
        (
            Some("response.future"),
            Some("response.output_text.delta"),
            "body",
            Some("body"),
        ),
        (
            Some("response.output_text.delta"),
            Some("response.future"),
            "ignored",
            None,
        ),
        (
            Some("response.output_text.delta"),
            None,
            "header",
            Some("header"),
        ),
        (
            None,
            Some("response.output_text.delta"),
            "body only",
            Some("body only"),
        ),
        (None, None, "no type", None),
    ] {
        let mut event = json!({"delta":delta});
        if let Some(body) = body {
            event["type"] = json!(body);
        }
        let chunks = encoder.push(header, &event).unwrap();
        assert_eq!(
            chunks,
            expected
                .into_iter()
                .map(|content| { chunk(json!({"role":"assistant","content":content})) })
                .collect::<Vec<_>>()
        );
        assert!(!encoder.has_failure());
        assert!(!encoder.is_completed());
    }
    finish(&mut encoder, json!([]), "stop");
}

#[test]
fn cpa_unknown_controls_and_unmapped_items_never_turn_eof_into_completion() {
    let mut encoder = ChatStreamEncoder::new(true);
    for event in [
        json!({"type":"ping"}),
        json!({"type":"codex.rate_limits"}),
        json!({"type":"rate_limits.updated"}),
        json!({"type":"response.metadata","response":{"id":"not-the-response","status":"completed"}}),
        json!({"type":"codex.response.metadata","response":"opaque"}),
        json!({"type":"response.future_event","response":{"id":"ignored","status":"completed"}}),
        json!({"type":"response.output_text.annotation.added","annotation":{"type":"future"}}),
        json!({"type":"response.content_part.done","part":{"type":"future"}}),
        json!({"type":"response.output_item.added","item":{"type":"future","payload":"ignored"}}),
        json!({"type":"response.output_item.added","item":{"type":"image_generation_call","id":"img","result":"not a done item"}}),
        json!({"type":"response.output_item.done","item":{"type":"future","payload":"ignored"}}),
        json!({"type":"response.output_text.done","text":"not a delta"}),
        json!({"type":"response.image_generation_call.completed","result":"not a done item"}),
        json!({"future_envelope":true}),
    ] {
        assert!(push(&mut encoder, event).is_empty());
        assert_eq!(encoder.response_id(), None);
        assert!(!encoder.is_completed());
        assert!(!encoder.has_failure());
    }
    assert!(created(&mut encoder).is_empty());
    assert!(!encoder.is_completed());
    let mut terminal = response(json!([]));
    terminal.as_object_mut().unwrap().remove("usage");
    let chunks = push(
        &mut encoder,
        json!({"type":"response.completed","response":terminal}),
    );
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0]["choices"][0]["delta"], json!({}));
    assert_eq!(chunks[0]["choices"][0]["finish_reason"], "stop");
    assert_eq!(chunks[1]["choices"], json!([]));
    assert!(chunks[1]["usage"].is_null());
    assert!(encoder.is_completed());
}

#[test]
fn cpa_known_failure_headers_and_bodies_win_even_when_the_other_disagrees() {
    for failure in ["response.failed", "response.cancelled", "error"] {
        for benign in ["response.created", "response.completed", "response.future"] {
            for (header, body) in [(failure, benign), (benign, failure)] {
                let mut encoder = ChatStreamEncoder::new(true);
                created(&mut encoder);
                let error = encoder
                    .push(
                        Some(header),
                        &json!({
                            "type":body,"message":"secret-body","response":response(json!([]))
                        }),
                    )
                    .unwrap_err();
                assert_eq!(error.code(), "upstream_error");
                assert!(!error.to_string().contains("secret-body"));
                assert!(encoder.has_failure());
                assert!(!encoder.is_completed());
                assert!(encoder.push(None, &json!({
                    "type":"response.completed","response":response(json!([message("ignored")]))
                })).is_err());
            }
        }
    }
}

#[test]
fn cpa_failure_status_and_error_fields_cannot_be_reported_as_normal_termination() {
    for kind in [
        "response.created",
        "response.queued",
        "response.in_progress",
        "response.completed",
        "response.incomplete",
    ] {
        for failure in [
            json!({"status":"failed"}),
            json!({"status":"cancelled"}),
            json!({"status":"completed","error":{"message":"secret-body"}}),
        ] {
            let mut encoder = ChatStreamEncoder::new(true);
            let error = encoder
                .push(None, &json!({"type":kind,"response":failure}))
                .unwrap_err();
            assert_eq!(error.code(), "upstream_error");
            assert!(!error.to_string().contains("secret-body"));
            assert!(encoder.has_failure());
            assert!(!encoder.is_completed());
        }
    }
}

#[test]
fn cpa_interleaved_item_ids_without_output_indices_keep_sequential_tool_indices() {
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    expect_delta(
        &mut encoder,
        json!({
            "type":"response.output_item.added","item":function("a", "lookup", "ignored initial arguments")
        }),
        json!({"role":"assistant","tool_calls":[{
            "index":0,"id":"a","type":"function","function":{"name":"lookup","arguments":""}
        }]}),
    );
    expect_delta(
        &mut encoder,
        json!({
            "type":"response.output_item.added","item":{
                "type":"custom_tool_call","id":"ct_b","call_id":"b","name":"code","input":"ignored"
            }
        }),
        json!({"role":"assistant","tool_calls":[{
            "index":1,"id":"b","type":"function","function":{"name":"code","arguments":""}
        }]}),
    );
    for (kind, id, index, delta) in [
        ("response.custom_tool_call_input.delta", "ct_b", 1, "print("),
        (
            "response.function_call_arguments.delta",
            "fc_a",
            0,
            "{\"x\":",
        ),
        (
            "response.custom_tool_call_input.delta",
            "ct_b",
            1,
            "\u{4e2d})\n",
        ),
        ("response.function_call_arguments.delta", "fc_a", 0, "1}"),
    ] {
        expect_delta(
            &mut encoder,
            json!({
                "type":kind,"item_id":id,"delta":delta
            }),
            arguments(index, delta),
        );
    }
    assert!(push(&mut encoder, json!({
        "type":"response.output_item.done","item":function("a", "changed", "different full arguments")
    })).is_empty());
    assert!(push(&mut encoder, json!({
        "type":"response.function_call_arguments.delta","item_id":"fc_a","delta":"ignored after item.done"
    })).is_empty());
    expect_delta(
        &mut encoder,
        json!({
            "type":"response.custom_tool_call_input.delta","item_id":"ct_b","delta":"# tail\n"
        }),
        arguments(1, "# tail\n"),
    );
    finish(
        &mut encoder,
        json!([function("a", "lookup", "terminal snapshot")]),
        "tool_calls",
    );
}

#[test]
fn cpa_tool_lookup_prefers_item_ids_then_output_index_and_only_keyless_current_fallback() {
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    for (index, id, alias) in [(8, "a", "alias_a"), (2, "b", "alias_b")] {
        push(
            &mut encoder,
            json!({
                "type":"response.output_item.added","output_index":index,"item_id":alias,
                "item":function(id, "run", "")
            }),
        );
    }
    for (event, expected_index, delta) in [
        (
            json!({"item_id":"alias_a","output_index":2}),
            0,
            "event alias",
        ),
        (json!({"item_id":"fc_a","output_index":2}), 0, "item id"),
        (
            json!({"item_id":"unrecognized","output_index":8}),
            0,
            "output index",
        ),
        (json!({}), 1, "no identifiers"),
    ] {
        let mut event = event;
        event["type"] = json!("response.function_call_arguments.delta");
        event["delta"] = json!(delta);
        expect_delta(&mut encoder, event, arguments(expected_index, delta));
    }
    for mut event in [
        json!({"item_id":"unrecognized"}),
        json!({"output_index":99}),
        json!({"item_id":"unrecognized","output_index":99}),
    ] {
        event["type"] = json!("response.function_call_arguments.delta");
        event["delta"] = json!("must not enter another tool");
        assert!(push(&mut encoder, event).is_empty());
    }
    // Event item_id wins over the nested done item's conflicting ID.
    assert!(
        push(
            &mut encoder,
            json!({
                "type":"response.output_item.done","item_id":"alias_a","output_index":2,
                "item":function("b", "run", "no replay")
            })
        )
        .is_empty()
    );
    assert!(
        push(
            &mut encoder,
            json!({
                "type":"response.function_call_arguments.delta","item_id":"fc_a","delta":"closed"
            })
        )
        .is_empty()
    );
    expect_delta(
        &mut encoder,
        json!({
            "type":"response.function_call_arguments.delta","item_id":"fc_b","delta":"still open"
        }),
        arguments(1, "still open"),
    );
    finish(&mut encoder, json!([]), "tool_calls");
}

#[test]
fn cpa_argument_done_fallback_emits_once_does_not_close_and_never_compares_suffixes() {
    for (kind, field, prefix) in [
        (
            "function_call",
            "arguments",
            "response.function_call_arguments",
        ),
        (
            "custom_tool_call",
            "input",
            "response.custom_tool_call_input",
        ),
    ] {
        let mut encoder = ChatStreamEncoder::new(false);
        created(&mut encoder);
        let item = json!({"type":kind,"id":"item","call_id":"call","name":"run",field:""});
        push(
            &mut encoder,
            json!({"type":"response.output_item.added","item":item}),
        );
        expect_delta(
            &mut encoder,
            json!({
                "type":format!("{prefix}.done"),"item_id":"item",field:"full raw \n{not-json"
            }),
            arguments(0, "full raw \n{not-json"),
        );
        assert!(
            push(
                &mut encoder,
                json!({
                    "type":format!("{prefix}.done"),"item_id":"item",field:"conflicting replacement"
                })
            )
            .is_empty()
        );
        expect_delta(
            &mut encoder,
            json!({
                "type":format!("{prefix}.delta"),"item_id":"item","delta":"late raw"
            }),
            arguments(0, "late raw"),
        );
        assert!(push(&mut encoder, json!({
            "type":"response.output_item.done",
            "item":{"type":kind,"id":"item","call_id":"changed","name":"changed",field:"unrelated"}
        })).is_empty());
        for ending in ["delta", "done"] {
            assert!(
                push(
                    &mut encoder,
                    json!({
                        "type":format!("{prefix}.{ending}"),"item_id":"item",
                        "delta":"too late",field:"too late"
                    })
                )
                .is_empty()
            );
        }
        finish(&mut encoder, json!([]), "tool_calls");
    }
}

#[test]
fn cpa_empty_argument_done_marks_fallback_emitted_but_empty_deltas_do_not() {
    for done_first in [false, true] {
        let mut encoder = ChatStreamEncoder::new(false);
        created(&mut encoder);
        push(
            &mut encoder,
            json!({
                "type":"response.output_item.added","item":function("a", "run", "")
            }),
        );
        assert!(push(&mut encoder, json!({
            "type":if done_first {"response.function_call_arguments.done"} else {"response.function_call_arguments.delta"},
            "item_id":"fc_a","arguments":"","delta":""
        })).is_empty());
        let chunks = push(
            &mut encoder,
            json!({
                "type":"response.output_item.done","item":function("a", "run", "fallback")
            }),
        );
        assert_eq!(
            chunks,
            if done_first {
                vec![]
            } else {
                vec![chunk(arguments(0, "fallback"))]
            }
        );
        assert!(
            push(
                &mut encoder,
                json!({
                    "type":"response.function_call_arguments.delta","item_id":"fc_a","delta":"late"
                })
            )
            .is_empty()
        );
        finish(&mut encoder, json!([]), "tool_calls");
    }
}

#[test]
fn cpa_output_item_done_emits_arguments_only_after_announcement_or_a_whole_unannounced_tool() {
    for (kind, field) in [
        ("function_call", "arguments"),
        ("custom_tool_call", "input"),
    ] {
        for announced in [false, true] {
            let mut encoder = ChatStreamEncoder::new(false);
            created(&mut encoder);
            assert!(push(&mut encoder, json!({
                "type":"response.function_call_arguments.delta","item_id":"item","delta":"discarded"
            })).is_empty());
            let item = json!({
                "type":kind,"id":"item","call_id":"call","name":"run",field:"raw final\n{"
            });
            if announced {
                expect_delta(
                    &mut encoder,
                    json!({
                        "type":"response.output_item.added","item":item
                    }),
                    json!({"role":"assistant","tool_calls":[{
                        "index":0,"id":"call","type":"function","function":{"name":"run","arguments":""}
                    }]}),
                );
            }
            let event = json!({"type":"response.output_item.done","item":item});
            let expected = if announced {
                arguments(0, "raw final\n{")
            } else {
                json!({"role":"assistant","tool_calls":[{
                    "index":0,"id":"call","type":"function","function":{"name":"run","arguments":"raw final\n{"}
                }]})
            };
            expect_delta(&mut encoder, event.clone(), expected);
            assert!(push(&mut encoder, event).is_empty());
            finish(&mut encoder, json!([]), "tool_calls");
        }
    }
}

#[test]
fn cpa_image_hash_is_per_item_previous_raw_payload_not_mime_or_all_history() {
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    for (id, payload, format, emits, mime) in [
        ("a", "A", "png", true, "image/png"),
        ("a", "A", "jpeg", false, "image/jpeg"),
        ("b", "A", "jpeg", true, "image/jpeg"),
        ("a", "B", "webp", true, "image/webp"),
        ("b", "A", "png", false, "image/png"),
        ("a", "A", "unknown", true, "image/png"),
        ("a", "A", "image/future", false, "image/future"),
        ("b", "B", "image/future", true, "image/future"),
    ] {
        let event = json!({
            "type":"response.image_generation_call.partial_image",
            "item_id":id,"partial_image_b64":payload,"output_format":format
        });
        if emits {
            expect_delta(
                &mut encoder,
                event,
                json!({"role":"assistant","images":[{
                    "index":0,"type":"image_url","image_url":{"url":format!("data:{mime};base64,{payload}")}
                }]}),
            );
        } else {
            assert!(push(&mut encoder, event).is_empty());
        }
    }
    assert!(
        push(
            &mut encoder,
            json!({
                "type":"response.output_item.done","item":{
                    "type":"image_generation_call","id":"a","result":"A","output_format":"jpeg"
                }
            })
        )
        .is_empty()
    );
    expect_delta(
        &mut encoder,
        json!({
            "type":"response.output_item.done","item":{
                "type":"image_generation_call","id":"a","result":"B","output_format":"jpeg"
            }
        }),
        json!({"role":"assistant","images":[{
            "index":0,"type":"image_url","image_url":{"url":"data:image/jpeg;base64,B"}
        }]}),
    );
    finish(
        &mut encoder,
        json!([{
            "type":"image_generation_call","id":"a","result":"terminal must not replay"
        }]),
        "stop",
    );
}

#[test]
fn cpa_images_without_item_ids_are_not_deduplicated_and_empty_payloads_do_not_reset_hashes() {
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    let expected = json!({"role":"assistant","images":[{
        "index":0,"type":"image_url","image_url":{"url":"data:image/png;base64,raw"}
    }]});
    for id in [None, Some(Value::Null), Some(json!(""))] {
        for done in [false, true] {
            let mut event = if done {
                json!({"type":"response.output_item.done","output_index":7,
                    "item":{"type":"image_generation_call","result":"raw"}})
            } else {
                json!({"type":"response.image_generation_call.partial_image",
                    "output_index":7,"partial_image_b64":"raw"})
            };
            if let Some(id) = &id {
                if done {
                    event["item"]["id"] = id.clone();
                } else {
                    event["item_id"] = id.clone();
                }
            }
            for _ in 0..2 {
                expect_delta(&mut encoder, event.clone(), expected.clone());
            }
        }
    }
    let nonempty = json!({"type":"response.image_generation_call.partial_image",
        "item_id":"a","partial_image_b64":"raw"});
    expect_delta(&mut encoder, nonempty.clone(), expected);
    for result in [Value::Null, json!("")] {
        assert!(
            push(
                &mut encoder,
                json!({
                    "type":"response.image_generation_call.partial_image",
                    "item_id":"a","partial_image_b64":result
                })
            )
            .is_empty()
        );
        assert!(
            push(
                &mut encoder,
                json!({
                    "type":"response.output_item.done",
                    "item":{"type":"image_generation_call","id":"a","result":result}
                })
            )
            .is_empty()
        );
    }
    assert!(push(&mut encoder, nonempty).is_empty());
    assert!(!encoder.is_completed());
}

#[test]
fn cpa_search_annotations_images_and_unknown_controls_do_not_edit_json_model_text() {
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    let parts = ["{\"label\":\"", "\u{4e2d}\u{1f642}", "\",\"ok\":true}\n"];
    let mut chunks = Vec::new();
    for delta in parts {
        let next = push(
            &mut encoder,
            json!({"type":"response.output_text.delta","delta":delta}),
        );
        assert_eq!(
            next,
            vec![chunk(json!({"role":"assistant","content":delta}))]
        );
        chunks.extend(next);
        for event in [
            json!({"type":"response.output_text.annotation.added","annotation":{
                "type":"url_citation","start_index":u64::MAX,"end_index":0,"url":"invalid"
            }}),
            json!({"type":"response.output_item.done","item":{
                "type":"web_search_call","action":{"sources":[{"url":"must not append"}]}
            }}),
            json!({"type":"response.future","payload":"must not append"}),
        ] {
            assert!(push(&mut encoder, event).is_empty());
        }
        chunks.extend(push(
            &mut encoder,
            json!({
                "type":"response.image_generation_call.partial_image","partial_image_b64":"raw"
            }),
        ));
    }
    finish(
        &mut encoder,
        json!([message("replacement snapshot")]),
        "stop",
    );
    let emitted = text(&chunks, "content");
    assert_eq!(emitted, parts.concat());
    assert_eq!(
        serde_json::from_str::<Value>(&emitted).unwrap(),
        json!({
            "label":"\u{4e2d}\u{1f642}","ok":true
        })
    );
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.pointer("/choices/0/delta/annotations").is_none())
    );
}

#[test]
fn cpa_incomplete_reasons_and_usage_absence_are_preserved_without_snapshot_replay() {
    for (reason, expected) in [
        ("max_tokens", "length"),
        ("max_output_tokens", "length"),
        ("content_filter", "content_filter"),
        ("future_reason", "stop"),
    ] {
        for include_usage in [false, true] {
            let mut encoder = ChatStreamEncoder::new(include_usage);
            created(&mut encoder);
            let mut terminal = response(json!([message("terminal only")]));
            terminal["status"] = json!("incomplete");
            terminal["incomplete_details"] = json!({"reason":reason});
            terminal.as_object_mut().unwrap().remove("usage");
            let chunks = push(
                &mut encoder,
                json!({"type":"response.incomplete","response":terminal}),
            );
            assert_eq!(chunks.len(), if include_usage { 2 } else { 1 });
            assert_eq!(chunks[0]["choices"][0]["finish_reason"], expected);
            assert_eq!(text(&chunks, "content"), "");
            if include_usage {
                assert_eq!(chunks[1]["choices"], json!([]));
                assert_eq!(chunks[1].get("usage"), Some(&Value::Null));
            } else {
                assert!(chunks[0].get("usage").is_none());
            }
            assert!(encoder.is_completed());
        }
    }
}

#[test]
fn cpa_optional_malformed_usage_never_blocks_completion_or_invents_zero_counters() {
    for usage in [
        Value::Null,
        json!("opaque"),
        json!({"input_tokens":-1,"output_tokens":null,"total_tokens":"bad"}),
        json!({"input_tokens_details":{"cached_tokens":false}}),
    ] {
        let mut encoder = ChatStreamEncoder::new(true);
        created(&mut encoder);
        let mut terminal = response(json!([]));
        terminal["usage"] = usage;
        let chunks = push(
            &mut encoder,
            json!({"type":"response.completed","response":terminal}),
        );
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "stop");
        assert_eq!(chunks[1]["choices"], json!([]));
        for field in [
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "prompt_tokens_details",
            "completion_tokens_details",
        ] {
            assert!(chunks[1]["usage"].get(field).is_none());
        }
        assert!(encoder.is_completed());
        assert!(!encoder.has_failure());
    }
    let mut encoder = ChatStreamEncoder::new(true);
    created(&mut encoder);
    let mut terminal = response(json!([]));
    terminal["usage"] = json!({
        "input_tokens":9_007_199_254_740_993_u64,"output_tokens":"bad",
        "input_tokens_details":{"cached_tokens":9_007_199_254_740_991_u64,"invalid":false}
    });
    let chunks = push(
        &mut encoder,
        json!({"type":"response.completed","response":terminal}),
    );
    assert_eq!(
        chunks[1]["usage"],
        json!({
            "prompt_tokens":9_007_199_254_740_993_u64,
            "prompt_tokens_details":{"cached_tokens":9_007_199_254_740_991_u64}
        })
    );
    assert!(encoder.is_completed());
}
