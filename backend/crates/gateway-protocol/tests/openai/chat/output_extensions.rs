use gateway_protocol::openai::chat::{ChatStreamEncoder, chat_response, decode_chat_request};
use serde_json::{Value, json};

use super::{created, function, message, push, response, text};

// CPA ac02da6c05e18f465aa7e3ed5b0a65a2f060917d:
// codex_openai_response.go image/tool event extraction and previous-payload hashing.

fn image(id: &str, result: impl Into<String>, format: &str) -> Value {
    let mut item = json!({
        "type":"image_generation_call","id":id,"status":"completed","output_format":format
    });
    item["result"] = Value::String(result.into());
    item
}

fn complete(output: Vec<Value>) -> Value {
    let mut value = response(json!([]));
    value["output"] = Value::Array(output);
    value
}

fn terminal(response: Value) -> Value {
    let mut event = json!({"type":"response.completed"});
    event["response"] = response;
    event
}

fn item_event(index: u64, item: Value, done: bool) -> Value {
    let mut event = json!({
        "type":if done {"response.output_item.done"} else {"response.output_item.added"},
        "output_index":index
    });
    event["item"] = item;
    event
}

fn preview(index: u64, id: &str, result: impl Into<String>) -> Value {
    let mut event = json!({
        "type":"response.image_generation_call.partial_image",
        "output_index":index,"item_id":id,"partial_image_index":0,"output_format":"png"
    });
    event["partial_image_b64"] = Value::String(result.into());
    event
}

fn images(chunks: &[Value]) -> Vec<&Value> {
    chunks
        .iter()
        .filter_map(|chunk| {
            chunk
                .pointer("/choices/0/delta/images")
                .and_then(Value::as_array)
        })
        .flatten()
        .collect()
}

fn custom(id: &str, name: &str, input: &str) -> Value {
    json!({"type":"custom_tool_call","id":format!("ct_{id}"),
        "call_id":id,"name":name,"input":input})
}

fn tool_event(custom: bool, done: bool, index: u64, input: &str) -> Value {
    let prefix = if custom {
        "response.custom_tool_call_input"
    } else {
        "response.function_call_arguments"
    };
    let field = if !done {
        "delta"
    } else if custom {
        "input"
    } else {
        "arguments"
    };
    json!({"type":format!("{prefix}.{}", if done {"done"} else {"delta"}),
        "output_index":index,field:input})
}

#[test]
fn metadata_is_skipped_before_identity_and_never_emits_a_role_only_chunk() {
    let mut encoder = ChatStreamEncoder::new(false);
    for kind in ["codex.response.metadata", "response.metadata"] {
        for response in [
            json!("not a response"),
            json!({"id":"unrelated","model":17}),
        ] {
            assert!(push(&mut encoder, json!({"type":kind,"response":response})).is_empty());
            assert_eq!(encoder.response_id(), None);
            assert!(!encoder.is_completed());
        }
    }
    let chunks = created(&mut encoder);
    assert!(chunks.is_empty());
    assert!(
        push(
            &mut encoder,
            json!({
                "type":"response.metadata","response":{"id":"different","model":"different"}
            })
        )
        .is_empty()
    );
    assert_eq!(encoder.response_id(), Some("resp_test"));
    let mut encoder = ChatStreamEncoder::new(false);
    assert!(
        encoder
            .push(
                Some("codex.response.metadata"),
                &json!({"type":"response.metadata"})
            )
            .unwrap()
            .is_empty()
    );
    assert!(!encoder.has_failure());
    assert!(!encoder.is_completed());
}

#[test]
fn buffered_images_leave_json_content_and_usage_unchanged() {
    let answer = "{\"ok\":true,\"label\":\"\\u4e2d\"}\n";
    let mut native = complete(vec![
        message(answer),
        json!({"type":"web_search_call","action":{"sources":[
            {"type":"url","url":"https://example.test/source","title":"Source"}
        ]}}),
        image("img_a", "AAEC", "png"),
        image("img_b", "AQID", "jpeg"),
    ]);
    let output = chat_response(&native).unwrap();
    let result = &output["choices"][0]["message"];
    assert_eq!(result["content"], answer);
    assert_eq!(
        serde_json::from_str::<Value>(result["content"].as_str().unwrap()).unwrap()["ok"],
        true
    );
    assert!(result.get("annotations").is_none());
    assert_eq!(
        result["images"],
        json!([
            {"index":0,"type":"image_url","image_url":{"url":"data:image/png;base64,AAEC"}},
            {"index":1,"type":"image_url","image_url":{"url":"data:image/jpeg;base64,AQID"}}
        ])
    );
    assert_eq!(
        output["usage"],
        json!({
            "prompt_tokens":20,"completion_tokens":8,"total_tokens":28,
            "prompt_tokens_details":{"cached_tokens":12},
            "completion_tokens_details":{"reasoning_tokens":3}
        })
    );
    assert_eq!(native["usage"]["input_tokens"], 20);
    native.as_object_mut().unwrap().remove("usage");
    assert!(chat_response(&native).unwrap().get("usage").is_none());
}

#[test]
fn image_only_results_follow_cpa_mime_defaults_and_pass_through_slash_formats() {
    for (format, mime) in [
        ("png", "image/png"),
        ("JPEG", "image/jpeg"),
        ("jpg", "image/jpeg"),
        ("image/jpeg", "image/jpeg"),
        ("webp", "image/webp"),
        ("image/webp", "image/webp"),
        ("gif", "image/gif"),
        ("", "image/png"),
        ("svg", "image/png"),
        ("future-format", "image/png"),
        ("image/jpg", "image/jpg"),
        ("IMAGE/JPEG", "IMAGE/JPEG"),
        ("image/png;injected", "image/png;injected"),
        ("application/x-custom-image", "application/x-custom-image"),
    ] {
        let output = chat_response(&complete(vec![image("img", "AA==", format)])).unwrap();
        assert!(output["choices"][0]["message"]["content"].is_null());
        assert_eq!(output["choices"][0]["finish_reason"], "stop");
        assert_eq!(
            output["choices"][0]["message"]["images"][0]["image_url"]["url"],
            format!("data:{mime};base64,AA==")
        );
    }
    for format in [None, Some(Value::Null)] {
        let mut item = image("img", "AAEC", "png");
        item.as_object_mut().unwrap().remove("output_format");
        if let Some(format) = format {
            item["output_format"] = format;
        }
        assert_eq!(
            chat_response(&complete(vec![item])).unwrap()["choices"][0]["message"]["images"][0]["image_url"]
                ["url"],
            "data:image/png;base64,AAEC"
        );
    }
}

#[test]
fn nonempty_image_payloads_are_forwarded_verbatim_without_base64_validation() {
    for payload in [
        "A",
        "AAA",
        "A===",
        "AB==",
        "AA=A",
        "AAAA\n",
        "____",
        "secret-body",
    ] {
        let item = image("img", payload, "png");
        let expected = format!("data:image/png;base64,{payload}");
        let buffered = chat_response(&complete(vec![item.clone()])).unwrap();
        assert_eq!(
            buffered["choices"][0]["message"]["images"][0]["image_url"]["url"],
            expected
        );
        for event in [item_event(0, item, true), preview(0, "img", payload)] {
            let mut encoder = ChatStreamEncoder::new(false);
            let chunks = push(&mut encoder, event);
            assert_eq!(images(&chunks).len(), 1);
            assert_eq!(images(&chunks)[0]["image_url"]["url"], expected);
            assert!(!encoder.has_failure());
            assert!(!encoder.is_completed());
        }
    }
    for (format, mime) in [
        (json!("svg"), "image/png"),
        (json!("image/png;injected"), "image/png;injected"),
        (json!(true), "image/png"),
        (
            json!("a-format-name-longer-than-thirty-two-characters"),
            "image/png",
        ),
    ] {
        let mut item = image("img", "AAEC", "png");
        item["output_format"] = format.clone();
        let expected = format!("data:{mime};base64,AAEC");
        let buffered = chat_response(&complete(vec![item.clone()])).unwrap();
        assert_eq!(
            buffered["choices"][0]["message"]["images"][0]["image_url"]["url"],
            expected
        );
        let mut event = preview(0, "img", "AAEC");
        event["output_format"] = format;
        for event in [item_event(0, item, true), event] {
            let mut encoder = ChatStreamEncoder::new(false);
            let chunks = push(&mut encoder, event);
            assert_eq!(images(&chunks)[0]["image_url"]["url"], expected);
        }
    }
}

#[test]
fn pending_or_empty_images_are_skipped_and_only_an_explicit_terminal_completes() {
    for status in ["in_progress", "generating"] {
        for result in [None, Some(Value::Null)] {
            let mut item = json!({"type":"image_generation_call","id":"img","status":status});
            if let Some(result) = result {
                item["result"] = result;
            }
            let mut encoder = ChatStreamEncoder::new(false);
            created(&mut encoder);
            assert!(push(&mut encoder, item_event(0, item.clone(), false)).is_empty());
            assert!(!encoder.is_completed());
            let buffered = chat_response(&complete(vec![item.clone()])).unwrap();
            assert!(buffered["choices"][0]["message"].get("images").is_none());
            assert_eq!(buffered["choices"][0]["finish_reason"], "stop");
            assert!(push(&mut encoder, item_event(0, item.clone(), true)).is_empty());
            let chunks = push(&mut encoder, terminal(complete(vec![item])));
            assert!(images(&chunks).is_empty());
            assert!(encoder.is_completed());
        }
    }
    let mut encoder = ChatStreamEncoder::new(false);
    for kind in ["in_progress", "generating", "completed"] {
        assert!(
            push(
                &mut encoder,
                json!({
                    "type":format!("response.image_generation_call.{kind}"),
                    "output_index":0,"item_id":"img"
                })
            )
            .is_empty()
        );
    }
    for result in [Value::Null, json!("")] {
        let mut event = preview(0, "img", "");
        event["partial_image_b64"] = result;
        assert!(push(&mut encoder, event).is_empty());
    }
    assert!(!encoder.is_completed());
    assert_eq!(encoder.response_id(), None);
    let chunks = push(
        &mut encoder,
        terminal(complete(vec![image("img", "AAEC", "png")])),
    );
    assert!(images(&chunks).is_empty());
    assert!(encoder.is_completed());
}

#[test]
fn streamed_images_use_frame_local_indices_and_only_deduplicate_the_previous_payload() {
    let mut encoder = ChatStreamEncoder::new(true);
    let mut chunks = created(&mut encoder);
    for (index, id) in [(2, "img_b"), (1, "img_a")] {
        push(
            &mut encoder,
            item_event(
                index,
                json!({"type":"image_generation_call","id":id,"status":"in_progress"}),
                false,
            ),
        );
    }
    chunks.extend(push(&mut encoder, preview(2, "img_b", "AAEC")));
    assert!(push(&mut encoder, preview(2, "img_b", "AAEC")).is_empty());
    chunks.extend(push(&mut encoder, preview(2, "img_b", "AQID")));
    chunks.extend(push(&mut encoder, preview(2, "img_b", "AAEC")));
    let done = item_event(2, image("img_b", "AQID", "png"), true);
    chunks.extend(encoder.push(None, &done).unwrap());
    assert!(encoder.push(None, &done).unwrap().is_empty());
    chunks.extend(push(&mut encoder, preview(2, "img_b", "AAEC")));
    let done = push(
        &mut encoder,
        item_event(1, image("img_a", "AgME", "webp"), true),
    );
    assert_eq!(images(&done)[0]["index"], 0);
    assert!(!encoder.is_completed());
    chunks.extend(done);
    chunks.extend(push(&mut encoder, json!({
        "type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"{\"ok\":"
    })));
    chunks.extend(push(
        &mut encoder,
        json!({
            "type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"true}"
        }),
    ));
    let final_chunks = push(
        &mut encoder,
        terminal(complete(vec![
            message("{\"ok\":true}"),
            image("img_a", "AgME", "webp"),
            image("img_b", "AQID", "png"),
            json!({"type":"web_search_call","action":{"sources":[{"url":"ignored"}]}}),
        ])),
    );
    assert!(images(&final_chunks).is_empty());
    chunks.extend(final_chunks);
    let image_chunks = images(&chunks);
    assert_eq!(image_chunks.len(), 6);
    assert_eq!(
        image_chunks
            .iter()
            .map(|entry| entry["index"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        image_chunks
            .iter()
            .map(|entry| entry["image_url"]["url"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "data:image/png;base64,AAEC",
            "data:image/png;base64,AQID",
            "data:image/png;base64,AAEC",
            "data:image/png;base64,AQID",
            "data:image/png;base64,AAEC",
            "data:image/webp;base64,AgME"
        ]
    );
    assert_eq!(text(&chunks, "content"), "{\"ok\":true}");
    assert_eq!(
        chunks
            .iter()
            .filter(|chunk| chunk.get("usage").is_some_and(|v| !v.is_null()))
            .count(),
        1
    );
    assert!(encoder.is_completed());
}

#[test]
fn a_final_image_matching_an_older_preview_is_restored_once() {
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    push(&mut encoder, preview(0, "img", "AAEC"));
    push(&mut encoder, preview(0, "img", "AQID"));
    let done = item_event(0, image("img", "AAEC", "png"), true);
    let chunks = encoder.push(None, &done).unwrap();
    assert_eq!(images(&chunks).len(), 1);
    assert_eq!(
        images(&chunks)[0]["image_url"]["url"],
        "data:image/png;base64,AAEC"
    );
    assert!(encoder.push(None, &done).unwrap().is_empty());
    let chunks = push(
        &mut encoder,
        terminal(complete(vec![image("img", "AAEC", "png")])),
    );
    assert!(images(&chunks).is_empty());
    assert!(encoder.is_completed());
}

#[test]
fn terminal_only_images_are_buffered_output_but_never_replayed_in_streams() {
    let native = complete(vec![image("a", "AAEC", "png"), image("b", "AQID", "jpeg")]);
    let buffered = chat_response(&native).unwrap();
    let mut encoder = ChatStreamEncoder::new(false);
    let chunks = push(&mut encoder, terminal(native));
    assert_eq!(
        buffered["choices"][0]["message"]["images"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(images(&chunks).is_empty());
    assert_eq!(text(&chunks, "content"), "");
    assert_eq!(
        chunks.last().unwrap()["choices"][0]["finish_reason"],
        "stop"
    );
    assert!(encoder.is_completed());
}

#[test]
fn previews_and_done_images_do_not_bypass_eof_or_failures_but_allow_empty_completion() {
    for done in [false, true] {
        for ending in [
            None,
            Some(json!({"type":"response.failed"})),
            Some(json!({"type":"response.cancelled"})),
            Some(json!({"type":"error"})),
            Some(terminal(complete(vec![]))),
            Some(terminal(complete(vec![json!({
                "type":"image_generation_call","id":"img","status":"completed"
            })]))),
            Some(terminal(complete(vec![json!({
                "type":"image_generation_call","id":"img","status":"in_progress","result":"AAEC"
            })]))),
        ] {
            let mut encoder = ChatStreamEncoder::new(false);
            created(&mut encoder);
            let event = if done {
                item_event(0, image("img", "AAEC", "png"), true)
            } else {
                preview(0, "img", "AAEC")
            };
            let chunks = push(&mut encoder, event);
            assert_eq!(images(&chunks).len(), 1);
            assert!(
                chunks
                    .iter()
                    .all(|chunk| chunk["choices"][0]["finish_reason"].is_null())
            );
            assert!(!encoder.is_completed());
            if let Some(ending) = ending {
                if ending["type"] == "response.completed" {
                    let chunks = push(&mut encoder, ending);
                    assert!(images(&chunks).is_empty());
                    assert_eq!(
                        chunks.last().unwrap()["choices"][0]["finish_reason"],
                        "stop"
                    );
                    assert!(encoder.is_completed());
                    assert!(!encoder.has_failure());
                } else {
                    assert!(encoder.push(None, &ending).is_err());
                    assert!(encoder.has_failure());
                    assert!(encoder.push(None, &terminal(complete(vec![]))).is_err());
                    assert!(!encoder.is_completed());
                }
            }
        }
    }
}

#[test]
fn image_dedup_uses_item_id_and_raw_payload_without_output_index_or_mime_identity() {
    for (event, expected_images, completed) in [
        (preview(1, "img", "AAEC"), 0, false),
        (preview(0, "other", "AAEC"), 1, false),
        (item_event(0, message("changed type"), true), 0, false),
        (
            terminal(complete(vec![image("other", "AAEC", "png")])),
            0,
            true,
        ),
    ] {
        let mut encoder = ChatStreamEncoder::new(false);
        created(&mut encoder);
        push(&mut encoder, preview(0, "img", "AAEC"));
        let chunks = push(&mut encoder, event);
        assert_eq!(images(&chunks).len(), expected_images);
        assert_eq!(encoder.is_completed(), completed);
        assert!(!encoder.has_failure());
    }
    for (event, expected_images, completed) in [
        (item_event(0, image("img", "AQID", "png"), true), 1, false),
        (item_event(0, image("img", "AAEC", "jpeg"), true), 0, false),
        (
            terminal(complete(vec![image("img", "AAEC", "jpeg")])),
            0,
            true,
        ),
        (
            terminal(complete(vec![image("img", "AQID", "png")])),
            0,
            true,
        ),
        (preview(0, "img", "AgME"), 1, false),
    ] {
        let mut encoder = ChatStreamEncoder::new(false);
        created(&mut encoder);
        push(
            &mut encoder,
            item_event(0, image("img", "AAEC", "png"), true),
        );
        let chunks = push(&mut encoder, event);
        assert_eq!(images(&chunks).len(), expected_images);
        assert_eq!(encoder.is_completed(), completed);
        assert!(!encoder.has_failure());
    }
    let duplicate = complete(vec![
        image("img", "AAEC", "png"),
        image("img", "AAEC", "png"),
    ]);
    let buffered = chat_response(&duplicate).unwrap();
    assert_eq!(
        buffered["choices"][0]["message"]["images"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut encoder = ChatStreamEncoder::new(false);
    assert!(images(&push(&mut encoder, terminal(duplicate))).is_empty());
    assert!(encoder.is_completed());
}

#[test]
fn chat_image_output_has_no_extra_single_or_aggregate_64_mib_budget() {
    const HALF: usize = 32 * 1024 * 1024;
    // Sequential scopes keep boundary fixtures from multiplying peak memory.
    {
        let native = complete(vec![
            json!({"type":"unknown_semantic"}),
            image("a", "A".repeat(HALF), "png"),
            image("b", "A".repeat(HALF), "png"),
            image("c", "AAEC", "png"),
        ]);
        {
            let buffered = chat_response(&native).unwrap();
            assert_eq!(
                buffered["choices"][0]["message"]["images"]
                    .as_array()
                    .unwrap()
                    .len(),
                3
            );
        }
        let mut encoder = ChatStreamEncoder::new(false);
        let chunks = push(&mut encoder, terminal(native));
        assert!(images(&chunks).is_empty());
        assert!(encoder.is_completed());
    }
    {
        let native = complete(vec![image("a", "A".repeat(2 * HALF + 4), "png")]);
        let buffered = chat_response(&native).unwrap();
        assert_eq!(
            buffered["choices"][0]["message"]["images"][0]["image_url"]["url"]
                .as_str()
                .unwrap()
                .len(),
            "data:image/png;base64,".len() + 2 * HALF + 4
        );
    }
    {
        let mut encoder = ChatStreamEncoder::new(false);
        created(&mut encoder);
        for (index, id) in [(0, "a"), (1, "b"), (2, "c")] {
            let event = item_event(index, image(id, "A".repeat(HALF), "png"), true);
            {
                let chunks = encoder.push(None, &event).unwrap();
                assert_eq!(images(&chunks).len(), 1);
                assert_eq!(
                    images(&chunks)[0]["image_url"]["url"]
                        .as_str()
                        .unwrap()
                        .len(),
                    "data:image/png;base64,".len() + HALF
                );
            }
            assert!(encoder.push(None, &event).unwrap().is_empty());
        }
        let chunks = push(&mut encoder, terminal(complete(vec![])));
        assert!(images(&chunks).is_empty());
        assert!(encoder.is_completed());
    }
    {
        let mut encoder = ChatStreamEncoder::new(false);
        created(&mut encoder);
        let event = preview(0, "a", "A".repeat(2 * HALF + 4));
        assert_eq!(images(&encoder.push(None, &event).unwrap()).len(), 1);
        assert!(encoder.push(None, &event).unwrap().is_empty());
        drop(event);
        let event = item_event(0, image("a", "B".repeat(2 * HALF + 4), "png"), true);
        assert_eq!(images(&encoder.push(None, &event).unwrap()).len(), 1);
        assert!(!encoder.has_failure());
        assert!(!encoder.is_completed());
    }
}

#[test]
fn image_versions_and_item_ids_do_not_have_a_chat_specific_4096_cap() {
    let mut encoder = ChatStreamEncoder::new(false);
    for index in 0..=4096 {
        let chunks = push(&mut encoder, preview(0, "img", format!("{index:04}")));
        assert_eq!(images(&chunks).len(), 1);
    }
    assert!(push(&mut encoder, preview(0, "img", "4096")).is_empty());
    assert_eq!(
        images(&push(&mut encoder, preview(0, "img", "0000"))).len(),
        1
    );
    assert!(!encoder.has_failure());
    assert!(!encoder.is_completed());
    let mut encoder = ChatStreamEncoder::new(false);
    for index in 0..=4096 {
        assert!(
            push(
                &mut encoder,
                json!({
                    "type":"response.image_generation_call.in_progress","output_index":index
                })
            )
            .is_empty()
        );
        let chunks = push(
            &mut encoder,
            preview(index, &format!("img_{index}"), "AAEC"),
        );
        assert_eq!(images(&chunks).len(), 1);
    }
    assert!(!encoder.has_failure());
    assert!(!encoder.is_completed());
}

#[test]
fn search_sources_are_ignored_without_faking_output_or_editing_json() {
    for sources in [
        Value::Null,
        json!([]),
        json!([{"type":"url","url":"https://example.test","title":"Source"}]),
        json!([{"type":"future_source","url":"javascript:invalid"}]),
        json!({"unrecognized":"metadata"}),
        json!("metadata"),
    ] {
        let search = json!({"type":"web_search_call","id":"search",
            "action":{"type":"search","sources":sources}});
        let native = complete(vec![search.clone(), message("{\"ok\":true}")]);
        let buffered = chat_response(&native).unwrap();
        assert_eq!(
            buffered["choices"][0]["message"]["content"],
            "{\"ok\":true}"
        );
        assert!(
            buffered["choices"][0]["message"]
                .get("annotations")
                .is_none()
        );
        let mut encoder = ChatStreamEncoder::new(false);
        created(&mut encoder);
        assert!(push(&mut encoder, item_event(0, search.clone(), true)).is_empty());
        let mut chunks = push(
            &mut encoder,
            json!({
                "type":"response.output_text.delta","delta":"{\"ok\":true}"
            }),
        );
        chunks.extend(push(&mut encoder, terminal(native)));
        assert_eq!(text(&chunks, "content"), "{\"ok\":true}");
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.pointer("/choices/0/delta/annotations").is_none())
        );
        let only_search = complete(vec![search]);
        let buffered = chat_response(&only_search).unwrap();
        assert!(buffered["choices"][0]["message"]["content"].is_null());
        assert_eq!(buffered["choices"][0]["finish_reason"], "stop");
        let mut encoder = ChatStreamEncoder::new(false);
        let chunks = push(&mut encoder, terminal(only_search));
        assert_eq!(text(&chunks, "content"), "");
        assert!(encoder.is_completed());
    }
}

#[test]
fn native_unicode_text_is_unchanged_while_annotations_and_source_appendices_are_omitted() {
    let prefix = "\u{4e2d}\u{1f642}";
    let citation = json!({"type":"url_citation","url":"https://example.test/native",
        "title":"Native","start_index":0,"end_index":2});
    let mut item = message("");
    item["content"] = json!([
        {"type":"output_text","text":prefix,"annotations":[]},
        {"type":"output_text","text":"OK","annotations":[citation]}
    ]);
    let native = complete(vec![
        item,
        image("img", "AAEC", "png"),
        json!({"type":"web_search_call","action":{"sources":[{"url":"ignored"}]}}),
    ]);
    let buffered = chat_response(&native).unwrap();
    assert_eq!(
        buffered["choices"][0]["message"]["content"],
        format!("{prefix}OK")
    );
    assert!(
        buffered["choices"][0]["message"]
            .get("annotations")
            .is_none()
    );
    let mut encoder = ChatStreamEncoder::new(false);
    let mut chunks = created(&mut encoder);
    for (index, delta) in [(0, prefix), (1, "OK")] {
        chunks.extend(push(&mut encoder, json!({
            "type":"response.output_text.delta","output_index":0,"content_index":index,"delta":delta
        })));
    }
    chunks.extend(push(
        &mut encoder,
        json!({
            "type":"response.output_text.annotation.added","output_index":0,
            "content_index":1,"annotation_index":0,"annotation":citation
        }),
    ));
    chunks.extend(push(&mut encoder, terminal(native)));
    assert_eq!(text(&chunks, "content"), format!("{prefix}OK"));
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.pointer("/choices/0/delta/annotations").is_none())
    );
    assert!(encoder.is_completed());
}

#[test]
fn ignored_citation_offsets_cannot_overflow_or_block_a_terminal() {
    let citation = json!({"type":"url_citation","url":"https://example.test",
        "title":"Overflow","start_index":0,"end_index":u64::MAX});
    let mut item = message("A");
    item["content"].as_array_mut().unwrap().push(json!({
        "type":"output_text","text":"B","annotations":[citation]
    }));
    let native = complete(vec![item]);
    let buffered = chat_response(&native).unwrap();
    assert_eq!(buffered["choices"][0]["message"]["content"], "AB");
    assert!(
        buffered["choices"][0]["message"]
            .get("annotations")
            .is_none()
    );
    let mut encoder = ChatStreamEncoder::new(false);
    created(&mut encoder);
    assert!(
        push(
            &mut encoder,
            json!({
                "type":"response.output_text.annotation.added","output_index":0,
                "content_index":1,"annotation_index":0,"annotation":citation
            }),
        )
        .is_empty()
    );
    let mut chunks = push(
        &mut encoder,
        json!({"type":"response.output_text.delta","delta":"AB"}),
    );
    chunks.extend(push(&mut encoder, terminal(native)));
    assert_eq!(text(&chunks, "content"), "AB");
    assert!(encoder.is_completed());
    assert!(!encoder.has_failure());
}

#[test]
fn custom_function_envelopes_roundtrip_by_declaration_with_exact_raw_strings() {
    let raw = "*** Begin Patch\n\u{4e2d}\n*** End Patch\n";
    let output = chat_response(&complete(vec![
        function("fn", "lookup", "{\"x\":1}"),
        custom("ct", "apply.patch", raw),
    ]))
    .unwrap();
    assert_eq!(output["choices"][0]["finish_reason"], "tool_calls");
    let assistant = &output["choices"][0]["message"];
    assert_eq!(
        assistant["tool_calls"][1],
        json!({
            "type":"function","id":"ct","function":{"name":"apply.patch","arguments":raw}
        })
    );
    assert!(assistant["tool_calls"][1].get("custom").is_none());
    let decoded = decode_chat_request(json!({
        "model":"gpt-test",
        "tools":[{"type":"function","function":{"name":"lookup"}},
            {"type":"custom","custom":{"name":"apply.patch"}}],
        "messages":[assistant,
            {"role":"tool","tool_call_id":"ct","content":"patched"},
            {"role":"tool","tool_call_id":"fn","content":"found"}]
    }))
    .unwrap();
    assert_eq!(
        decoded.responses["input"],
        json!([
            {"type":"function_call","call_id":"fn","name":"lookup","arguments":"{\"x\":1}"},
            {"type":"custom_tool_call","call_id":"ct","name":"apply.patch","input":raw},
            {"type":"custom_tool_call_output","call_id":"ct","output":"patched"},
            {"type":"function_call_output","call_id":"fn","output":"found"}
        ])
    );
    let empty = chat_response(&complete(vec![custom("ct", "apply.patch", "")])).unwrap();
    assert_eq!(
        empty["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        ""
    );
}

#[test]
fn mixed_custom_function_streams_use_announcement_order_and_never_replay_snapshot_suffixes() {
    let mut encoder = ChatStreamEncoder::new(false);
    let mut chunks = created(&mut encoder);
    assert!(push(&mut encoder, tool_event(true, false, 1, "print(")).is_empty());
    chunks.extend(push(
        &mut encoder,
        item_event(0, function("fn", "lookup", ""), false),
    ));
    chunks.extend(push(
        &mut encoder,
        item_event(1, custom("ct", "run", ""), false),
    ));
    chunks.extend(push(&mut encoder, tool_event(false, false, 0, "{\"x\":")));
    chunks.extend(push(&mut encoder, tool_event(true, false, 1, "print(")));
    chunks.extend(push(&mut encoder, tool_event(true, false, 1, "1")));
    chunks.extend(push(&mut encoder, tool_event(true, true, 1, "print(1)")));
    assert!(push(&mut encoder, tool_event(true, true, 1, "print(1)")).is_empty());
    chunks.extend(push(&mut encoder, tool_event(false, true, 0, "{\"x\":1")));
    chunks.extend(push(
        &mut encoder,
        item_event(1, custom("ct", "run", "print(1)\n"), true),
    ));
    chunks.extend(push(
        &mut encoder,
        terminal(complete(vec![
            function("fn", "lookup", "{\"x\":1}"),
            custom("ct", "run", "print(1)\n"),
        ])),
    ));
    let mut arguments = [String::new(), String::new()];
    let mut announcements = Vec::new();
    for call in chunks
        .iter()
        .filter_map(|chunk| chunk.pointer("/choices/0/delta/tool_calls/0"))
    {
        let index = call["index"].as_u64().unwrap() as usize;
        arguments[index].push_str(call["function"]["arguments"].as_str().unwrap());
        assert!(call.get("custom").is_none());
        if let Some(id) = call["id"].as_str() {
            assert_eq!(call["type"], "function");
            announcements.push((index, id));
        }
    }
    assert_eq!(announcements, [(0, "fn"), (1, "ct")]);
    assert_eq!(arguments, ["{\"x\":", "print(1"]);
    assert_eq!(
        chunks.last().unwrap()["choices"][0]["finish_reason"],
        "tool_calls"
    );
    assert!(encoder.is_completed());
}

#[test]
fn argument_done_allows_late_deltas_but_item_done_ignores_them_without_suffix_replay() {
    for is_custom in [false, true] {
        for item_done in [false, true] {
            let tool = |input| {
                if is_custom {
                    custom("call", "run", input)
                } else {
                    function("call", "run", input)
                }
            };
            let mut encoder = ChatStreamEncoder::new(false);
            created(&mut encoder);
            push(&mut encoder, item_event(0, tool(""), false));
            let mut chunks = if item_done {
                push(&mut encoder, item_event(0, tool("x"), true))
            } else {
                push(&mut encoder, tool_event(is_custom, true, 0, "x"))
            };
            let late = push(&mut encoder, tool_event(is_custom, false, 0, "y"));
            assert_eq!(late.is_empty(), item_done);
            chunks.extend(late);
            chunks.extend(push(&mut encoder, terminal(complete(vec![tool("xy")]))));
            let arguments: String = chunks
                .iter()
                .filter_map(|chunk| {
                    chunk
                        .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                        .and_then(Value::as_str)
                })
                .collect();
            assert_eq!(arguments, if item_done { "x" } else { "xy" });
            assert!(!encoder.has_failure());
            assert!(encoder.is_completed());

            let mut encoder = ChatStreamEncoder::new(false);
            created(&mut encoder);
            push(&mut encoder, item_event(0, tool(""), false));
            let mut chunks = push(&mut encoder, tool_event(is_custom, false, 0, "x"));
            chunks.extend(push(&mut encoder, tool_event(is_custom, true, 0, "xy")));
            chunks.extend(push(&mut encoder, item_event(0, tool("xyz"), true)));
            chunks.extend(push(&mut encoder, terminal(complete(vec![tool("xyz!")]))));
            let arguments: String = chunks
                .iter()
                .filter_map(|chunk| {
                    chunk
                        .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                        .and_then(Value::as_str)
                })
                .collect();
            assert_eq!(arguments, "x");
            assert!(encoder.is_completed());
        }
    }
}

#[test]
fn unknown_output_items_and_events_are_skipped_without_changing_request_tool_boundaries() {
    for kind in [
        "computer_call",
        "file_search_call",
        "mcp_approval_request",
        "future_output",
    ] {
        let item = json!({"type":kind,"payload":"secret-body"});
        let buffered = chat_response(&complete(vec![message("text"), item.clone()])).unwrap();
        assert_eq!(buffered["choices"][0]["message"]["content"], "text");
        assert!(!buffered.to_string().contains("secret-body"));
        let mut encoder = ChatStreamEncoder::new(false);
        assert!(push(&mut encoder, item_event(0, item, true)).is_empty());
        assert!(!encoder.has_failure());
        assert!(!encoder.is_completed());
    }
    for kind in [
        "response.computer_call.completed",
        "response.file_search_call.completed",
        "response.mcp_approval_request",
        "codex.response.unknown",
    ] {
        let mut encoder = ChatStreamEncoder::new(false);
        assert!(push(&mut encoder, json!({"type":kind})).is_empty());
        assert!(!encoder.is_completed());
        assert!(!encoder.has_failure());
    }
    for item in [
        json!({"type":"custom_tool_call","call_id":"ct","name":"run","input":null}),
        json!({"type":"custom_tool_call","call_id":"ct","name":"run","input":"x","namespace":"ns"}),
    ] {
        let expected = item["input"].as_str().unwrap_or("");
        let buffered = chat_response(&complete(vec![item.clone()])).unwrap();
        assert_eq!(
            buffered["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            expected
        );
        let mut encoder = ChatStreamEncoder::new(false);
        let chunks = push(&mut encoder, item_event(0, item.clone(), true));
        assert_eq!(
            chunks[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            expected
        );
        assert!(!encoder.is_completed());
        assert!(!encoder.has_failure());
    }
}
