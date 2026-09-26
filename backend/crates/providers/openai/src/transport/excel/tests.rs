use super::*;
use crate::{
    config::OpenAiConfig,
    transport::{
        CodexBackendClient, CodexBackendTransport, CodexRequestContext,
        protocol::responses::{CodexResponsesRequest, TransportRequirement, transport_requirement},
    },
};
use futures::TryStreamExt;
use futures::future::BoxFuture;
use gateway_core::{
    account::OpaqueProviderData,
    provider_ports::{ProviderReplayPort, ProviderStoreError, ProviderStoreErrorKind},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

pub(super) const VERIFIED_MODEL: &str = "gpt-5.6-sol";

#[derive(Default)]
pub(super) struct MemoryReplay(Mutex<BTreeMap<String, OpaqueProviderData>>);

impl ProviderReplayPort for MemoryReplay {
    fn read<'a>(
        &'a self,
        key: &'a str,
    ) -> BoxFuture<'a, Result<Option<OpaqueProviderData>, ProviderStoreError>> {
        Box::pin(async move { Ok(self.0.lock().unwrap().get(key).cloned()) })
    }

    fn write<'a>(
        &'a self,
        key: &'a str,
        value: &'a OpaqueProviderData,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            let mut records = self.0.lock().unwrap();
            if let Some(existing) = records.get(key)
                && existing != value
            {
                return Err(ProviderStoreError::new(
                    ProviderStoreErrorKind::Conflict,
                    "replay",
                ));
            }
            records.insert(key.into(), value.clone());
            Ok(())
        })
    }
}

pub(super) fn request(endpoint: String, input: Value) -> CodexResponsesRequest {
    let mut request = CodexResponsesRequest::from_body(
        json!({"model":VERIFIED_MODEL,"input":input,"stream":false})
            .as_object()
            .unwrap()
            .clone(),
    );
    request.downstream_websocket_connection_id = Some("ws_fixture".into());
    request.use_websocket = true;
    let tools = ClientTools::default();
    request.excel = Some(ExcelPreparedRequest {
        body: prepare_request(request.body(), &tools, &BTreeMap::new(), None).unwrap(),
        tools,
        structured: None,
        _image_lease: None,
        completed: Default::default(),
        usage: Default::default(),
        replay: None,
        endpoint,
    });
    request
}

pub(super) fn client(base: &str) -> CodexBackendClient {
    CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        base,
        OpenAiConfig::default().wire_profile_state(),
    )
}

pub(super) fn completed() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .insert_header("set-cookie", "excel_secret=fixture; Path=/")
        .insert_header("x-codex-turn-state", "unwanted-state")
        .set_body_string("event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fixture\",\"status\":\"completed\",\"output\":[]}}\n\n")
}

fn fixture_tools() -> ClientTools {
    ClientTools::parse(json!({"tools":[
        {"type":"function","name":"read","parameters":{"type":"object","required":["path"],"properties":{"path":{"type":"string"}}}},
        {"type":"custom","name":"apply_patch"}
    ]}).as_object().unwrap()).unwrap()
}

fn native_fixture(code: Value) -> Value {
    json!({"type":"function_call","name":"run_officejs","id":"fc_fixture","call_id":"call_fixture",
        "arguments":json!({"code":code}).to_string()})
}

fn function_code_source() -> Value {
    json!({"tools":[{"type":"namespace","name":"runner","tools":[
        {"type":"function","name":"execute","parameters":{"type":"object",
            "properties":{"code":{"type":"string"},"timeout_ms":{"type":"integer"},
                "permission":{"type":"string"},"id":{"type":"integer"}},
            "required":["code"],"additionalProperties":false}}
    ]}]})
}

fn function_code_native(name: &str, code: &str, metadata: Value) -> Value {
    let mut native = native_fixture(Value::Null);
    native["arguments"] = json!({"summary":format!("codex2api.function_code/{name}"),
        "code":code,"extended_summary":metadata})
    .to_string()
    .into();
    native["encrypted_function_args"] = json!(["outer_encryption_must_not_leak"]);
    native
}

#[test]
fn excel_function_code_transport_preserves_source_metadata_and_namespace() {
    let source = function_code_source();
    let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
    for code in [
        "",
        "print(\"quoted\")\nC:\\temp\\not-an-envelope\r\t\u{0}中文",
    ] {
        let native = function_code_native(
            "runner.execute",
            code,
            json!("{\"timeout_ms\":30000,\"permission\":\"restricted\",\"id\":9007199254740993}"),
        );
        let converted = tools.convert_call(&native).unwrap();
        let args: Value = serde_json::from_str(converted["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args["code"], code);
        assert_eq!(args["timeout_ms"], 30000);
        assert_eq!(args["permission"], "restricted");
        assert_eq!(args["id"].as_u64(), Some(9_007_199_254_740_993));
        assert_eq!(converted["namespace"], "runner");
        assert_eq!(converted["name"], "execute");
        assert_eq!(converted["encrypted_function_args"], json!([]));
    }
    assert!(tools.instructions().contains("codex2api.function_code/"));
    assert!(tools.reminder().unwrap().contains("runner.execute"));
    let old = tools::rebuild_history_call(&json!({"type":"function_call","name":"execute",
        "namespace":"runner","call_id":"old_call","arguments":"{\"code\":\"legacy\"}"}))
    .unwrap();
    assert_eq!(
        tools.convert_call(&old).unwrap()["arguments"],
        "{\"code\":\"legacy\"}"
    );
}

#[test]
fn excel_function_code_transport_rejects_wrong_catalog_and_malformed_metadata() {
    let source = function_code_source();
    let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
    for name in [
        "execute",
        "functions.runner.execute",
        "runner.missing",
        "runner.execute/",
        " runner.execute",
    ] {
        assert!(
            tools
                .convert_call(&function_code_native(name, "code", json!("{}")))
                .is_err()
        );
    }
    for metadata in [
        json!("{\"code\":\"duplicate\"}"),
        json!("null"),
        json!("[]"),
        json!("{} {}"),
        json!("{\"timeout_ms\":\"wrong\"}"),
        json!({}),
        json!("{\"unknown\":1}"),
    ] {
        assert!(
            tools
                .convert_call(&function_code_native("runner.execute", "code", metadata))
                .is_err()
        );
    }
    for tool in [
        json!({"type":"function","name":"execute"}),
        json!({"type":"function","name":"execute","parameters":{"type":"object","properties":{"code":{"type":"number"}}}}),
        json!({"type":"custom","name":"execute","parameters":{"type":"object","properties":{"code":{"type":"string"}}}}),
    ] {
        let source = json!({"tools":[tool]});
        let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
        assert!(!tools.instructions().contains("codex2api.function_code/"));
        assert!(
            tools
                .convert_call(&function_code_native("execute", "code", json!("{}")))
                .is_err()
        );
    }
}

#[test]
fn excel_function_code_transport_bounds_decoded_and_encoded_sizes() {
    let source = function_code_source();
    let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
    let mut native = function_code_native("runner.execute", "", json!("{}"));
    let mut args: Value = serde_json::from_str(native["arguments"].as_str().unwrap()).unwrap();
    for code in ["x".repeat(1024 * 1024 + 1), "\u{0}".repeat(200_000)] {
        args["code"] = code.into();
        native["arguments"] = args.clone();
        assert!(tools.convert_call(&native).is_err());
    }
    args["code"] = "x".into();
    args["extended_summary"] = format!("{{}}{}", " ".repeat(1024 * 1024)).into();
    native["arguments"] = args;
    assert!(tools.convert_call(&native).is_err());
}

#[tokio::test]
async fn excel_function_code_stream_converts_only_validated_completed_arguments() {
    let source = function_code_source();
    let native = function_code_native("runner.execute", "print(\"x\")\n", json!("{}"));
    let events = transformed_fixture(source, vec![
        json!({"type":"response.output_item.added","output_index":0,"item":native}),
        json!({"type":"response.function_call_arguments.delta","delta":"not client code"}),
        json!({"type":"response.completed","response":{"id":"resp_fixture","status":"completed","output":[native]}})
    ]).await.unwrap();
    let deltas: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "response.function_call_arguments.delta")
        .collect();
    assert_eq!(deltas.len(), 1);
    let args: Value = serde_json::from_str(deltas[0]["delta"].as_str().unwrap()).unwrap();
    assert_eq!(args["code"], "print(\"x\")\n");
    assert_eq!(
        events.last().unwrap()["response"]["output"][0]["name"],
        "execute"
    );
}

#[test]
fn excel_plaintext_tool_arguments_are_explicitly_not_encrypted() {
    let tools = ClientTools::parse(
        json!({"tools":[
            {"type":"function","name":"spawn_agent","parameters":{"type":"object",
                "properties":{"message":{"type":"string","encrypted":true}}}}
        ]})
        .as_object()
        .unwrap(),
    )
    .unwrap();
    let mut native = native_fixture(json!({"name":"spawn_agent","arguments":{"message":"hello"}}));
    native["encrypted_function_args"] = json!(["code"]);
    let converted = tools.convert_call(&native).unwrap();
    assert_eq!(converted["encrypted_function_args"], json!([]));
    assert_eq!(converted["arguments"], "{\"message\":\"hello\"}");
    let mut direct = json!({"type":"function_call","name":"spawn_agent",
        "call_id":"call_direct","arguments":"{\"message\":\"opaque\"}",
        "encrypted_function_args":["message"]});
    assert_eq!(
        tools.convert_call(&direct).unwrap()["encrypted_function_args"],
        json!(["message"])
    );
    direct
        .as_object_mut()
        .unwrap()
        .remove("encrypted_function_args");
    assert_eq!(
        tools.convert_call(&direct).unwrap()["encrypted_function_args"],
        json!([])
    );
    let tools = ClientTools::parse(
        json!({"tools":[{"type":"function","name":"update_plan"}]})
            .as_object()
            .unwrap(),
    )
    .unwrap();
    let plan = json!({"type":"function_call","name":"update_plan","call_id":"call_plan",
        "arguments":json!({"plan":[{"title":"read","status":"todo"}]}).to_string(),
        "encrypted_function_args":["plan"]});
    assert_eq!(
        tools.convert_call(&plan).unwrap()["encrypted_function_args"],
        json!([])
    );
}

#[test]
fn excel_single_invocation_uses_the_callee_not_payload_fields() {
    let tools = ClientTools::parse(
        json!({"tools":[
            {"type":"function","name":"exec","parameters":{"type":"object"}},
            {"type":"function","name":"functions.exec","parameters":{"type":"object"}},
            {"type":"custom","name":"patch"}
        ]})
        .as_object()
        .unwrap(),
    )
    .unwrap();
    let args =
        r#"{"name":"not_a_tool","arguments":{"id":9007199254740993},"big":18446744073709551617}"#;
    for prefix in ["", "await ", "return ", "return await ", "return\nawait\t"] {
        let converted = tools
            .convert_call(&native_fixture(json!(format!("{prefix}exec({args}) ;"))))
            .unwrap();
        assert_eq!(converted["name"], "exec");
        let result: Value = serde_json::from_str(converted["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(result, serde_json::from_str::<Value>(args).unwrap());
        assert!(
            converted["arguments"]
                .as_str()
                .unwrap()
                .contains("18446744073709551617")
        );
    }
    let direct = tools
        .convert_call(&native_fixture(json!("functions.exec({})")))
        .unwrap();
    assert_eq!(direct["name"], "functions.exec");
    let raw = "line\n\"quoted\"\\exact";
    let converted = tools
        .convert_call(&native_fixture(json!(format!(
            "await patch({})",
            serde_json::to_string(raw).unwrap()
        ))))
        .unwrap();
    assert_eq!(converted["input"], raw);
    for code in [
        "exec({}); exec({})",
        "exec({}, {})",
        "exec({}) trailing",
        "unknown({})",
        "exec([{}])",
        "exec(\"text\")",
        "patch({})",
        "exec({}",
        "const x = exec({});",
        "await return exec({})",
        "exec({}); // ignored?",
        "exec({\"x\":1e})",
    ] {
        assert!(
            tools.convert_call(&native_fixture(json!(code))).is_err(),
            "{code}"
        );
    }
}

#[test]
fn excel_optional_hosted_declarations_do_not_block_text_or_authorize_calls() {
    for kind in [
        "web_search",
        "web_search_preview",
        "web_search_preview_2025_03_11",
        "web_search_2025_08_26",
        "tool_search",
        "image_generation",
        "file_search",
        "code_interpreter",
        "computer",
        "computer_use_preview",
        "mcp",
    ] {
        let mut source = json!({"tool_choice":"auto","tools":[{"type":kind}]});
        let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
        assert!(tools.instructions().contains("unavailable"));
        assert!(tools.unavailable().contains(kind));
        assert!(
            tools
                .convert_call(&native_fixture(json!({"name":kind,"arguments":{}})))
                .is_err()
        );
        source["tool_choice"] = json!({"type":kind});
        assert!(ClientTools::parse(source.as_object().unwrap()).is_err());
    }
    let source = json!({"tools":[{"type":"web_search"},{"type":"function","name":"read"}]});
    let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
    assert!(
        tools
            .convert_call(&native_fixture(json!("read({})")))
            .is_ok()
    );
    assert!(tools.instructions().contains("unavailable"));
    for source in [
        json!({"tools":[{"type":"unknown"}]}),
        json!({"tool_choice":"required","tools":[{"type":"web_search"}]}),
        json!({"tools":[{"type":"function"}]}),
    ] {
        assert!(ClientTools::parse(source.as_object().unwrap()).is_err());
    }
}

#[test]
fn excel_effort_normalization_is_explicit_and_content_errors_are_private() {
    for (requested, effective) in [
        ("max", "xhigh"),
        (" ULTRA ", "xhigh"),
        ("extra_high", "xhigh"),
        ("none", "low"),
        ("minimal", "low"),
        ("", "medium"),
        ("high", "high"),
    ] {
        let source =
            json!({"model":VERIFIED_MODEL,"input":"hello","reasoning":{"effort":requested}});
        let body = prepare_request(
            source.as_object().unwrap(),
            &ClientTools::default(),
            &BTreeMap::new(),
            None,
        )
        .unwrap();
        assert_eq!(body["reasoning_effort"], effective);
    }
    for reasoning in [
        json!({"effort":7}),
        json!({"effort":"unknown"}),
        json!({"mode":"adaptive"}),
    ] {
        let source = json!({"model":VERIFIED_MODEL,"input":"hello","reasoning":reasoning});
        assert_eq!(
            prepare_request(
                source.as_object().unwrap(),
                &ClientTools::default(),
                &BTreeMap::new(),
                None
            ),
            Err(ExcelRequestError::Effort)
        );
    }
    for (part, label) in [
        (
            json!({"type":"input_file","file_data":"PRIVATE_BODY"}),
            "input_file",
        ),
        (json!({"type":"PRIVATE_TYPE\nPRIVATE_BODY"}), "unknown"),
        (json!("PRIVATE_BODY"), "non_object"),
        (json!({}), "missing"),
        (json!({"type":null}), "non_string"),
    ] {
        let source = json!({"model":VERIFIED_MODEL,"input":[{"role":"user","content":[
            {"type":"input_text","text":"PRIVATE_BODY"},part]}]});
        let error = prepare_request(
            source.as_object().unwrap(),
            &ClientTools::default(),
            &BTreeMap::new(),
            None,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("unsupported Excel content (path=input[0].content[1]; type={label})")
        );
        assert!(!error.to_string().contains("PRIVATE"));
    }
    let native_calls = BTreeMap::from([(
        "call_fixture".into(),
        native_fixture(json!({"name":"read","arguments":{}})),
    )]);
    let source = json!({"model":VERIFIED_MODEL,"input":[
        {"type":"function_call_output","call_id":"call_fixture","output":[{"type":"input_audio"}]}]});
    let error = prepare_request(
        source.as_object().unwrap(),
        &ClientTools::default(),
        &native_calls,
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("input[0].output[0]"));
}

#[test]
fn excel_content_validation_preserves_images_encryption_and_final_compaction_trigger() {
    let source = json!({"model":VERIFIED_MODEL,"input":[
        {"type":"compaction_trigger"},
        {"role":"user","content":[{"type":"input_text","text":"describe"},
            {"type":"input_image","image_url":"https://images.example.com/test.png"}]},
        {"type":"reasoning","encrypted_content":"opaque-fixture","summary":[]}
    ]});
    let body = prepare_request(
        source.as_object().unwrap(),
        &ClientTools::default(),
        &BTreeMap::new(),
        None,
    )
    .unwrap();
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[1], source["input"][1]);
    assert_eq!(input[2], source["input"][2]);
    assert_eq!(input.last().unwrap(), &json!({"type":"compaction_trigger"}));
}

#[tokio::test]
async fn excel_stream_projects_effective_effort_and_plaintext_metadata_on_all_tool_events() {
    let source = json!({"model":VERIFIED_MODEL,"input":"delegate","reasoning":{"effort":"max"},
        "tools":[{"type":"function","name":"spawn_agent"}]});
    let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
    let prepared = ExcelPreparedRequest {
        body: prepare_request(source.as_object().unwrap(), &tools, &BTreeMap::new(), None).unwrap(),
        tools,
        structured: None,
        _image_lease: None,
        completed: Default::default(),
        usage: Default::default(),
        replay: None,
        endpoint: RESPONSES_URL.into(),
    };
    let events = [
        json!({"type":"response.created","response":{"id":"resp_fixture","status":"in_progress","output":[]}}),
        json!({"type":"response.completed","response":{"id":"resp_fixture","status":"completed",
            "output":[native_fixture(json!("spawn_agent({\"message\":\"hello\"})"))]}}),
    ];
    let chunks = events
        .into_iter()
        .map(|event| Ok(bytes::Bytes::from(format!("data: {event}\n\n"))));
    let bytes = transform_stream(Box::pin(futures::stream::iter(chunks)), &prepared)
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let events = gateway_protocol::openai::sse::SseEventDecoder::default()
        .push(&bytes.concat())
        .unwrap();
    let mut responses = 0;
    let mut tool_items = 0;
    for event in events {
        let event: Value = serde_json::from_str(&event.data).unwrap();
        if let Some(response) = event.get("response") {
            responses += 1;
            assert_eq!(response["reasoning"]["effort"], "xhigh");
            if event["type"] == "response.completed" {
                assert_eq!(response["output"][0]["encrypted_function_args"], json!([]));
            }
        }
        if let Some(item) = event.get("item") {
            tool_items += 1;
            assert_eq!(item["name"], "spawn_agent");
            assert_eq!(item["encrypted_function_args"], json!([]));
        }
    }
    assert_eq!((responses, tool_items), (2, 2));
}

#[test]
fn excel_image_tool_options_are_explicit_and_edits_require_real_images() {
    use super::image_generation::PreparedImage;
    use crate::transport::{CODEX_IMAGE_EDITS_PATH, CODEX_IMAGE_GENERATIONS_PATH};
    for source in [
        json!({"prompt":"draw"}),
        json!({"prompt":"draw","model":"gpt-image-2","n":3,"quality":"low","size":"1024x1024"}),
    ] {
        assert!(
            PreparedImage::parse(source.to_string().as_bytes(), CODEX_IMAGE_GENERATIONS_PATH)
                .is_ok()
        );
    }
    for source in [
        json!({"prompt":""}),
        json!({"prompt":"draw","n":true}),
        json!({"prompt":"draw","n":4}),
        json!({"prompt":"draw","model":"other"}),
        json!({"prompt":"draw","background":"transparent"}),
        json!({"prompt":"draw","output_format":"jpeg"}),
        json!({"prompt":"draw","stream":true}),
        json!({"prompt":"draw","mask":{"image_url":"data:image/png;base64,AQID"}}),
    ] {
        assert!(
            PreparedImage::parse(source.to_string().as_bytes(), CODEX_IMAGE_GENERATIONS_PATH)
                .is_err()
        );
    }
    for images in [
        json!([]),
        json!([{"image_url":"https://images.example.com/picture.png"}]),
        json!([{"image_url":"data:image/png;base64,AQID"}]),
    ] {
        assert!(
            PreparedImage::parse(
                json!({"prompt":"edit","images":images})
                    .to_string()
                    .as_bytes(),
                CODEX_IMAGE_EDITS_PATH
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn excel_image_tool_uses_excel_headers_json_and_multipart_without_codex_state() {
    use super::image_generation::PreparedImage;
    use crate::transport::{CODEX_IMAGE_EDITS_PATH, CODEX_IMAGE_GENERATIONS_PATH};
    let server = MockServer::start().await;
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("set-cookie", "unwanted=fixture")
                .insert_header("x-codex-turn-state", "unwanted")
                .set_body_json(json!({"data":[{"b64_json":png}]})),
        )
        .expect(2)
        .mount(&server)
        .await;
    let client = client(&server.uri());
    for (path, body) in [
        (
            CODEX_IMAGE_GENERATIONS_PATH,
            json!({"prompt":"draw","session_id":"client-fixture"}),
        ),
        (
            CODEX_IMAGE_EDITS_PATH,
            json!({"prompt":"edit","images":[{"image_url":format!("data:image/png;base64,{png}")}]}),
        ),
    ] {
        let mut image = PreparedImage::parse(body.to_string().as_bytes(), path).unwrap();
        image.endpoint = format!("{}/images", server.uri());
        let mut context = CodexRequestContext::auxiliary(
            "Bearer fixture",
            Some("workspace"),
            "req",
            Some("device"),
        );
        context.cookie_header = Some("codex=fixture");
        let response = client.post_excel_image(&image, context).await.unwrap();
        assert!(response.set_cookie_headers.is_empty());
        assert!(
            !response
                .response_metadata
                .client_headers
                .iter()
                .any(|(name, _)| name == "x-codex-turn-state")
        );
    }
    let sent = server.received_requests().await.unwrap();
    assert_eq!(
        sent[0].body_json::<Value>().unwrap()["model"],
        "gpt-image-2"
    );
    assert!(
        sent[0]
            .body_json::<Value>()
            .unwrap()
            .get("session_id")
            .is_none()
    );
    assert!(
        sent[1].headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("multipart/form-data;")
    );
    assert!(String::from_utf8_lossy(&sent[1].body).contains("name=\"image\""));
    for request in &sent {
        assert_eq!(request.headers["chatgpt-account-id"], "workspace");
        assert_eq!(request.headers["x-openai-account-id"], "workspace");
        assert_eq!(request.headers["accept"], "application/json");
        assert!(!request.headers.contains_key("cookie"));
        assert!(!request.headers.contains_key("x-codex-turn-state"));
        assert!(!request.headers.contains_key("oai-device-id"));
    }
}

#[tokio::test]
async fn excel_image_tool_does_not_retry_rejection_or_accept_empty_success() {
    use super::image_generation::PreparedImage;
    use crate::transport::CODEX_IMAGE_GENERATIONS_PATH;
    for (status, body) in [
        (403, json!({"error":{"code":"workspace_not_allowed"}})),
        (200, json!({"data":[]})),
        (200, json!({"error":"failure"})),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let mut image =
            PreparedImage::parse(br#"{"prompt":"draw"}"#, CODEX_IMAGE_GENERATIONS_PATH).unwrap();
        image.endpoint = server.uri();
        let result = client(&server.uri())
            .post_excel_image(
                &image,
                CodexRequestContext::auxiliary("Bearer fixture", Some("workspace"), "req", None),
            )
            .await;
        assert!(result.is_err());
    }
}

#[test]
fn excel_tool_formatting_aliases_and_raw_custom_preserve_values() {
    let tools = fixture_tools();
    for code in [
        json!({"tool":"read","args":{"path":"sample.txt"}}),
        json!("{\"name\":\"read\",\"arguments\":{\"path\":\"sample.txt\"}}"),
        json!("```json\n{\"name\":\"read\",\"arguments\":{\"path\":\"sample.txt\"}}\n```"),
        json!("Tool: {\"name\":\"read\",\"arguments\":{\"path\":\"sample.txt\"}}"),
        json!("Tool: {\"name\":\"read\",\"arguments\":{\"path\":\"sample.txt\"}} Done."),
        json!("read({\"path\":\"sample.txt\"})"),
        json!("functions.read({\"path\":\"sample.txt\"});"),
        json!({"name":"run_officejs","arguments":{"code":"{\"name\":\"read\",\"arguments\":{\"path\":\"sample.txt\"}}"}}),
    ] {
        let converted = tools.convert_call(&native_fixture(code)).unwrap();
        assert_eq!(converted["name"], "read");
        assert_eq!(
            serde_json::from_str::<Value>(converted["arguments"].as_str().unwrap()).unwrap()["path"],
            "sample.txt"
        );
    }
    let raw = "*** Begin Patch\n*** Add File: sample.txt\n+\"quoted\" \\q 中文\n*** End Patch";
    let mut native = native_fixture(json!("unused"));
    native["arguments"] = json!({"summary":"cpr.custom/apply_patch","code":raw})
        .to_string()
        .into();
    assert_eq!(tools.convert_call(&native).unwrap()["input"], raw);
    native["arguments"] = json!({"summary":"cpr.custom/read","code":raw})
        .to_string()
        .into();
    assert!(tools.convert_call(&native).is_err());
    let repaired = tools
        .convert_call(&native_fixture(json!(
            "{\"name\":\"read\",\"arguments\":{\"path\":\"a\nb\\q\"}}"
        )))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(repaired["arguments"].as_str().unwrap()).unwrap()["path"],
        "a\nb\\q"
    );
    for code in [
        json!("[{\"name\":\"read\"},{\"name\":\"read\"}]"),
        json!("Excel.run(async ctx => {});"),
        json!({"name":"read","tool":"delete","arguments":{"path":"x"}}),
        json!({"name":"read","arguments":{"path":"x"},"args":{"path":"y"}}),
        json!("{\"name\":\"read\",\"arguments\":"),
        json!("delete({\"name\":\"read\",\"arguments\":{\"path\":\"x\"}})"),
        json!("read({\"name\":\"read\",\"arguments\":{\"path\":\"x\"}}); delete()"),
        json!(
            "Tool: {\"name\":\"read\",\"arguments\":{\"path\":\"x\"}} {\"name\":\"read\",\"arguments\":{\"path\":\"y\"}}"
        ),
        json!("[{\"name\":\"read\",\"arguments\":{\"path\":\"x\"}}]"),
        json!("const x = {\"name\":\"read\",\"arguments\":{\"path\":\"x\"}};"),
    ] {
        assert!(tools.convert_call(&native_fixture(code)).is_err());
    }
}

#[tokio::test]
async fn excel_parallel_tools_preserve_positions_and_serial_mode_fails_atomically() {
    let mut second = native_fixture(json!({"name":"read","arguments":{"path":"second"}}));
    second["id"] = "fc_second".into();
    second["call_id"] = "call_second".into();
    let output = json!([
        {"type":"message","id":"msg_fixture","role":"assistant","content":[]},
        native_fixture(json!({"name":"read","arguments":{"path":"first"}})),
        second
    ]);
    for parallel in [Value::Null, json!(true), json!(false)] {
        let source =
            json!({"parallel_tool_calls":parallel,"tools":[{"type":"function","name":"read"}]});
        let events = transformed_fixture(source, vec![
            json!({"type":"response.completed","response":{"id":"resp_fixture","status":"completed","output":output}})
        ]).await;
        if parallel == false {
            assert!(events.is_err());
        } else {
            let events = events.unwrap();
            let indices: Vec<_> = events
                .iter()
                .filter(|event| event["type"] == "response.output_item.added")
                .map(|event| event["output_index"].as_u64().unwrap())
                .collect();
            assert_eq!(indices, [1, 2]);
            let last = events.last().unwrap();
            assert_eq!(last["response"]["output"][1]["call_id"], "call_fixture");
            assert_eq!(last["response"]["output"][2]["call_id"], "call_second");
        }
    }
    assert!(
        ClientTools::parse(json!({"parallel_tool_calls":"false"}).as_object().unwrap()).is_err()
    );
}

#[test]
fn excel_nested_namespaces_and_additional_tools_are_exact_and_deduplicated() {
    let declaration = json!({"type":"function","name":"read","parameters":{"type":"object"}});
    let tools = ClientTools::parse(
        json!({"tools":[{"type":"namespace","name":"outer","tools":[
        {"type":"namespace","name":"inner","tools":[declaration,declaration]}
    ]}],"input":[{"type":"additional_tools","tools":[{"type":"custom","name":"patch"}]}]})
        .as_object()
        .unwrap(),
    )
    .unwrap();
    let call = tools
        .convert_call(&native_fixture(
            json!({"name":"outer.inner.read","arguments":{}}),
        ))
        .unwrap();
    assert_eq!(call["namespace"], "outer.inner");
    assert_eq!(call["name"], "read");
    assert!(tools.instructions().contains("\"name\":\"patch\""));
    assert!(
        ClientTools::parse(
            json!({"tools":[declaration,{"type":"custom","name":"read"}]})
                .as_object()
                .unwrap()
        )
        .is_err()
    );
}

#[test]
fn excel_structured_formats_validate_nested_values_and_never_fetch_schemas() {
    let source = json!({"text":{"format":{"type":"json_schema","name":"answer","strict":true,
        "schema":{"type":"object","properties":{"items":{"type":"array","items":{"$ref":"#/$defs/positive"}}},
            "required":["items"],"additionalProperties":false,"$defs":{"positive":{"type":"integer","minimum":1}}}}}});
    let format = StructuredOutput::parse(source.as_object().unwrap())
        .unwrap()
        .unwrap();
    for (answer, valid) in [
        ("{\"items\":[1,2]}", true),
        ("{\"items\":[0]}", false),
        ("{\"items\":[]}", true),
        ("{}", false),
        ("```json\n{}\n```", false),
    ] {
        let response =
            json!({"output":[{"type":"message","content":[{"type":"output_text","text":answer}]}]});
        assert_eq!(format.validate(&response).is_ok(), valid);
    }
    for reference in ["https://example.invalid/schema", "file:///etc/passwd"] {
        let source = json!({"text":{"format":{"type":"json_schema","name":"external","schema":{"$ref":reference}}}});
        assert!(StructuredOutput::parse(source.as_object().unwrap()).is_err());
    }
    let format = StructuredOutput::parse(
        json!({"text":{"format":{"type":"json_object"}}})
            .as_object()
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(format.validate(&json!({"output":[{"type":"message","content":[{"type":"output_text","text":"[]"}]}]})).is_err());
    assert!(format.validate(&json!({"output":[{"type":"message","content":[{"type":"refusal","refusal":"No"}]}]})).is_ok());
}

async fn transformed_fixture(
    source: Value,
    events: Vec<Value>,
) -> Result<Vec<Value>, crate::transport::CodexClientError> {
    let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
    let structured = StructuredOutput::parse(source.as_object().unwrap()).unwrap();
    let request = ExcelPreparedRequest {
        body: Default::default(),
        tools,
        structured,
        _image_lease: None,
        completed: Default::default(),
        usage: Default::default(),
        replay: None,
        endpoint: RESPONSES_URL.into(),
    };
    let bytes = events.into_iter().map(|event| {
        Ok(bytes::Bytes::from(format!(
            "event: {}\ndata: {}\n\n",
            event["type"].as_str().unwrap(),
            event
        )))
    });
    let result = transform_stream(Box::pin(futures::stream::iter(bytes)), &request)
        .try_collect::<Vec<_>>()
        .await?;
    let mut decoder = gateway_protocol::openai::sse::SseEventDecoder::default();
    Ok(decoder
        .push(&result.concat())
        .unwrap()
        .into_iter()
        .map(|event| serde_json::from_str(&event.data).unwrap())
        .collect())
}

#[tokio::test]
async fn excel_structured_stream_only_publishes_validated_terminal_text() {
    let source = json!({"text":{"format":{"type":"json_object"}}});
    let answer = json!({"type":"message","id":"msg_fixture","role":"assistant","status":"completed","content":[{"type":"output_text","text":"{\"answer\":21}"}]});
    let events = vec![
        json!({"type":"response.created","response":{"id":"resp_fixture","status":"in_progress",
            "output":[{"type":"message","content":[{"type":"output_text","text":"UNVALIDATED"}]}]}}),
        json!({"type":"response.output_text.delta","delta":"UNVALIDATED"}),
        json!({"type":"response.completed","response":{"id":"resp_fixture","status":"completed","output":[answer]}}),
    ];
    let output = transformed_fixture(source.clone(), events).await.unwrap();
    assert!(
        !serde_json::to_string(&output)
            .unwrap()
            .contains("UNVALIDATED")
    );
    assert_eq!(
        output.last().unwrap()["response"]["text"]["format"]["type"],
        "json_object"
    );
    assert!(
        output
            .iter()
            .any(|event| event["type"] == "response.output_text.delta"
                && event["delta"] == "{\"answer\":21}")
    );
    for (index, event) in output.iter().enumerate() {
        assert_eq!(event["sequence_number"], index);
    }
    let invalid = vec![
        json!({"type":"response.completed","response":{"id":"resp_fixture","status":"completed",
        "output":[{"type":"message","content":[{"type":"output_text","text":"not json"}]}]}}),
    ];
    assert!(transformed_fixture(source, invalid).await.is_err());
    let incomplete = transformed_fixture(
        json!({"text":{"format":{"type":"json_object"}}}),
        vec![json!({"type":"response.incomplete","response":{
            "id":"resp_fixture","status":"incomplete",
            "output":[{"type":"message","content":[{"type":"output_text","text":"UNVALIDATED"}]}]
        }})],
    )
    .await
    .unwrap();
    assert!(
        !serde_json::to_string(&incomplete)
            .unwrap()
            .contains("UNVALIDATED")
    );
}

#[tokio::test]
async fn excel_terminal_cannot_omit_a_reported_tool() {
    let events = vec![
        json!({"type":"response.output_item.done","item":native_fixture(json!({"name":"read","arguments":{"path":"x"}}))}),
        json!({"type":"response.completed","response":{"id":"resp_fixture","status":"completed","output":[]}}),
    ];
    assert!(transformed_fixture(json!({}), events).await.is_err());
}

#[tokio::test]
async fn excel_complete_history_recovers_without_cache_but_output_alone_does_not() {
    let store: Arc<dyn ProviderReplayPort> = Arc::new(MemoryReplay::default());
    let call = json!({"type":"function_call","name":"read","call_id":"same-call","arguments":"{\"path\":\"one\"}"});
    let result = json!({"type":"function_call_output","call_id":"same-call","output":"contents"});
    let restored = replay::restore(
        store.clone(),
        "owner".into(),
        "thread".into(),
        None,
        &json!([call, result]),
    )
    .await
    .unwrap();
    assert_eq!(restored.native_calls["same-call"]["name"], "run_officejs");
    assert!(
        replay::restore(
            store,
            "owner".into(),
            "thread".into(),
            None,
            &json!([result])
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn excel_downstream_websocket_uses_only_http_with_clean_headers_and_no_codex_cookies() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RESPONSES_PATH))
        .respond_with(completed())
        .expect(1)
        .mount(&server)
        .await;
    let mut request = request(format!("{}{RESPONSES_PATH}", server.uri()), json!("hello"));
    request.turn_state = Some("client-state".into());
    request
        .excel
        .as_mut()
        .unwrap()
        .body
        .insert("extra_fixture".into(), "x".repeat(2048).into());
    let mut context = CodexRequestContext::auxiliary(
        "Bearer fixture",
        Some("workspace-a"),
        "request-a",
        Some("device-a"),
    );
    context.cookie_header = Some("codex_cookie=fixture");
    assert_eq!(
        transport_requirement(&request),
        TransportRequirement::HttpRequired
    );
    let result = client(&format!("{}/codex", server.uri()))
        .create_response_stream_with_pool_account(&request, context, Some("account-a"))
        .await
        .unwrap();
    assert_eq!(result.transport, CodexBackendTransport::HttpSse);
    assert!(result.set_cookie_headers.is_empty());
    assert!(result.turn_state.is_none());
    assert!(
        !result
            .response_metadata
            .client_headers
            .iter()
            .any(|(name, _)| name == "x-codex-turn-state")
    );
    let bytes = result.body.try_collect::<Vec<_>>().await.unwrap();
    assert!(
        String::from_utf8(bytes.concat())
            .unwrap()
            .contains("response.completed")
    );
    let requests = server.received_requests().await.unwrap();
    let headers = &requests[0].headers;
    assert_eq!(headers["chatgpt-account-id"], "workspace-a");
    assert_eq!(headers["x-openai-account-id"], "workspace-a");
    for key in [
        "cookie",
        "x-codex-turn-state",
        "content-encoding",
        "upgrade",
        "openai-beta",
        "oai-device-id",
    ] {
        assert!(!headers.contains_key(key), "{key}");
    }
    assert_eq!(requests[0].body_json::<Value>().unwrap()["stream"], true);
    assert_eq!(
        requests[0].body_json::<Value>().unwrap()["model_selection"],
        "explicit"
    );
}

#[tokio::test]
async fn excel_denied_request_does_not_fall_back_or_open_codex_websocket() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RESPONSES_PATH))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"error":{"code":"basispoints_model_access_changed"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let request = request(format!("{}{RESPONSES_PATH}", server.uri()), json!("hello"));
    let result = client(&server.uri())
        .create_response_stream_with_pool_account(
            &request,
            CodexRequestContext::auxiliary("Bearer fixture", Some("workspace"), "req", None),
            Some("account"),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn excel_completed_tool_mapping_and_history_are_available_to_next_turn() {
    let server = MockServer::start().await;
    let native = json!({"type":"function_call","name":"run_officejs","id":"fc_native","call_id":"call_fixture",
        "arguments":json!({"code":json!({"name":"read","arguments":{}}).to_string()}).to_string()});
    let event = json!({"type":"response.completed","response":{"id":"resp_tool","status":"completed","output":[native]}});
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("event: response.completed\ndata: {event}\n\n")),
        )
        .mount(&server)
        .await;
    let store: Arc<dyn gateway_core::provider_ports::ProviderReplayPort> =
        Arc::new(MemoryReplay::default());
    let restored = replay::restore(
        store.clone(),
        "owner".into(),
        "thread".into(),
        None,
        &json!("read"),
    )
    .await
    .unwrap();
    let mut request = request(server.uri(), json!("read"));
    request.excel.as_mut().unwrap().tools = ClientTools::parse(
        json!({"tools":[{"type":"function","name":"read","parameters":{"type":"object"}}]})
            .as_object()
            .unwrap(),
    )
    .unwrap();
    request.excel.as_mut().unwrap().replay = Some(restored.capture);
    let result = client(&server.uri())
        .create_response_stream_with_pool_account(
            &request,
            CodexRequestContext::auxiliary("Bearer fixture", Some("workspace"), "req", None),
            Some("account"),
        )
        .await
        .unwrap();
    let bytes = result.body.try_collect::<Vec<_>>().await.unwrap();
    assert!(
        !String::from_utf8(bytes.concat())
            .unwrap()
            .contains("run_officejs")
    );
    let next = replay::restore(
        store,
        "owner".into(),
        "thread".into(),
        Some("resp_tool"),
        &json!([{"type":"function_call_output","call_id":"call_fixture","output":"ok"}]),
    )
    .await
    .unwrap();
    assert_eq!(next.input.len(), 3);
    assert_eq!(next.native_calls["call_fixture"]["id"], "fc_native");
}

#[tokio::test]
async fn excel_user_image_uploads_before_generation_without_changing_identity() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RESPONSES_PATH))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let image = &body["input"].as_array().unwrap().last().unwrap()["content"][0];
            if image.get("image_url").is_some() {
                ResponseTemplate::new(422)
                    .set_body_json(json!({"error":{"code":"inline_image_unsupported"}}))
            } else {
                assert_eq!(image["file_id"], "file_fixture");
                completed()
            }
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/basispoints/api/attachments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"openai_file_id":"file_fixture"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let image = json!({"type":"input_image","image_url":"data:image/png;base64,AQID"});
    let request = request(
        format!("{}{RESPONSES_PATH}", server.uri()),
        json!([{"role":"user","content":[image.clone(), image]}]),
    );
    let result = client(&server.uri())
        .create_response_stream_with_pool_account(
            &request,
            CodexRequestContext::auxiliary("Bearer fixture", Some("workspace"), "req", None),
            Some("account"),
        )
        .await
        .unwrap();
    result.body.try_collect::<Vec<_>>().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.headers["authorization"], "Bearer fixture");
        assert_eq!(request.headers["chatgpt-account-id"], "workspace");
        assert!(!request.headers.contains_key("cookie"));
    }
    assert!(
        requests[0].headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("multipart/form-data;")
    );
}

#[tokio::test]
async fn excel_generic_validation_errors_do_not_upload_or_replay() {
    for status in [400, 422] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(RESPONSES_PATH))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_json(json!({"detail":"Invalid request body."})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/basispoints/api/attachments"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let request = request(
            format!("{}{RESPONSES_PATH}", server.uri()),
            json!([{"role":"user","content":[{"type":"input_image","image_url":"https://images.example.com/image.png"}]}]),
        );
        let result = client(&server.uri())
            .create_response_stream_with_pool_account(
                &request,
                CodexRequestContext::auxiliary("Bearer fixture", Some("workspace"), "req", None),
                Some("account"),
            )
            .await;
        assert!(
            matches!(result, Err(crate::transport::CodexClientError::Upstream {status: actual, ..}) if actual.as_u16() == status)
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn excel_image_upload_auth_failure_is_not_hidden_or_retried_without_image() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RESPONSES_PATH))
        .respond_with(
            ResponseTemplate::new(422)
                .set_body_json(json!({"error":{"code":"inline_image_unsupported"}})),
        )
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/basispoints/api/attachments"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({"error":{"code":"token_revoked"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let request = request(
        format!("{}{RESPONSES_PATH}", server.uri()),
        json!([{"role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,AQID"}]}]),
    );
    let result = client(&server.uri())
        .create_response_stream_with_pool_account(
            &request,
            CodexRequestContext::auxiliary("Bearer fixture", Some("workspace"), "req", None),
            Some("account"),
        )
        .await;
    assert!(
        matches!(result, Err(crate::transport::CodexClientError::Upstream {status, ..}) if status.as_u16() == 401)
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
