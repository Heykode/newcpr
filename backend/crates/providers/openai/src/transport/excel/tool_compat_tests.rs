use super::{ClientTools, ExcelPreparedRequest, ExcelRequestError, transform_stream};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{Value, json};

fn parse(source: Value) -> Result<ClientTools, ExcelRequestError> {
    ClientTools::parse(source.as_object().unwrap())
}

fn function(schema: Value) -> Value {
    json!({"type":"function","name":"read","parameters":schema})
}

fn native(name: &str, args: Value) -> Value {
    json!({"type":"function_call","id":"fc_fixture","call_id":"call_fixture",
        "name":"run_officejs","arguments":json!({"code":json!({"name":name,"arguments":args}).to_string()}).to_string()})
}

#[test]
fn transport_namespace_never_changes_the_declared_client_target() {
    let tools = parse(json!({"tools":[function(json!({"type":"object"})),
        {"type":"namespace","name":"workspace","tools":[function(json!({"type":"object"}))]},
        {"type":"namespace","name":"functions","tools":[function(json!({"type":"object"}))]}
    ]}))
    .unwrap();
    for target in ["read", "workspace.read", "functions.read"] {
        let mut call = native(target, json!({}));
        let expected = tools.convert_call(&call).unwrap();
        for outer_name in ["run_officejs", "functions.run_officejs"] {
            for namespace in ["functions", "unrelated"] {
                call["name"] = outer_name.into();
                call["namespace"] = namespace.into();
                assert_eq!(tools.convert_call(&call), Ok(expected.clone()));
            }
        }
    }
    let direct = json!({"type":"function_call","call_id":"direct","name":"read",
        "namespace":"workspace","arguments":"{}"});
    assert_eq!(
        tools.convert_call(&direct).unwrap()["namespace"],
        "workspace"
    );
    let mut wrong = direct;
    wrong["namespace"] = "unrelated".into();
    assert!(tools.convert_call(&wrong).is_err());
    wrong["name"] = "functions.read".into();
    assert!(tools.convert_call(&wrong).is_err());
}

#[test]
fn function_schemas_enforce_references_composition_and_constraints() {
    let cases = [
        (
            json!({"type":"object","oneOf":[{"required":["path"]},{"required":["uri"]}]}),
            json!({"path":"x"}),
            json!({}),
        ),
        (
            json!({"type":"object","properties":{"path":{"$ref":"#/$defs/path"}},"$defs":{"path":{"type":"string"}},"required":["path"]}),
            json!({"path":"x"}),
            json!({"path":17}),
        ),
        (
            json!({"type":"object","properties":{"limit":{"type":"integer","minimum":0,"maximum":10}}}),
            json!({"limit":5}),
            json!({"limit":-1}),
        ),
        (
            json!({"type":"object","additionalProperties":false}),
            json!({}),
            json!({"extra":1}),
        ),
        (
            json!({"type":"object","properties":{"path":{"type":"string","pattern":"^safe/"}}}),
            json!({"path":"safe/x"}),
            json!({"path":"bad/x"}),
        ),
        (
            json!({"type":"object","properties":{"kind":{"const":"read"}}}),
            json!({"kind":"read"}),
            json!({"kind":"write"}),
        ),
        (
            json!({"type":"object","anyOf":[{"required":["path"]},{"required":["uri"]}]}),
            json!({"uri":"x"}),
            json!({}),
        ),
        (
            json!({"type":"object","allOf":[{"required":["path"]},{"required":["limit"]}]}),
            json!({"path":"x","limit":1}),
            json!({"path":"x"}),
        ),
    ];
    for (schema, valid, invalid) in cases {
        let tools = parse(json!({"tools":[function(schema.clone())]})).unwrap();
        assert!(
            tools.convert_call(&native("read", valid)).is_ok(),
            "{schema}"
        );
        assert!(
            tools.convert_call(&native("read", invalid)).is_err(),
            "{schema}"
        );
    }
    let tools = parse(json!({"tools":[function(json!(false))]})).unwrap();
    assert!(tools.convert_call(&native("read", json!({}))).is_err());
    let tools = parse(json!({"tools":[function(json!(true))]})).unwrap();
    assert!(tools.convert_call(&native("read", json!({}))).is_ok());
}

#[test]
fn function_schemas_reject_invalid_external_and_oversized_schemas() {
    for schema in [
        json!({"type":"unknown"}),
        json!({"required":"path"}),
        json!({"$ref":"https://example.invalid/schema.json"}),
        json!({"$ref":"file:///unavailable-schema.json"}),
        json!({"description":"x".repeat(1024 * 1024)}),
    ] {
        assert!(parse(json!({"tools":[function(schema)]})).is_err());
    }
}

#[test]
fn required_and_named_choices_enforce_cardinality_and_exact_identity() {
    let catalog = json!([function(json!({})), {"type":"function","name":"write"},
        {"type":"custom","name":"patch"},
        {"type":"namespace","name":"workspace","tools":[function(json!({}))]}]);
    let required = parse(json!({"tool_choice":"required","tools":catalog})).unwrap();
    assert!(required.instructions().contains("at least one"));
    assert!(required.reminder().unwrap().contains("at least one"));
    assert!(required.validate_call_count(0).is_err());
    assert!(required.validate_call_count(2).is_ok());
    let serial =
        parse(json!({"tool_choice":"required","parallel_tool_calls":false,"tools":catalog}))
            .unwrap();
    assert!(serial.validate_call_count(1).is_ok());
    assert!(serial.validate_call_count(2).is_err());
    for choice in [
        json!({"type":"function","name":"workspace.read"}),
        json!({"type":"function","name":"read","namespace":"workspace"}),
    ] {
        let tools = parse(json!({"tool_choice":choice,"tools":catalog})).unwrap();
        assert!(
            tools
                .instructions()
                .contains("exactly one call to catalog tool workspace.read")
        );
        assert!(
            tools
                .convert_call(&native("workspace.read", json!({})))
                .is_ok()
        );
        assert!(tools.convert_call(&native("read", json!({}))).is_err());
        assert!(tools.validate_call_count(0).is_err());
        assert!(tools.validate_call_count(1).is_ok());
        assert!(tools.validate_call_count(2).is_err());
    }
    let tools =
        parse(json!({"tool_choice":{"type":"custom","name":"patch"},"tools":catalog})).unwrap();
    let raw = "*** Begin Patch\n*** End Patch\n";
    let call = json!({"type":"function_call","call_id":"patch_fixture","name":"run_officejs",
        "arguments":json!({"summary":"cpr.custom/patch","code":raw}).to_string()});
    assert_eq!(tools.convert_call(&call).unwrap()["input"], raw);
    assert!(tools.convert_call(&native("read", json!({}))).is_err());
}

#[test]
fn tool_choice_never_authorizes_undeclared_or_hosted_tools() {
    for source in [
        json!({"tool_choice":"required"}),
        json!({"tool_choice":"required","tools":[{"type":"web_search"}]}),
        json!({"tool_choice":{"type":"web_search"},"tools":[{"type":"web_search"}]}),
        json!({"tool_choice":{"type":"function","name":"absent"},"tools":[function(json!({}))]}),
        json!({"tool_choice":{"type":"custom","name":"read"},"tools":[function(json!({}))]}),
        json!({"tool_choice":{"type":"function","name":"read","namespace":17},"tools":[function(json!({}))]}),
        json!({"tool_choice":{"type":"function","name":"read","unexpected":true},"tools":[function(json!({}))]}),
    ] {
        assert!(parse(source).is_err());
    }
    let tools = parse(json!({"tool_choice":"none","tools":[function(json!({}))]})).unwrap();
    assert!(tools.convert_call(&native("read", json!({}))).is_err());
    assert!(tools.validate_call_count(0).is_ok());
}

async fn relay(tools: ClientTools, output: Value) -> (String, bool) {
    let prepared = ExcelPreparedRequest {
        body: Default::default(),
        tools,
        structured: None,
        _image_lease: None,
        completed: Default::default(),
        usage: Default::default(),
        replay: None,
        endpoint: "https://example.invalid".into(),
    };
    let event = json!({"type":"response.completed","response":{
        "id":"resp_fixture","status":"completed","tool_choice":"auto","output":output
    }});
    let source = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(format!(
        "data: {event}\n\n"
    )))]));
    let mut stream = transform_stream(source, &prepared);
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => text.push_str(std::str::from_utf8(&bytes).unwrap()),
            Err(_) => return (text, false),
        }
    }
    (text, true)
}

#[tokio::test]
async fn forced_tool_completion_validates_before_delivering_calls() {
    let source = json!({"tool_choice":{"type":"function","name":"read"},"tools":[
        function(json!({"type":"object","required":["limit"],"properties":{"limit":{"type":"integer","minimum":1}}})),
        {"type":"function","name":"write"}
    ]});
    let mut valid = native("read", json!({"limit":1}));
    valid["namespace"] = "functions".into();
    let (text, ok) = relay(parse(source.clone()).unwrap(), json!([valid.clone()])).await;
    assert!(ok);
    assert!(text.contains("response.function_call_arguments.done"));
    assert!(text.contains("\"tool_choice\":{\"type\":\"function\",\"name\":\"read\"}"));
    for output in [
        json!([]),
        json!([native("write", json!({}))]),
        json!([native("read", json!({"limit":0}))]),
        json!([valid.clone(), valid]),
    ] {
        let (text, ok) = relay(parse(source.clone()).unwrap(), output).await;
        assert!(!ok);
        assert!(!text.contains("response.function_call_arguments"));
        assert!(!text.contains("response.completed"));
    }
}

#[tokio::test]
async fn required_tool_allows_explicit_refusal_but_not_silent_text_substitution() {
    let source = json!({"tool_choice":"required","tools":[function(json!({}))]});
    for (content, expected) in [
        (
            json!([{"type":"refusal","refusal":"I cannot help with that request."}]),
            true,
        ),
        (
            json!([{"type":"output_text","text":"No tool was used."}]),
            false,
        ),
        (json!([{"type":"refusal","refusal":""}]), false),
    ] {
        let (_, ok) = relay(
            parse(source.clone()).unwrap(),
            json!([
                {"type":"message","role":"assistant","id":"msg_fixture","content":content}
            ]),
        )
        .await;
        assert_eq!(ok, expected);
    }
}
