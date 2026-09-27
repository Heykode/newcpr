use std::collections::BTreeMap;

use futures::TryStreamExt;
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

use super::{
    ClientTools, ExcelRequestError, RESPONSES_PATH, images, prepare_request,
    tests::{VERIFIED_MODEL, client, completed, request},
};
use crate::transport::CodexRequestContext;

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
