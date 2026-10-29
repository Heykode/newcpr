use super::*;

#[test]
fn buffered_tool_identity_rejects_unusable_calls_before_delivery() {
    let valid = function("call_one", "lookup", "{}");
    let mut malformed = Vec::new();
    for field in ["call_id", "name"] {
        for value in [Value::Null, json!(""), json!("  "), json!(17), json!({})] {
            let mut item = valid.clone();
            item[field] = value;
            malformed.push(json!([item]));
        }
        let mut item = valid.clone();
        item.as_object_mut().unwrap().remove(field);
        malformed.push(json!([item]));
    }
    malformed.push(json!([valid, function("call_one", "other", "{}")]));
    malformed.push(json!([
        function("call_one", "lookup", "{}"),
        {"type":"custom_tool_call","call_id":"call_one","name":"code","input":"print(1)"}
    ]));
    for output in malformed {
        let native = response(output);
        let error = chat_response(&native)
            .expect_err("unusable identity cannot be a successful tool reply");
        assert_eq!(error.code(), "invalid_upstream_response");
    }
}

#[test]
fn buffered_tool_identity_valid_extensions_roundtrip_without_rewriting_input() {
    let arguments = "  {\"partial\": \n";
    let custom_input = "*** Begin Patch\n+print(1)\n*** End Patch";
    let native = response(json!([
        function("call_one", "lookup", arguments),
        {"type":"custom_tool_call","call_id":"call_two","name":"code","input":custom_input},
        {"type":"reasoning","summary":[{"type":"summary_text","text":"explanation"}]},
        {"type":"image_generation_call","result":"synthetic-image","output_format":"png"}
    ]));
    let before = native.clone();
    let chat = chat_response(&native).unwrap();
    assert_eq!(native, before);
    let assistant = &chat["choices"][0]["message"];
    assert_eq!(assistant["reasoning_content"], "explanation");
    assert_eq!(assistant["images"].as_array().unwrap().len(), 1);
    let followup = decode_chat_request(json!({
        "model":"gpt-test","use_websocket":true,
        "tools":[{"type":"custom","custom":{"name":"code","format":{"type":"text"}}}],
        "messages":[assistant,
            {"role":"tool","tool_call_id":"call_one","content":"lookup result"},
            {"role":"tool","tool_call_id":"call_two","content":"patch result"}
        ]
    }))
    .unwrap();
    let calls: Vec<_> = followup.responses["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        })
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["arguments"], arguments);
    assert_eq!(calls[1]["input"], custom_input);
    assert_eq!(calls[0]["call_id"], "call_one");
    assert_eq!(calls[1]["call_id"], "call_two");
    assert_eq!(followup.responses["use_websocket"], true);
}
