use gateway_protocol::openai::chat::{DecodedChatRequest, decode_chat_request};
use serde_json::{Value, json};

use super::request;

fn decode_tools(
    tools: Value,
) -> Result<DecodedChatRequest, gateway_protocol::openai::chat::ChatConversionError> {
    let mut input = request();
    input["tools"] = tools;
    decode_chat_request(input)
}

fn history(call: Value, output: Value) -> Value {
    json!({"model":"gpt-test","messages":[
        {"role":"assistant","content":null,"tool_calls":[call]},
        {"role":"tool","tool_call_id":"call_custom","content":output}
    ]})
}

fn custom_call() -> Value {
    json!({"id":"call_custom","type":"custom","custom":{
        "name":"apply.patch","input":"*** Begin Patch\n*** End Patch\n"
    }})
}

fn two_custom_rounds() -> Value {
    json!({"model":"gpt-test","messages":[
        {"role":"assistant","content":null,"tool_calls":[
            {"id":"call_custom_1","type":"custom","custom":{
                "name":"apply.patch","input":"first patch\n  exact input\n"
            }}
        ]},
        {"role":"tool","tool_call_id":"call_custom_1","name":"apply.patch","content":"first result\n"},
        {"role":"assistant","content":null,"tool_calls":[
            {"id":"call_custom_2","type":"custom","custom":{
                "name":"apply.patch","input":"second patch\n  different input\n"
            }}
        ]},
        {"role":"tool","tool_call_id":"call_custom_2","name":"apply.patch","content":"second result\n"}
    ]})
}

#[test]
fn explicit_image_generation_preserves_native_fields_and_forced_choice() {
    let tool = json!({
        "type":"image_generation",
        "action":"edit",
        "model":"gpt-image-2",
        "background":"transparent",
        "quality":"high",
        "size":"1536x864",
        "output_format":"webp",
        "output_compression":70,
        "partial_images":3,
        "moderation":"auto",
        "input_fidelity":"high",
        "input_image_mask":{"image_url":"data:image/png;base64,AAEC"}
    });
    let mut input = request();
    input["tools"] = json!([tool]);
    input["tool_choice"] = json!({"type":"image_generation"});
    let decoded = decode_chat_request(input).unwrap();
    assert_eq!(decoded.responses["tools"], json!([tool]));
    assert_eq!(
        decoded.responses["tool_choice"],
        json!({"type":"image_generation"})
    );
    assert!(!decoded.stream);
    assert_eq!(decoded.responses["model"], "gpt-test");
    assert!(
        !decode_chat_request(request())
            .unwrap()
            .responses
            .contains_key("tools")
    );
    for mask in [
        json!({"file_id":"file_mask_exact"}),
        json!({"image_url":"data:image/png;base64,AAEC"}),
    ] {
        let tool = json!({"type":"image_generation","input_image_mask":mask});
        assert_eq!(
            decode_tools(json!([tool])).unwrap().responses["tools"],
            json!([tool])
        );
    }
}

#[test]
fn image_generation_validates_enum_numeric_and_mask_fields() {
    for (key, value) in [
        ("action", json!("erase")),
        ("background", json!("white")),
        ("quality", json!("hd")),
        ("output_format", json!("jpg")),
        ("moderation", json!("disabled")),
        ("input_fidelity", json!("auto")),
        ("model", json!(" ")),
        ("model", json!(7)),
        ("size", json!(1024)),
        ("size", json!("1024")),
        ("size", json!("0x1024")),
        ("size", json!("1024x1024x1024")),
        ("size", json!("+1024x1024")),
        ("output_compression", json!(-1)),
        ("output_compression", json!(101)),
        ("output_compression", json!(1.5)),
        ("output_compression", json!("70")),
        ("partial_images", json!(-1)),
        ("partial_images", json!(4)),
        ("partial_images", json!(true)),
        ("partial_images", json!(1.5)),
        ("input_image_mask", json!({})),
        ("input_image_mask", json!({"image_url":""})),
        ("input_image_mask", json!({"file_id":17})),
        (
            "input_image_mask",
            json!({"image_url":"mask","file_id":"file_mask"}),
        ),
        (
            "input_image_mask",
            json!({"image_url":"mask","unknown":"secret-body"}),
        ),
    ] {
        let mut tool = json!({"type":"image_generation"});
        tool[key] = value;
        assert!(decode_tools(json!([tool])).is_err(), "{key}: {tool}");
    }
    for key in [
        "action",
        "background",
        "quality",
        "output_format",
        "moderation",
        "model",
        "size",
        "output_compression",
        "partial_images",
        "input_image_mask",
    ] {
        let mut tool = json!({"type":"image_generation"});
        tool[key] = Value::Null;
        assert!(decode_tools(json!([tool])).is_err(), "{key}");
    }
    assert!(
        decode_tools(json!([{
            "type":"image_generation","background":"transparent","output_format":"jpeg"
        }]))
        .is_err()
    );
    for tool in [
        json!({"type":"image_generation"}),
        json!({"type":"image_generation","size":"auto","quality":"auto","input_fidelity":null}),
        json!({"type":"image_generation","size":"1024x1024","partial_images":0,"output_compression":0}),
        json!({"type":"image_generation","size":"1024x1536","partial_images":3,"output_compression":100}),
        json!({"type":"image_generation","size":"1536x1024","quality":"xhigh"}),
        json!({"type":"image_generation","size":"3840x2160","quality":"max"}),
        json!({"type":"image_generation","size":"1025x1024"}),
        json!({"type":"image_generation","size":"1024x64"}),
        json!({"type":"image_generation","size":"3840x3840"}),
        json!({"type":"image_generation","size":"99999999999999999999999999x1024"}),
    ] {
        assert_eq!(
            decode_tools(json!([tool])).unwrap().responses["tools"],
            json!([tool])
        );
    }
}

#[test]
fn image_generation_rejects_unrelated_images_endpoint_parameters() {
    for (key, value) in [
        ("n", json!(2)),
        ("prompt", json!("secret-body")),
        ("response_format", json!("b64_json")),
        ("images", json!([])),
        ("user", json!("secret-body")),
        ("seed", json!(1)),
        ("unknown_secret_field", json!("secret-body")),
    ] {
        let mut tool = json!({"type":"image_generation"});
        tool[key] = value;
        let error = decode_tools(json!([tool])).unwrap_err();
        assert_eq!(error.code(), "unsupported_parameter");
        assert_eq!(error.param(), Some("tools[0]"));
        assert!(!error.to_string().contains("secret"));
    }
}

#[test]
fn explicit_web_search_preserves_options_location_filters_and_choice() {
    let tool = json!({
        "type":"web_search","search_context_size":"high","external_web_access":false,
        "user_location":{"type":"approximate","city":"New York","country":"US",
            "region":"NY","timezone":"America/New_York"},
        "filters":{"allowed_domains":["example.test","docs.example.test"]}
    });
    let mut input = request();
    input["tools"] = json!([tool]);
    input["tool_choice"] = json!({"type":"web_search"});
    let decoded = decode_chat_request(input).unwrap();
    assert_eq!(decoded.responses["tools"], json!([tool]));
    assert_eq!(
        decoded.responses["tool_choice"],
        json!({"type":"web_search"})
    );
    for tool in [
        json!({"type":"web_search","filters":null,"user_location":null}),
        json!({"type":"web_search","filters":{"allowed_domains":null},
            "user_location":{"city":null,"country":null,"region":null,"timezone":null}}),
    ] {
        assert_eq!(
            decode_tools(json!([tool])).unwrap().responses["tools"],
            json!([tool])
        );
    }
}

#[test]
fn web_search_rejects_malformed_and_unknown_options() {
    for (key, value) in [
        ("search_context_size", json!("huge")),
        ("search_context_size", Value::Null),
        ("external_web_access", json!("false")),
        ("external_web_access", Value::Null),
        ("user_location", json!("US")),
        ("user_location", json!({"type":"precise"})),
        ("user_location", json!({"type":null})),
        ("user_location", json!({"country":123})),
        ("user_location", json!({"approximate":{"country":"US"}})),
        ("user_location", json!({"latitude":0})),
        ("filters", json!([])),
        ("filters", json!({"allowed_domains":"example.test"})),
        ("filters", json!({"allowed_domains":[true]})),
        ("filters", json!({"allowed_domains":[" "]})),
        ("filters", json!({"allowed_domains":[null]})),
        ("filters", json!({"denied_domains":["example.test"]})),
        ("sources", json!([])),
    ] {
        let mut tool = json!({"type":"web_search"});
        tool[key] = value;
        assert!(decode_tools(json!([tool])).is_err(), "{key}: {tool}");
    }
}

#[test]
fn legacy_search_options_keep_nested_location_and_reject_ambiguous_declarations() {
    let mut input = request();
    input["tools"] = json!([{"type":"image_generation"}]);
    input["web_search_options"] = json!({"search_context_size":"low","user_location":{
        "type":"approximate","approximate":{"country":"US","city":"New York"}
    }});
    input["tool_choice"] = json!({"type":"web_search"});
    let decoded = decode_chat_request(input.clone()).unwrap();
    assert_eq!(
        decoded.responses["tools"][1],
        json!({
            "type":"web_search","search_context_size":"low",
            "user_location":{"type":"approximate","country":"US","city":"New York"}
        })
    );
    input["tools"][0] = json!({"type":"web_search"});
    assert_eq!(
        decode_chat_request(input).unwrap_err().param(),
        Some("web_search_options")
    );
    for options in [
        json!({"search_context_size":true}),
        json!({"user_location":{"type":"approximate","approximate":{"city":2}}}),
        json!({"user_location":{"type":"approximate","country":"US"}}),
        json!({"filters":{"allowed_domains":["example.test"]}}),
    ] {
        let mut input = request();
        input["web_search_options"] = options;
        assert!(decode_chat_request(input).is_err());
    }
}

#[test]
fn custom_tools_convert_nested_grammar_without_renaming() {
    for syntax in ["regex", "lark"] {
        let mut input = request();
        input["tools"] = json!([{"type":"custom","custom":{
            "name":"apply.patch/exact","description":"Apply a patch",
            "format":{"type":"grammar","grammar":{"syntax":syntax,"definition":"start: \"ok\""}}
        }}]);
        input["tool_choice"] = json!({"type":"custom","custom":{"name":"apply.patch/exact"}});
        let decoded = decode_chat_request(input).unwrap();
        assert_eq!(
            decoded.responses["tools"][0],
            json!({
                "type":"custom","name":"apply.patch/exact","description":"Apply a patch",
                "format":{"type":"grammar","syntax":syntax,"definition":"start: \"ok\""}
            })
        );
        assert_eq!(
            decoded.responses["tool_choice"],
            json!({"type":"custom","name":"apply.patch/exact"})
        );
        assert!(decoded.responses["tools"][0].get("strict").is_none());
        assert!(decoded.responses["tools"][0].get("parameters").is_none());
    }
    for custom in [
        json!({"name":"freeform"}),
        json!({"name":"freeform","format":{"type":"text"}}),
    ] {
        let decoded = decode_tools(json!([{"type":"custom","custom":custom}])).unwrap();
        let mut expected = custom;
        expected["type"] = json!("custom");
        assert_eq!(decoded.responses["tools"][0], expected);
    }
}

#[test]
fn flat_custom_declarations_accept_text_and_both_grammar_forms_without_renaming() {
    let name = format!("namespace/apply.patch/{}", "exact".repeat(20));
    for format in [
        None,
        Some(json!({"type":"text"})),
        Some(json!({"type":"grammar","syntax":"regex","definition":"[a-z]+"})),
        Some(json!({"type":"grammar","syntax":"lark","definition":"start: \"ok\""})),
        Some(json!({"type":"grammar","grammar":{"syntax":"regex","definition":"[a-z]+"}})),
    ] {
        let mut tool = json!({"type":"custom","name":name,"description":"Apply exact input"});
        if let Some(format) = format {
            tool["format"] = format;
        }
        let mut expected = tool.clone();
        if tool["format"].get("grammar").is_some() {
            expected["format"] = json!({"type":"grammar","syntax":"regex","definition":"[a-z]+"});
        }
        let decoded = decode_tools(json!([tool])).unwrap();
        assert_eq!(decoded.responses["tools"], json!([expected]));
    }
}

#[test]
fn custom_tool_declarations_reject_missing_malformed_and_unknown_fields() {
    for tool in [
        json!({"type":"custom"}),
        json!({"type":"custom","custom":null}),
        json!({"type":"custom","custom":[]}),
        json!({"type":"custom","custom":{}}),
        json!({"type":"custom","custom":{"name":" "}}),
        json!({"type":"custom","custom":{"name":9}}),
        json!({"type":"custom","custom":{"name":"run","description":null}}),
        json!({"type":"custom","custom":{"name":"run","strict":false}}),
        json!({"type":"custom","custom":{"name":"run","parameters":{}}}),
        json!({"type":"custom","custom":{"name":"run","namespace":"hidden"}}),
        json!({"type":"custom","name":null}),
        json!({"type":"custom","name":" "}),
        json!({"type":"custom","name":9}),
        json!({"type":"custom","name":"run","description":null}),
        json!({"type":"custom","name":"run","strict":false}),
        json!({"type":"custom","name":"run","parameters":{}}),
        json!({"type":"custom","name":"run","namespace":"hidden"}),
        json!({"type":"custom","custom":{"name":"run"},"name":"run"}),
        json!({"type":"custom","custom":{"name":"run"},"name":"different"}),
        json!({"type":"custom","custom":{"name":"run"},"description":null}),
        json!({"type":"custom","custom":null,"name":"run"}),
    ] {
        assert!(decode_tools(json!([tool])).is_err(), "{tool}");
    }
    for format in [
        Value::Null,
        json!({}),
        json!({"type":"json_schema","schema":{}}),
        json!({"type":"text","grammar":{"definition":"secret-body"}}),
        json!({"type":"grammar"}),
        json!({"type":"grammar","syntax":"regex"}),
        json!({"type":"grammar","syntax":"regex","definition":""}),
        json!({"type":"grammar","syntax":"unknown","definition":".*"}),
        json!({"type":"grammar","syntax":"regex","definition":true}),
        json!({"type":"grammar","syntax":"regex","definition":".*","unknown":true}),
        json!({"type":"grammar","syntax":"regex","definition":".*","grammar":null}),
        json!({"type":"grammar","syntax":"regex","definition":".*",
            "grammar":{"syntax":"regex","definition":".*"}}),
        json!({"type":"grammar","grammar":{"syntax":"regex"}}),
        json!({"type":"grammar","grammar":{"syntax":"regex","definition":""}}),
        json!({"type":"grammar","grammar":{"syntax":"regex","definition":true}}),
        json!({"type":"grammar","grammar":{"syntax":"unknown","definition":".*"}}),
        json!({"type":"grammar","grammar":{"syntax":"regex","definition":".*","unknown":true}}),
    ] {
        for tool in [
            json!({"type":"custom","custom":{"name":"run","format":format}}),
            json!({"type":"custom","name":"run","format":format}),
        ] {
            assert!(decode_tools(json!([tool])).is_err(), "{tool}");
        }
    }
}

#[test]
fn custom_and_builtin_declarations_accept_only_empty_newapi_function_envelopes() {
    let declarations = [
        json!({"type":"custom","custom":{"name":"run"}}),
        json!({"type":"custom","name":"run"}),
        json!({"type":"image_generation"}),
        json!({"type":"web_search"}),
    ];
    for declaration in declarations {
        for placeholder in [json!({}), json!({"name":""})] {
            let mut tool = declaration.clone();
            tool["function"] = placeholder;
            let decoded = decode_tools(json!([tool])).unwrap();
            assert!(decoded.responses["tools"][0].get("function").is_none());
        }
        for conflict in [
            Value::Null,
            json!([]),
            json!({"name":null}),
            json!({"name":" "}),
            json!({"name":"actual_function"}),
            json!({"name":"","description":"must not lose"}),
            json!({"name":"","parameters":{}}),
            json!({"name":"","strict":false}),
            json!({"name":"","arguments":""}),
        ] {
            let mut tool = declaration.clone();
            tool["function"] = conflict;
            let error = decode_tools(json!([tool])).unwrap_err();
            assert!(error.param().unwrap().starts_with("tools[0].function"));
        }
    }
}

#[test]
fn custom_and_builtin_choices_preserve_identity_with_empty_function_envelopes() {
    for (tool, choice, expected) in [
        (
            json!({"type":"custom","custom":{"name":"run.exact"}}),
            json!({"type":"custom","custom":{"name":"run.exact"}}),
            json!({"type":"custom","name":"run.exact"}),
        ),
        (
            json!({"type":"custom","name":"run.exact"}),
            json!({"type":"custom","name":"run.exact"}),
            json!({"type":"custom","name":"run.exact"}),
        ),
        (
            json!({"type":"image_generation"}),
            json!({"type":"image_generation"}),
            json!({"type":"image_generation"}),
        ),
        (
            json!({"type":"web_search"}),
            json!({"type":"web_search"}),
            json!({"type":"web_search"}),
        ),
    ] {
        for placeholder in [json!({}), json!({"name":""})] {
            let mut input = request();
            input["tools"] = json!([tool]);
            input["tool_choice"] = choice.clone();
            input["tool_choice"]["function"] = placeholder;
            assert_eq!(
                decode_chat_request(input).unwrap().responses["tool_choice"],
                expected
            );
        }
        for option in ["auto", "none", "required"] {
            let mut input = request();
            input["tools"] = json!([tool]);
            input["tool_choice"] = json!(option);
            assert_eq!(
                decode_chat_request(input).unwrap().responses["tool_choice"],
                option
            );
        }
    }
}

#[test]
fn tool_declarations_and_choices_reject_duplicate_or_conflicting_identities() {
    for tools in [
        json!([{"type":"web_search"},{"type":"web_search"}]),
        json!([{"type":"image_generation"},{"type":"image_generation"}]),
        json!([{"type":"custom","custom":{"name":"run"}},{"type":"custom","custom":{"name":"run"}}]),
        json!([{"type":"function","function":{"name":"run"}},{"type":"custom","custom":{"name":"run"}}]),
        json!([{"type":"function","function":{"name":"run"}},{"type":"function","function":{"name":"run"}}]),
        json!([{"type":"custom","name":"run"},{"type":"custom","name":"run"}]),
        json!([{"type":"custom","name":"run"},{"type":"custom","custom":{"name":"run"}}]),
        json!([{"type":"custom","name":"run"},{"type":"function","function":{"name":"run"}}]),
        json!([{"type":"function","function":{"name":"run"}},{"type":"custom","name":"run"}]),
    ] {
        assert!(decode_tools(tools).is_err());
    }
    for choice in [
        json!({"type":"custom","custom":{"name":"missing"}}),
        json!({"type":"custom","custom":{"name":"function_name"}}),
        json!({"type":"function","function":{"name":"missing"}}),
        json!({"type":"custom","name":"function_name"}),
        json!({"type":"custom","name":"missing"}),
        json!({"type":"custom","name":null}),
        json!({"type":"custom","custom":null,"name":"custom_name"}),
        json!({"type":"custom","custom":{"name":"custom_name"},"name":"custom_name"}),
        json!({"type":"custom","custom":{"name":"custom_name"},"name":"different"}),
        json!({"type":"custom","name":"custom_name","input":"secret-body"}),
        json!({"type":"function","name":"custom_name","function":{"name":"custom_name"}}),
        json!({"type":"custom","custom":{"name":"custom_name","input":"secret-body"}}),
        json!({"type":"custom","custom":{"name":"custom_name"},"function":{"name":"function_name"}}),
        json!({"type":"image_generation","function":{"name":"function_name"}}),
        json!({"type":"web_search","filters":{}}),
        json!({"type":"namespace","name":"custom_name"}),
    ] {
        let mut input = request();
        input["tools"] = json!([
            {"type":"custom","custom":{"name":"custom_name"}},
            {"type":"function","function":{"name":"function_name"}},
            {"type":"image_generation"},{"type":"web_search"}
        ]);
        input["tool_choice"] = choice;
        assert!(decode_chat_request(input).is_err());
    }
    for kind in ["custom", "image_generation", "web_search"] {
        let mut input = request();
        input["tool_choice"] = if kind == "custom" {
            json!({"type":kind,"custom":{"name":"run"}})
        } else {
            json!({"type":kind})
        };
        assert!(decode_chat_request(input).is_err());
    }
}

#[test]
fn forced_function_envelopes_and_custom_choices_restore_declared_custom_identity() {
    let name = format!("apply.patch/{}", "exact".repeat(20));
    for tool in [
        json!({"type":"custom","name":name}),
        json!({"type":"custom","custom":{"name":name}}),
    ] {
        for choice in [
            json!({"type":"function","function":{"name":name}}),
            json!({"type":"custom","name":name}),
            json!({"type":"custom","custom":{"name":name}}),
        ] {
            let mut input = request();
            input["tools"] = json!([tool,{"type":"function","function":{"name":"lookup"}}]);
            input["tool_choice"] = choice;
            assert_eq!(
                decode_chat_request(input).unwrap().responses["tool_choice"],
                json!({"type":"custom","name":name})
            );
        }
        let mut input = request();
        input["tools"] = json!([tool,{"type":"function","function":{"name":"lookup"}}]);
        input["tool_choice"] = json!({"type":"function","function":{"name":"lookup"}});
        assert_eq!(
            decode_chat_request(input).unwrap().responses["tool_choice"],
            json!({"type":"function","name":"lookup"})
        );
    }
}

#[test]
fn declarations_are_validated_before_ambiguous_history_is_converted() {
    for tools in [
        json!([{"type":"custom","name":"run"},{"type":"function","function":{"name":"run"}}]),
        json!([{"type":"custom","custom":{"name":"run"}},"malformed"]),
    ] {
        let mut input = history(
            json!({"id":"call_custom","type":"function","function":{"name":"run","arguments":null}}),
            json!("result"),
        );
        input["tools"] = tools;
        let error = decode_chat_request(input).unwrap_err();
        assert!(error.param().unwrap().starts_with("tools[1]"));
    }
}

#[test]
fn function_envelopes_restore_mixed_parallel_calls_and_multiple_rounds_by_id() {
    let mut input = json!({"model":"gpt-test","messages":[
        {"role":"assistant","content":"working","tool_calls":[
            {"id":"call.function/1","type":"function","function":{
                "name":"lookup.exact","arguments":" { \"id\": 1 } "}},
            {"id":"call.custom/1","type":"function","function":{
                "name":"apply.patch","arguments":"first patch\n  exact input\n"}},
            {"id":"call.custom/2","type":"function","function":{
                "name":"apply.patch","arguments":"second patch\n"}}
        ]},
        {"role":"tool","tool_call_id":"call.custom/2","name":"apply.patch","content":"second result"},
        {"role":"tool","tool_call_id":"call.function/1","name":"lookup.exact","content":"found"},
        {"role":"tool","tool_call_id":"call.custom/1","content":"first result"},
        {"role":"assistant","content":null,"tool_calls":[
            {"id":"call.custom/3","type":"function","function":{
                "name":"apply.patch","arguments":"third patch\n"}},
            {"id":"call.function/2","type":"function","function":{
                "name":"lookup.exact","arguments":"[1, 2]"}}
        ]},
        {"role":"tool","tool_call_id":"call.custom/3","content":"third result"},
        {"role":"tool","tool_call_id":"call.function/2","content":"found again"},
        {"role":"assistant","content":"done"}
    ]});
    for tool in [
        json!({"type":"custom","name":"apply.patch"}),
        json!({"type":"custom","custom":{"name":"apply.patch"}}),
    ] {
        input["tools"] = json!([tool,{"type":"function","function":{"name":"lookup.exact"}}]);
        assert_eq!(
            decode_chat_request(input.clone()).unwrap().responses["input"],
            json!([
                {"type":"message","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":"working"}]},
                {"type":"function_call","call_id":"call.function/1","name":"lookup.exact",
                    "arguments":" { \"id\": 1 } "},
                {"type":"custom_tool_call","call_id":"call.custom/1","name":"apply.patch",
                    "input":"first patch\n  exact input\n"},
                {"type":"custom_tool_call","call_id":"call.custom/2","name":"apply.patch",
                    "input":"second patch\n"},
                {"type":"custom_tool_call_output","call_id":"call.custom/2","output":"second result"},
                {"type":"function_call_output","call_id":"call.function/1","output":"found"},
                {"type":"custom_tool_call_output","call_id":"call.custom/1","output":"first result"},
                {"type":"custom_tool_call","call_id":"call.custom/3","name":"apply.patch",
                    "input":"third patch\n"},
                {"type":"function_call","call_id":"call.function/2","name":"lookup.exact","arguments":"[1, 2]"},
                {"type":"custom_tool_call_output","call_id":"call.custom/3","output":"third result"},
                {"type":"function_call_output","call_id":"call.function/2","output":"found again"},
                {"type":"message","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":"done"}]}
            ])
        );
    }
}

#[test]
fn restored_custom_envelopes_preserve_arbitrary_strings_and_long_names_and_ids() {
    let name = format!("apply.patch/{}", "exact".repeat(20));
    let id = format!("call.custom/{}", "exact".repeat(20));
    for payload in [
        "",
        " \n\t ",
        "*** Begin Patch\r\n  arbitrary input\r\n*** End Patch\n",
        "print(\"hello\")\n\u{0000}\u{4e2d}\u{1f642}",
        " { \"key\": \"value\" } ",
        "[{\"type\":\"image_url\",\"image_url\":{\"url\":\"data:image/png;base64,AAEC\"}}]",
    ] {
        for kind in ["function", "custom"] {
            let call = if kind == "function" {
                json!({"id":id,"type":"function","function":{"name":name,"arguments":payload}})
            } else {
                json!({"id":id,"type":"custom","custom":{"name":name,"input":payload}})
            };
            let mut input = history(call, json!(payload));
            input["tools"] = json!([{"type":"custom","name":name}]);
            input["messages"][1]["tool_call_id"] = json!(id);
            assert_eq!(
                decode_chat_request(input).unwrap().responses["input"],
                json!([
                    {"type":"custom_tool_call","call_id":id,"name":name,"input":payload},
                    {"type":"custom_tool_call_output","call_id":id,"output":payload}
                ])
            );
        }
    }
}

#[test]
fn native_and_function_custom_envelopes_can_share_a_declaration_in_one_batch() {
    let input = json!({
        "model":"gpt-test","tools":[{"type":"custom","name":"apply.patch"}],"messages":[
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"native","type":"custom","custom":{"name":"apply.patch","input":"first"}},
                {"id":"envelope","type":"function","function":{"name":"apply.patch","arguments":"second"}}
            ]},
            {"role":"tool","tool_call_id":"envelope","content":"second result"},
            {"role":"tool","tool_call_id":"native","content":"first result"}
        ]
    });
    assert_eq!(
        decode_chat_request(input).unwrap().responses["input"],
        json!([
            {"type":"custom_tool_call","call_id":"native","name":"apply.patch","input":"first"},
            {"type":"custom_tool_call","call_id":"envelope","name":"apply.patch","input":"second"},
            {"type":"custom_tool_call_output","call_id":"envelope","output":"second result"},
            {"type":"custom_tool_call_output","call_id":"native","output":"first result"}
        ])
    );
}

#[test]
fn function_history_never_guesses_custom_types_without_a_matching_declaration() {
    for tools in [
        None,
        Some(Value::Null),
        Some(json!([])),
        Some(json!([{"type":"function","function":{"name":"apply.patch"}}])),
        Some(json!([{"type":"custom","name":"other.patch"}])),
        Some(json!([{"type":"custom","name":"Apply.Patch"}])),
        Some(json!([{"type":"custom","name":"apply.patch "}])),
    ] {
        for payload in [
            "print(1)\n",
            "*** Begin Patch\n*** End Patch\n",
            "{\"type\":\"custom\",\"name\":\"apply.patch\",\"input\":\"raw\"}",
        ] {
            let mut input = history(
                json!({"id":"call_custom","type":"function","function":{
                    "name":"apply.patch","arguments":payload}}),
                json!(payload),
            );
            if let Some(tools) = &tools {
                input["tools"] = tools.clone();
            }
            assert_eq!(
                decode_chat_request(input).unwrap().responses["input"],
                json!([
                    {"type":"function_call","call_id":"call_custom","name":"apply.patch","arguments":payload},
                    {"type":"function_call_output","call_id":"call_custom","output":payload}
                ])
            );
        }
    }
}

#[test]
fn restored_custom_envelopes_reject_ambiguous_ids_results_and_nonstring_arguments() {
    let mut input = history(
        json!({"id":"call_custom","type":"function","function":{
            "name":"apply.patch","arguments":"raw"}}),
        json!("ok"),
    );
    input["tools"] = json!([{"type":"custom","name":"apply.patch"}]);
    for arguments in [Value::Null, json!({}), json!([]), json!(1)] {
        let mut malformed = input.clone();
        malformed["messages"][0]["tool_calls"][0]["function"]["arguments"] = arguments;
        assert_eq!(
            decode_chat_request(malformed).unwrap_err().param(),
            Some("messages[0].tool_calls[0].function.arguments")
        );
    }
    for duplicate in [
        input["messages"][0]["tool_calls"][0].clone(),
        custom_call(),
        json!({"id":"call_custom","type":"function","function":{"name":"lookup","arguments":"{}"}}),
    ] {
        let mut ambiguous = input.clone();
        ambiguous["messages"][0]["tool_calls"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert_eq!(
            decode_chat_request(ambiguous).unwrap_err().param(),
            Some("messages[0].tool_calls[1].id")
        );
    }
    for (key, value) in [("tool_call_id", "unknown"), ("name", "other.patch")] {
        let mut mismatched = input.clone();
        mismatched["messages"][1][key] = json!(value);
        let error = decode_chat_request(mismatched).unwrap_err();
        assert_eq!(error.param(), Some(format!("messages[1].{key}").as_str()));
    }
    let result = input["messages"][1].clone();
    input["messages"].as_array_mut().unwrap().push(result);
    assert_eq!(
        decode_chat_request(input).unwrap_err().param(),
        Some("messages[2].tool_call_id")
    );
}

#[test]
fn restored_custom_envelopes_keep_cross_round_duplicate_and_replay_guards() {
    let mut input = two_custom_rounds();
    input["tools"] = json!([{"type":"custom","name":"apply.patch"}]);
    for index in [0, 2] {
        let call = &input["messages"][index]["tool_calls"][0];
        input["messages"][index]["tool_calls"][0] = json!({
            "id":call["id"],"type":"function","function":{
                "name":call["custom"]["name"],"arguments":call["custom"]["input"]
            }
        });
    }
    for index in [0, 2] {
        let mut duplicate = input.clone();
        let message = input["messages"][index].clone();
        duplicate["messages"].as_array_mut().unwrap().push(message);
        assert_eq!(
            decode_chat_request(duplicate).unwrap_err().param(),
            Some("messages[4].tool_calls[0].id")
        );
    }
    for index in [1, 3] {
        let mut replay = input.clone();
        let message = input["messages"][index].clone();
        replay["messages"].as_array_mut().unwrap().push(message);
        assert_eq!(
            decode_chat_request(replay).unwrap_err().param(),
            Some("messages[4].tool_call_id")
        );
    }
}

#[test]
fn custom_history_preserves_raw_input_call_identity_and_result_order() {
    let input = json!({"model":"gpt-test","messages":[
        {"role":"assistant","content":"working","tool_calls":[
            {"id":"call.function/exact","type":"function","function":{"name":"lookup.exact","arguments":" { \"id\": 1 } "}},
            {"id":"call.custom/exact","type":"custom","custom":{"name":"apply.patch/exact","input":"not JSON\n  exact text\n"}}
        ]},
        {"role":"tool","tool_call_id":"call.custom/exact","name":"apply.patch/exact","content":"patched\n"},
        {"role":"tool","tool_call_id":"call.function/exact","name":"lookup.exact","content":"found"},
        {"role":"assistant","content":"done"}
    ]});
    let decoded = decode_chat_request(input).unwrap();
    let items = decoded.responses["input"].as_array().unwrap();
    assert_eq!(items.len(), 6);
    assert_eq!(items[0]["content"][0]["text"], "working");
    assert_eq!(
        items[1],
        json!({"type":"function_call","call_id":"call.function/exact",
        "name":"lookup.exact","arguments":" { \"id\": 1 } "})
    );
    assert_eq!(
        items[2],
        json!({"type":"custom_tool_call","call_id":"call.custom/exact",
        "name":"apply.patch/exact","input":"not JSON\n  exact text\n"})
    );
    assert_eq!(
        items[3],
        json!({"type":"custom_tool_call_output","call_id":"call.custom/exact","output":"patched\n"})
    );
    assert_eq!(
        items[4],
        json!({"type":"function_call_output","call_id":"call.function/exact","output":"found"})
    );
    assert_eq!(items[5]["content"][0]["text"], "done");
}

#[test]
fn custom_history_preserves_two_consecutive_rounds_with_the_same_tool_name() {
    let decoded = decode_chat_request(two_custom_rounds()).unwrap();
    assert_eq!(
        decoded.responses["input"],
        json!([
            {"type":"custom_tool_call","call_id":"call_custom_1","name":"apply.patch",
                "input":"first patch\n  exact input\n"},
            {"type":"custom_tool_call_output","call_id":"call_custom_1","output":"first result\n"},
            {"type":"custom_tool_call","call_id":"call_custom_2","name":"apply.patch",
                "input":"second patch\n  different input\n"},
            {"type":"custom_tool_call_output","call_id":"call_custom_2","output":"second result\n"}
        ])
    );
}

#[test]
fn custom_history_matches_same_name_parallel_calls_by_id_with_reversed_results() {
    let input = json!({"model":"gpt-test","messages":[
        {"role":"assistant","content":null,"tool_calls":[
            {"id":"call_custom_1","type":"custom","custom":{"name":"apply.patch","input":"first patch"}},
            {"id":"call_custom_2","type":"custom","custom":{"name":"apply.patch","input":"second patch"}}
        ]},
        {"role":"tool","tool_call_id":"call_custom_2","name":"apply.patch","content":"second result"},
        {"role":"tool","tool_call_id":"call_custom_1","name":"apply.patch","content":"first result"}
    ]});
    let decoded = decode_chat_request(input).unwrap();
    assert_eq!(
        decoded.responses["input"],
        json!([
            {"type":"custom_tool_call","call_id":"call_custom_1","name":"apply.patch","input":"first patch"},
            {"type":"custom_tool_call","call_id":"call_custom_2","name":"apply.patch","input":"second patch"},
            {"type":"custom_tool_call_output","call_id":"call_custom_2","output":"second result"},
            {"type":"custom_tool_call_output","call_id":"call_custom_1","output":"first result"}
        ])
    );
}

#[test]
fn custom_history_rejects_call_ids_reused_from_either_previous_round() {
    for id in ["call_custom_1", "call_custom_2"] {
        for name in ["apply.patch", "different.patch"] {
            let mut input = two_custom_rounds();
            input["messages"].as_array_mut().unwrap().push(json!({
                "role":"assistant","content":null,"tool_calls":[
                    {"id":id,"type":"custom","custom":{"name":name,"input":"third patch"}}
                ]
            }));
            let error = decode_chat_request(input).unwrap_err();
            assert_eq!(error.code(), "invalid_request_error");
            assert_eq!(error.param(), Some("messages[4].tool_calls[0].id"));
        }
    }
}

#[test]
fn custom_history_rejects_prior_round_results_replayed_during_a_later_round() {
    for result_index in [1, 3] {
        for replace_content in [false, true] {
            let mut input = two_custom_rounds();
            let mut repeated_result = input["messages"][result_index].clone();
            if replace_content {
                repeated_result["content"] = json!("changed result");
            }
            input["messages"].as_array_mut().unwrap().extend([
                json!({
                    "role":"assistant","content":null,"tool_calls":[
                        {"id":"call_custom_3","type":"custom","custom":{
                            "name":"apply.patch","input":"third patch"
                        }}
                    ]
                }),
                repeated_result,
            ]);
            let error = decode_chat_request(input).unwrap_err();
            assert_eq!(error.code(), "invalid_request_error");
            assert_eq!(error.param(), Some("messages[5].tool_call_id"));
        }
    }
}

#[test]
fn custom_history_preserves_empty_strings_and_typed_results() {
    let mut call = custom_call();
    call["custom"]["input"] = json!("");
    let decoded = decode_chat_request(history(call, json!(""))).unwrap();
    assert_eq!(decoded.responses["input"][0]["input"], "");
    assert_eq!(decoded.responses["input"][1]["output"], "");
    let decoded = decode_chat_request(history(
        custom_call(),
        json!([
            {"type":"text","text":"result"},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,AAEC","detail":"low"}}
        ]),
    ))
    .unwrap();
    assert_eq!(
        decoded.responses["input"][1],
        json!({
            "type":"custom_tool_call_output","call_id":"call_custom","output":[
                {"type":"input_text","text":"result"},
                {"type":"input_image","image_url":"data:image/png;base64,AAEC","detail":"low"}
            ]
        })
    );
}

#[test]
fn custom_history_accepts_empty_newapi_function_but_not_conflicting_payloads() {
    for placeholder in [json!({}), json!({"name":""})] {
        let mut call = custom_call();
        call["function"] = placeholder;
        let decoded = decode_chat_request(history(call, json!("ok"))).unwrap();
        assert_eq!(
            decoded.responses["input"][0],
            json!({
                "type":"custom_tool_call","call_id":"call_custom","name":"apply.patch",
                "input":"*** Begin Patch\n*** End Patch\n"
            })
        );
    }
    for conflict in [
        Value::Null,
        json!({"name":"run","arguments":"{}"}),
        json!({"name":"","arguments":"{}"}),
        json!({"name":"","arguments":""}),
        json!({"name":" "}),
        json!({"name":null}),
    ] {
        let mut call = custom_call();
        call["function"] = conflict;
        let error = decode_chat_request(history(call, json!("ok"))).unwrap_err();
        assert!(
            error
                .param()
                .unwrap()
                .starts_with("messages[0].tool_calls[0].function")
        );
    }
}

#[test]
fn custom_history_rejects_malformed_calls_or_outputs_and_unknown_fields() {
    for call in [
        json!({"id":"call_custom","type":"custom"}),
        json!({"id":"call_custom","type":"custom","custom":{}}),
        json!({"id":"call_custom","type":"custom","custom":{"name":"run"}}),
        json!({"id":"call_custom","type":"custom","custom":{"name":"run","input":{}}}),
        json!({"id":"call_custom","type":"custom","custom":{"name":"run","input":null}}),
        json!({"id":"call_custom","type":"custom","custom":{"name":"run","input":"","arguments":"{}"}}),
        json!({"id":"call_custom","type":"custom","custom":{"name":"run","input":"","namespace":"secret"}}),
        json!({"id":"call_custom","type":"custom","name":"run","input":"raw"}),
        json!({"id":"","type":"custom","custom":{"name":"run","input":"raw"}}),
        json!({"type":"custom","custom":{"name":"run","input":"raw"}}),
        json!({"id":"call_custom","type":"custom","custom":{"name":" ","input":"raw"}}),
        json!({"id":"call_custom","type":"function","function":{"name":"run","arguments":"{}"},"custom":{"name":"run","input":"raw"}}),
    ] {
        assert!(decode_chat_request(history(call, json!("ok"))).is_err());
    }
    for output in [
        Value::Null,
        json!(12),
        json!({"text":"ok"}),
        json!([]),
        json!([{"type":"unknown","text":"secret-body"}]),
    ] {
        let error = decode_chat_request(history(custom_call(), output)).unwrap_err();
        assert!(!error.to_string().contains("secret-body"));
    }
    for role in ["user", "system", "developer", "tool"] {
        let mut input = history(custom_call(), json!("ok"));
        input["messages"][0]["role"] = json!(role);
        assert!(decode_chat_request(input).is_err());
    }
}

#[test]
fn custom_history_rejects_duplicate_ids_results_and_wrong_names() {
    for duplicate in [
        custom_call(),
        json!({"id":"call_custom","type":"function","function":{"name":"run","arguments":"{}"}}),
    ] {
        let mut input = history(custom_call(), json!("ok"));
        input["messages"][0]["tool_calls"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert_eq!(
            decode_chat_request(input).unwrap_err().param(),
            Some("messages[0].tool_calls[1].id")
        );
    }
    let mut input = history(custom_call(), json!("ok"));
    input["messages"][1]["name"] = json!("different");
    assert_eq!(
        decode_chat_request(input).unwrap_err().param(),
        Some("messages[1].name")
    );
    let mut input = history(custom_call(), json!("ok"));
    input["messages"][1]["tool_call_id"] = json!("missing");
    assert_eq!(
        decode_chat_request(input).unwrap_err().param(),
        Some("messages[1].tool_call_id")
    );
    let mut input = history(custom_call(), json!("ok"));
    let result = input["messages"][1].clone();
    input["messages"].as_array_mut().unwrap().push(result);
    assert_eq!(
        decode_chat_request(input).unwrap_err().param(),
        Some("messages[2].tool_call_id")
    );
    let mut input = history(custom_call(), json!("ok"));
    input["messages"].as_array_mut().unwrap().swap(0, 1);
    assert_eq!(
        decode_chat_request(input).unwrap_err().param(),
        Some("messages[0].tool_call_id")
    );
}

#[test]
fn assistant_image_only_history_is_skipped_without_reconstructing_upstream_input() {
    let images = json!([
        {"type":"image_url","index":0,"image_url":{"url":"data:image/png;base64,AAEC"}},
        {"type":"image_url","index":1,"image_url":{"url":"https://example.test/output-only.png"}}
    ]);
    for content in [None, Some(Value::Null), Some(json!("")), Some(json!([]))] {
        let mut assistant = json!({"role":"assistant","images":images});
        if let Some(content) = content {
            assistant["content"] = content;
        }
        let input = json!({"model":"gpt-test","messages":[
            {"role":"user","content":"draw"},assistant,
            {"role":"user","content":"continue"}
        ]});
        let decoded = decode_chat_request(input).unwrap();
        assert_eq!(
            decoded.responses["input"],
            json!([
                {"type":"message","role":"user","content":[{"type":"input_text","text":"draw"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}
            ])
        );
        assert!(!decoded.responses.contains_key("tools"));
        assert_eq!(
            decode_chat_request(json!({"model":"gpt-test","messages":[assistant]}))
                .unwrap()
                .responses["input"],
            json!([])
        );
    }
}

#[test]
fn assistant_output_images_preserve_associated_text_calls_and_refusals() {
    let images = json!([{
        "type":"image_url","index":0,"image_url":{"url":"https://example.test/output-only-secret.png"}
    }]);
    let input = json!({
        "model":"gpt-test","tools":[{"type":"custom","name":"run"}],
        "response_format":{"type":"json_object"},
        "web_search_options":{"search_context_size":"low"},
        "messages":[
            {"role":"user","content":[
                {"type":"image_url","image_url":{
                    "url":"https://example.test/user.png","detail":"original","MimeType":""}}
            ]},
            {"role":"assistant","content":"{\"ok\":true}","images":images,"tool_calls":[
                {"id":"custom/image","type":"function","function":{"name":"run","arguments":"print(1)"}}
            ]},
            {"role":"tool","tool_call_id":"custom/image","content":[
                {"type":"text","text":"result"},
                {"type":"image_url","image_url":{"url":"https://example.test/tool.png","detail":"high"}}
            ]},
            {"role":"assistant","content":null,"refusal":"Cannot edit further","images":images}
        ]
    });
    let decoded = decode_chat_request(input).unwrap();
    assert_eq!(
        decoded.responses["input"],
        json!([
            {"type":"message","role":"user","content":[
                {"type":"input_image","image_url":"https://example.test/user.png","detail":"original"}
            ]},
            {"type":"message","role":"assistant","status":"completed",
                "content":[{"type":"output_text","text":"{\"ok\":true}"}]},
            {"type":"custom_tool_call","call_id":"custom/image","name":"run","input":"print(1)"},
            {"type":"custom_tool_call_output","call_id":"custom/image","output":[
                {"type":"input_text","text":"result"},
                {"type":"input_image","image_url":"https://example.test/tool.png","detail":"high"}
            ]},
            {"type":"message","role":"assistant","status":"completed",
                "content":[{"type":"refusal","refusal":"Cannot edit further"}]}
        ])
    );
    assert_eq!(
        decoded.responses["text"]["format"],
        json!({"type":"json_object"})
    );
    assert_eq!(
        decoded.responses["tools"],
        json!([
            {"type":"custom","name":"run"},
            {"type":"web_search","search_context_size":"low"}
        ])
    );
    assert!(
        !Value::Object(decoded.responses)
            .to_string()
            .contains("output-only-secret")
    );

    for content in [Value::Null, json!(""), json!([])] {
        let mut input = history(custom_call(), json!("ok"));
        input["messages"][0]["content"] = content;
        input["messages"][0]["images"] = images.clone();
        let decoded = decode_chat_request(input).unwrap();
        assert_eq!(decoded.responses["input"].as_array().unwrap().len(), 2);
        assert_eq!(decoded.responses["input"][0]["type"], "custom_tool_call");
        assert_eq!(
            decoded.responses["input"][1]["type"],
            "custom_tool_call_output"
        );
    }
    let decoded = decode_chat_request(json!({"model":"gpt-test","messages":[{
        "role":"assistant","images":images,"content":[
            {"type":"text","text":"before"},
            {"type":"refusal","refusal":"refused"},
            {"type":"text","text":"after"}
        ]
    }]}))
    .unwrap();
    assert_eq!(
        decoded.responses["input"][0]["content"],
        json!([
            {"type":"output_text","text":"before"},
            {"type":"refusal","refusal":"refused"},
            {"type":"output_text","text":"after"}
        ])
    );
}

#[test]
fn assistant_image_history_accepts_unindexed_images_and_empty_optional_extensions() {
    for images in [
        Value::Null,
        json!([]),
        json!([
            {"type":"image_url","image_url":{"url":"https://example.test/one.png"}},
            {"type":"image_url","image_url":{"url":"https://example.test/two.png"}}
        ]),
        json!([
            {"type":"image_url","index":7,"image_url":{"url":"https://example.test/one.png"}},
            {"type":"image_url","index":2,"image_url":{"url":"https://example.test/two.png"}}
        ]),
    ] {
        let decoded = decode_chat_request(json!({"model":"gpt-test","messages":[
            {"role":"assistant","content":"exact text","images":images}
        ]}))
        .unwrap();
        assert_eq!(
            decoded.responses["input"],
            json!([{
                "type":"message","role":"assistant","status":"completed",
                "content":[{"type":"output_text","text":"exact text"}]
            }])
        );
    }
    for images in [Value::Null, json!([])] {
        let error = decode_chat_request(json!({"model":"gpt-test","messages":[
            {"role":"assistant","content":null,"images":images}
        ]}))
        .unwrap_err();
        assert_eq!(error.param(), Some("messages[0].content"));
    }
}

#[test]
fn assistant_images_reject_malformed_and_ambiguous_history_even_alongside_text() {
    let valid =
        json!({"type":"image_url","index":0,"image_url":{"url":"https://example.test/image.png"}});
    let mut malformed = vec![
        json!("secret-body"),
        json!({}),
        json!([null]),
        json!([{}]),
        json!([{"type":"image_url","image_url":"https://example.test/image.png"}]),
        json!([{"type":"image_url","image_url":{}}]),
        json!([{"type":"image_url","image_url":{"url":null}}]),
        json!([{"type":"image_url","image_url":{"url":7}}]),
        json!([{"type":"image_url","image_url":{"url":" "}}]),
        json!([{"type":"image_url","image_url":{"url":"url","unknown":"secret-body"}}]),
        json!([{"type":"image_url","image_url":{"url":"url"},"unknown":"secret-body"}]),
        json!([{"type":"input_image","image_url":"url"}]),
        json!([valid, valid]),
        json!([valid,{"type":"image_url","image_url":{"url":"url"}}]),
        json!([{"type":"image_url","image_url":{"url":"url"}},valid]),
    ];
    for index in [Value::Null, json!(-1), json!(0.5), json!("0"), json!(true)] {
        let mut image = valid.clone();
        image["index"] = index;
        malformed.push(json!([image]));
    }
    for images in malformed {
        let error = decode_chat_request(json!({"model":"gpt-test","messages":[{
            "role":"assistant","content":"keep this text","images":images
        }]}))
        .unwrap_err();
        assert!(error.param().unwrap().starts_with("messages[0].images"));
        assert!(!error.to_string().contains("secret-body"));
    }
    for content in [
        json!(17),
        json!({}),
        json!([{"type":"image_url","image_url":{"url":"https://example.test/input.png"}}]),
        json!([{"type":"text","text":null}]),
    ] {
        let error = decode_chat_request(json!({"model":"gpt-test","messages":[{
            "role":"assistant","content":content,"images":[valid]
        }]}))
        .unwrap_err();
        assert!(error.param().unwrap().starts_with("messages[0].content"));
    }
    for role in ["user", "tool", "system", "developer"] {
        for images in [Value::Null, json!([]), json!([valid])] {
            let error = decode_chat_request(json!({"model":"gpt-test","messages":[{
                "role":role,"content":"text","images":images
            }]}))
            .unwrap_err();
            assert_eq!(error.param(), Some("messages[0].images"));
        }
    }
}

#[test]
fn function_strict_defaults_and_explicit_values_remain_unchanged() {
    for strict in [
        None,
        Some(Value::Null),
        Some(json!(false)),
        Some(json!(true)),
    ] {
        let mut function =
            json!({"name":"lookup.exact","description":"Lookup","parameters":{"type":"object"}});
        if let Some(strict) = &strict {
            function["strict"] = strict.clone();
        }
        let decoded = decode_tools(json!([
            {"type":"function","function":function},{"type":"custom","custom":{"name":"custom.exact"}}
        ])).unwrap();
        assert_eq!(decoded.responses["tools"][0]["name"], "lookup.exact");
        assert_eq!(
            decoded.responses["tools"][0]["parameters"],
            function["parameters"]
        );
        assert_eq!(
            decoded.responses["tools"][0]["strict"],
            strict.filter(|v| !v.is_null()).unwrap_or(json!(false))
        );
    }
    for tool in [
        json!({"type":"function"}),
        json!({"type":"function","function":{}}),
        json!({"type":"function","function":{"name":"run","strict":"false"}}),
        json!({"type":"function","function":{"name":"run","parameters":[]}}),
        json!({"type":"function","function":{"name":"run","description":null}}),
        json!({"type":"function","function":{"name":"run","unknown":"secret-body"}}),
        json!({"type":"function","function":{"name":"run"},"custom":{"name":"hidden"}}),
    ] {
        assert!(decode_tools(json!([tool])).is_err());
    }
}

#[test]
fn unsupported_tool_types_and_namespaces_remain_explicit_errors() {
    for tool in [
        json!({"type":"mcp","server_label":"remote"}),
        json!({"type":"computer"}),
        json!({"type":"computer_use_preview"}),
        json!({"type":"namespace","name":"ns","tools":[{"type":"custom","name":"run"}]}),
        json!({"type":"future_tool","payload":"secret-body"}),
    ] {
        let error = decode_tools(json!([tool])).unwrap_err();
        assert_eq!(error.code(), "unsupported_parameter");
        assert_eq!(error.param(), Some("tools[0].type"));
    }
    for kind in ["mcp", "computer", "namespace", "custom_tool_call"] {
        let mut call = custom_call();
        call["type"] = json!(kind);
        assert_eq!(
            decode_chat_request(history(call, json!("ok")))
                .unwrap_err()
                .code(),
            "unsupported_parameter"
        );
    }
    let mut input = request();
    input["unknown_secret_field"] = json!("secret-body");
    let error = decode_chat_request(input).unwrap_err();
    assert_eq!(error.param(), Some("request"));
    assert_eq!(error.code(), "unsupported_parameter");
}
