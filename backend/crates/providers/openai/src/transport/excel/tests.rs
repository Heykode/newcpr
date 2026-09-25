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

fn request(endpoint: String, input: Value) -> CodexResponsesRequest {
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
        replay: None,
        endpoint,
    });
    request
}

fn client(base: &str) -> CodexBackendClient {
    CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        base,
        OpenAiConfig::default().wire_profile_state(),
    )
}

fn completed() -> ResponseTemplate {
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

#[test]
fn excel_tool_formatting_aliases_and_raw_custom_preserve_values() {
    let tools = fixture_tools();
    for code in [
        json!({"tool":"read","args":{"path":"sample.txt"}}),
        json!("{\"name\":\"read\",\"arguments\":{\"path\":\"sample.txt\"}}"),
        json!("```json\n{\"name\":\"read\",\"arguments\":{\"path\":\"sample.txt\"}}\n```"),
        json!("Tool: {\"name\":\"read\",\"arguments\":{\"path\":\"sample.txt\"}}"),
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
    ] {
        assert!(tools.convert_call(&native_fixture(code)).is_err());
    }
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
async fn excel_image_fallback_uploads_once_without_changing_identity() {
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
        .expect(2)
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
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(request.headers["authorization"], "Bearer fixture");
        assert_eq!(request.headers["chatgpt-account-id"], "workspace");
        assert!(!request.headers.contains_key("cookie"));
    }
    assert!(
        requests[1].headers["content-type"]
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
        .expect(1)
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
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}
