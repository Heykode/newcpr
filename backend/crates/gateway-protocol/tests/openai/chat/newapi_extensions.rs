use super::*;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../gateway-api/tests/openai/fixtures/newapi-chat-extensions.json"
    ))
    .unwrap()
}

#[test]
fn newapi_typed_request_preserves_custom_history_and_builtin_tool_types() {
    let fixture = fixture();
    let forwarded = &fixture["request_forwarded"];
    let decoded = decode_chat_request(forwarded.clone()).unwrap();
    assert_eq!(decoded.responses["tools"][0]["type"], "image_generation");
    assert_eq!(decoded.responses["tools"][1]["type"], "custom");
    assert_eq!(decoded.responses["tools"][1]["name"], "code");
    assert_eq!(decoded.responses["tools"][2]["type"], "web_search");
    assert_eq!(decoded.responses["tools"][2]["search_context_size"], "low");
    assert_eq!(
        decoded.responses["input"][1],
        json!({"type":"custom_tool_call","call_id":"call_old","name":"code","input":"print(1)"})
    );
    assert_eq!(
        decoded.responses["input"][2],
        json!({"type":"custom_tool_call_output","call_id":"call_old","output":"1"})
    );
    // New API's typed tool declaration loses image options before reaching CPR.
    assert_eq!(
        fixture["request_source"]["tools"][0]["output_format"],
        "png"
    );
    assert!(forwarded["tools"][0].get("output_format").is_none());
    assert!(decoded.responses["tools"][0].get("output_format").is_none());
}

#[test]
fn newapi_images_require_transparent_forwarding_and_never_pollute_content() {
    let fixture = fixture();
    let source = &fixture["image_response_source"];
    let complete = chat_response(source).unwrap();
    let forwarded = &fixture["image_complete_raw_forwarded"];
    assert_eq!(
        complete["choices"][0]["message"]["images"],
        forwarded["choices"][0]["message"]["images"]
    );
    assert!(
        fixture["image_complete_forwarded"]["choices"][0]["message"]
            .get("images")
            .is_none()
    );
    assert!(complete["choices"][0]["message"]["content"].is_null());
    assert_eq!(
        complete["choices"][0]["message"]["content"],
        forwarded["choices"][0]["message"]["content"]
    );
    assert_eq!(
        complete["usage"]["total_tokens"],
        forwarded["usage"]["total_tokens"]
    );
    let mut encoder = ChatStreamEncoder::new(true);
    let mut chunks = Vec::new();
    for (index, item) in source["output"].as_array().unwrap().iter().enumerate() {
        chunks.extend(push(
            &mut encoder,
            json!({"type":"response.output_item.done","output_index":index,"item":item}),
        ));
    }
    chunks.extend(push(
        &mut encoder,
        json!({"type":"response.completed","response":source}),
    ));
    assert_eq!(text(&chunks, "content"), "");
    let images: Vec<_> = chunks
        .iter()
        .filter_map(|chunk| chunk["choices"][0]["delta"]["images"].as_array())
        .flatten()
        .cloned()
        .collect();
    assert_eq!(json!(images), forwarded["choices"][0]["message"]["images"]);
    assert!(
        fixture["stream_typed_forwarded"]["choices"][0]["delta"]
            .get("images")
            .is_none()
    );
    assert_eq!(
        fixture["stream_raw_forwarded"]["choices"][0]["delta"]["images"],
        fixture["stream_source"]["choices"][0]["delta"]["images"]
    );
    assert!(encoder.is_completed());
    assert_eq!(
        fixture["stream_source"]["choices"][0]["delta"]["annotations"],
        fixture["stream_typed_forwarded"]["choices"][0]["delta"]["annotations"]
    );
}

#[test]
fn newapi_custom_function_envelope_survives_typed_reformatting_and_restores() {
    let fixture = fixture();
    let original = &fixture["stream_source"]["choices"][0]["delta"]["tool_calls"][0];
    let typed = &fixture["stream_typed_forwarded"]["choices"][0]["delta"]["tool_calls"][0];
    assert_eq!(original["function"]["arguments"], "print(1)");
    assert_eq!(typed["type"], "function");
    assert_eq!(typed["function"]["arguments"], "print(1)");
    assert!(typed.get("custom").is_none());
    let output = json!([{"type":"custom_tool_call","id":"ct_1","call_id":"call_1","name":"code","input":"print(1)"}]);
    let complete = chat_response(&response(output)).unwrap();
    assert_eq!(
        complete["choices"][0]["message"]["tool_calls"][0]["function"],
        original["function"]
    );
    assert!(
        complete["choices"][0]["message"]["tool_calls"][0]
            .get("custom")
            .is_none()
    );
    let mut next = request();
    next["tools"] = fixture["request_forwarded"]["tools"].clone();
    let mut returned_call = typed.clone();
    returned_call.as_object_mut().unwrap().remove("index");
    next["messages"] = json!([
        {"role":"assistant","content":null,"tool_calls":[returned_call]},
        {"role":"tool","tool_call_id":"call_1","content":"1"}
    ]);
    let next = decode_chat_request(next).unwrap();
    assert_eq!(next.responses["input"][0]["type"], "custom_tool_call");
    assert_eq!(next.responses["input"][0]["input"], "print(1)");
    assert_eq!(
        next.responses["input"][1]["type"],
        "custom_tool_call_output"
    );
    assert_eq!(next.responses["input"][1]["call_id"], "call_1");
}

#[test]
fn newapi_flat_custom_declarations_need_raw_forwarding_or_nested_form() {
    let fixture = fixture();
    let direct = decode_chat_request(fixture["flat_custom_request_source"].clone()).unwrap();
    assert_eq!(direct.responses["tools"][0]["name"], "code");
    assert!(decode_chat_request(fixture["flat_custom_request_forwarded"].clone()).is_err());
}
