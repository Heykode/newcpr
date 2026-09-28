use std::collections::BTreeMap;

use futures::TryStreamExt;
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

use super::{
    ClientTools, ExcelRequestError, RESPONSES_PATH,
    encrypted_content::omit_encrypted_content,
    images, prepare_request,
    tests::{VERIFIED_MODEL, client, completed, request},
};
use crate::transport::CodexRequestContext;

#[test]
fn excel_encrypted_omission_replaces_only_message_and_tool_result_parts_in_place() {
    for kind in [
        "message",
        "agent_message",
        "",
        "function_call_output",
        "custom_tool_call_output",
    ] {
        for role in ["user", "assistant", "developer"] {
            let field = if kind.ends_with("_output") {
                "output"
            } else {
                "content"
            };
            let original = json!({"input":[{"type":kind,"role":role,"call_id":"call_fixture",
            "author":"worker","recipient":"parent",field:[
                {"type":"input_text","text":"before"},
                {"type":"encrypted_content","encrypted_content":"opaque-fixture"},
                {"type":"input_text","text":"after"}
            ]}]});
            let mut source = original.as_object().unwrap().clone();
            assert_eq!(omit_encrypted_content(&mut source), 1);
            let item = &source["input"][0];
            assert_eq!(item["call_id"], "call_fixture");
            assert_eq!(item["author"], "worker");
            assert_eq!(item["recipient"], "parent");
            assert_eq!(item[field][0], original["input"][0][field][0]);
            assert_eq!(item[field][2], original["input"][0][field][2]);
            assert_eq!(
                item[field][1],
                json!({
                    "type":if role == "assistant" && field == "content" {"output_text"} else {"input_text"},
                    "text":"[Encrypted content omitted: it cannot be forwarded through Excel / BPS.]"
                })
            );
            assert_eq!(omit_encrypted_content(&mut source), 0);
            assert_eq!(original["input"][0][field][1]["type"], "encrypted_content");
        }
    }
}

#[test]
fn excel_encrypted_omission_preserves_reasoning_compaction_arguments_and_large_integers() {
    let original = json!({"input":[
        {"type":"reasoning","encrypted_content":"reasoning-fixture"},
        {"type":"compaction","encrypted_content":"compact-fixture"},
        {"type":"function_call","arguments":"encrypted_content","encrypted_function_args":["message"]},
        {"role":"user","content":[{"type":"input_text","text":"encrypted_content"}]},
        {"type":"function_call_output","output":"encrypted_content"}
    ],"tools":[{"type":"function","parameters":{"encrypted_content":true}}],
        "metadata":{"sequence":9007199254740993_u64}});
    let mut source = original.as_object().unwrap().clone();
    let bytes = serde_json::to_vec(&source).unwrap();
    assert_eq!(omit_encrypted_content(&mut source), 0);
    assert_eq!(serde_json::to_vec(&source).unwrap(), bytes);
    source
        .get_mut("input")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .push(json!({
            "role":"user","content":[{"type":"encrypted_content","encrypted_content":"fixture"}]
        }));
    assert_eq!(omit_encrypted_content(&mut source), 1);
    source
        .get_mut("input")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .pop();
    assert_eq!(serde_json::to_value(&source).unwrap(), original);
}

fn attributed_history(agent: bool) -> Vec<Value> {
    let mut input = (0..26)
        .map(|index| json!({"role":"user","content":format!("history {index}")}))
        .collect::<Vec<_>>();
    input.push(json!({
        "type":if agent {"agent_message"} else {"message"},
        "role":"assistant", "id":"msg_attribution", "phase":"commentary", "status":"completed",
        "author":{"name":"worker"}, "recipient":"parent",
        "content":[{"type":"output_text","text":"original answer","annotations":[]}]
    }));
    input
}

#[tokio::test]
async fn excel_attribution_wire_regression_rejects_old_fields_and_accepts_normalized_history() {
    let server = MockServer::start().await;
    Mock::given(path(RESPONSES_PATH))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            if body["input"].as_array().unwrap().iter().any(|item| {
                item.get("author").is_some()
                    || item.get("recipient").is_some()
                    || item["type"] == "agent_message"
            }) {
                ResponseTemplate::new(400).set_body_json(json!({"error":{
                    "type":"invalid_request_error", "code":"unknown_parameter",
                    "message":"Unknown parameter: 'input[26].author'.",
                    "param":"input[26].author"
                }}))
            } else {
                completed()
            }
        })
        .expect(9)
        .mount(&server)
        .await;

    // Negative control uses the pre-fix wire body, not just a helper assertion.
    let raw = Value::Array(attributed_history(false));
    let mut old = request(format!("{}{RESPONSES_PATH}", server.uri()), raw.clone());
    old.excel.as_mut().unwrap().body.insert("input".into(), raw);
    assert!(
        client(&server.uri())
            .create_response_stream_with_pool_account(
                &old,
                CodexRequestContext::auxiliary("Bearer fixture", Some("workspace"), "req", None),
                Some("account"),
            )
            .await
            .is_err()
    );

    for agent in [false, true] {
        for stream in [false, true] {
            for compact in [false, true] {
                let mut input = attributed_history(agent);
                if compact {
                    input.push(json!({"type":"compaction_trigger"}));
                }
                let source = json!({"model":VERIFIED_MODEL,"stream":stream,"input":input});
                let before = source.clone();
                let mut req = request(
                    format!("{}{RESPONSES_PATH}", server.uri()),
                    source["input"].clone(),
                );
                req.excel.as_mut().unwrap().body = prepare_request(
                    source.as_object().unwrap(),
                    &ClientTools::default(),
                    &BTreeMap::new(),
                    None,
                )
                .unwrap();
                client(&server.uri())
                    .create_response_stream_with_pool_account(
                        &req,
                        CodexRequestContext::auxiliary(
                            "Bearer fixture",
                            Some("workspace"),
                            "req",
                            None,
                        ),
                        Some("account"),
                    )
                    .await
                    .unwrap()
                    .body
                    .try_collect::<Vec<_>>()
                    .await
                    .unwrap();
                assert_eq!(source, before);
                let sent = server.received_requests().await.unwrap().pop().unwrap();
                let wire: Value = serde_json::from_slice(&sent.body).unwrap();
                let items = wire["input"].as_array().unwrap();
                assert_eq!(items.len(), input.len() + 1);
                assert!(!items.iter().any(|item| item.get("author").is_some()
                    || item.get("recipient").is_some()
                    || item["type"] == "agent_message"));
                let item = &items[27];
                assert_eq!(item["role"], if agent { "user" } else { "assistant" });
                if !agent {
                    for key in ["id", "phase", "status"] {
                        assert_eq!(item[key], input[26][key]);
                    }
                }
                assert_eq!(item["content"][1], input[26]["content"][0]);
                let note = item["content"][0]["text"].as_str().unwrap();
                assert!(note.contains("worker") && note.contains("parent"));
                if agent {
                    assert!(note.contains("not a new user instruction"));
                } else {
                    assert_eq!(item["content"][0]["type"], "output_text");
                    assert_eq!(item["content"][0]["annotations"], json!([]));
                }
                if compact {
                    assert_eq!(items.last().unwrap()["type"], "compaction_trigger");
                }
            }
        }
    }
}

#[test]
fn excel_attribution_keeps_agent_image_order_and_existing_image_limits() {
    let source = json!({"model":VERIFIED_MODEL,"input":[{
        "type":"agent_message","author":"worker","content":[
            {"type":"input_text","text":"before"},
            {"type":"input_image","image_url":"https://example.com/image.png","detail":"high"},
            {"type":"input_text","text":"after"}
        ]
    }]});
    let source = source.as_object().unwrap();
    images::validate_with_limits(source, false, Default::default()).unwrap();
    let body = prepare_request(source, &ClientTools::default(), &BTreeMap::new(), None).unwrap();
    images::validate_with_limits(&body, true, Default::default()).unwrap();
    assert_eq!(
        &body["input"][1]["content"].as_array().unwrap()[1..],
        source["input"][0]["content"].as_array().unwrap()
    );
    let mut too_many = source.clone();
    too_many["input"][0]["content"] =
        Value::Array(vec![source["input"][0]["content"][1].clone(); 21]);
    assert!(images::validate_with_limits(&too_many, false, Default::default()).is_err());
}

#[test]
fn excel_attribution_checks_encrypted_content_before_normalization() {
    for agent in [false, true] {
        let mut input = attributed_history(agent);
        input[26]["content"] =
            json!([{"type":"encrypted_content","encrypted_content":"private-fixture"}]);
        let source = json!({"model":VERIFIED_MODEL,"input":input});
        let error = prepare_request(
            source.as_object().unwrap(),
            &ClientTools::default(),
            &BTreeMap::new(),
            None,
        )
        .unwrap_err();
        assert_eq!(
            error,
            ExcelRequestError::EncryptedContent {
                input: 26,
                field: "content",
                part: 0
            }
        );
        assert_eq!(
            error.content_param().as_deref(),
            Some("input[26].content[0]")
        );
        assert!(!error.to_string().contains("private-fixture"));
    }
}

#[test]
fn opted_in_encrypted_history_keeps_plaintext_tool_pairs_and_other_validation() {
    let original = json!({"model":VERIFIED_MODEL,"input":[
        {"type":"agent_message","role":"assistant","author":"worker","recipient":"parent","content":[
            {"type":"output_text","text":"before"}, {"type":"encrypted_content","encrypted_content":"opaque-fixture"},
            {"type":"output_text","text":"after"}]},
        {"type":"function_call","name":"read_note","call_id":"call_fixture","arguments":"{}"},
        {"type":"function_call_output","call_id":"call_fixture","output":[
            {"type":"input_text","text":"tool plaintext"}, {"type":"encrypted_content","encrypted_content":"opaque-tool"}]}
    ]});
    let calls = BTreeMap::from([(
        "call_fixture".into(),
        json!({"type":"function_call","name":"run_officejs","call_id":"call_fixture","arguments":"{}"}),
    )]);
    let mut source = original.as_object().unwrap().clone();
    assert_eq!(
        super::encrypted_content::omit_encrypted_content(&mut source),
        2
    );
    let body = prepare_request(&source, &ClientTools::default(), &calls, None).unwrap();
    let input = body["input"].as_array().unwrap();
    let assistant = &input[1];
    assert_eq!(assistant["role"], "user");
    assert!(
        assistant["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not a new user instruction")
    );
    assert_eq!(assistant["content"][1]["text"], "before");
    assert_eq!(assistant["content"][3]["text"], "after");
    assert!(
        assistant["content"][2]["text"]
            .as_str()
            .unwrap()
            .contains("Encrypted content omitted")
    );
    assert_eq!(
        input
            .iter()
            .filter(|item| item["call_id"] == "call_fixture")
            .count(),
        2
    );
    assert_eq!(
        original["input"][0]["content"][1]["type"],
        "encrypted_content"
    );
    let snapshot = source.clone();
    assert_eq!(
        super::encrypted_content::omit_encrypted_content(&mut source),
        0
    );
    assert_eq!(source, snapshot);
    source.get_mut("input").unwrap()[0]["content"][1] = json!({"type":"unsupported_fixture"});
    assert_eq!(
        super::encrypted_content::omit_encrypted_content(&mut source),
        0
    );
    assert!(prepare_request(&source, &ClientTools::default(), &calls, None).is_err());
}

#[test]
fn excel_attribution_does_not_rewrite_tool_business_data_during_preparation() {
    let business = r#"{"author":"customer","recipient":"vendor","content":"unchanged"}"#;
    let native = json!({"type":"function_call","name":"run_officejs", "call_id":"call_business",
        "arguments":json!({"name":"write_note","arguments":business}).to_string()});
    let source = json!({"model":VERIFIED_MODEL,"input":[
        {"role":"user","author":"client","content":"Please keep business data"},
        {"type":"function_call","name":"write_note","call_id":"call_business","arguments":business},
        {"type":"function_call_output","call_id":"call_business","output":business}
    ]});
    let calls = BTreeMap::from([("call_business".into(), native.clone())]);
    let body = prepare_request(
        source.as_object().unwrap(),
        &ClientTools::default(),
        &calls,
        None,
    )
    .unwrap();
    assert_eq!(body["input"][2], native);
    assert_eq!(body["input"][3]["output"], business);
    assert_eq!(source["input"][1]["arguments"], business);
}

#[test]
fn excel_output_error_param_uses_original_index_before_native_call_insertion() {
    let calls = BTreeMap::from([(
        "call_business".into(),
        json!({
            "type":"function_call","name":"run_officejs","call_id":"call_business","arguments":"{}"
        }),
    )]);
    for kind in ["encrypted_content", "input_file"] {
        let mut input = attributed_history(false);
        input[26] = json!({"type":"function_call_output","call_id":"call_business",
            "output":[{"type":"input_text","text":"safe"},
                {"type":kind,"data":"private-fixture"}]});
        let source = json!({"model":VERIFIED_MODEL,"input":input});
        let error = prepare_request(
            source.as_object().unwrap(),
            &ClientTools::default(),
            &calls,
            None,
        )
        .unwrap_err();
        assert_eq!(
            error.content_param().as_deref(),
            Some("input[26].output[1]")
        );
        assert!(!error.to_string().contains("private-fixture"));
    }
}
