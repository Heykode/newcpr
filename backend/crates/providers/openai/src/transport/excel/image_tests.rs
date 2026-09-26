use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures::{TryStreamExt, future::BoxFuture};
use gateway_core::{
    account::OpaqueProviderData,
    provider_ports::{ProviderReplayPort, ProviderStoreError, TemporaryImageSource},
};
use serde_json::{Map, Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

use super::{
    ClientTools, RESPONSES_PATH,
    image_relay::ImageRelay,
    images, prepare_request, replay,
    tests::{MemoryReplay, VERIFIED_MODEL, client, completed, request},
};
use crate::transport::{
    CodexClientError, CodexRequestContext, protocol::responses::CodexResponsesRequest,
};

const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";

fn image() -> Value {
    json!({"type":"input_image","image_url":PNG,"detail":"high"})
}

fn body(input: Value) -> Map<String, Value> {
    json!({"input":input}).as_object().unwrap().clone()
}

#[derive(Default)]
struct Assets {
    replay: MemoryReplay,
    entries: Mutex<BTreeMap<String, OpaqueProviderData>>,
}

impl ProviderReplayPort for Assets {
    fn read<'a>(
        &'a self,
        key: &'a str,
    ) -> BoxFuture<'a, Result<Option<OpaqueProviderData>, ProviderStoreError>> {
        self.replay.read(key)
    }
    fn write<'a>(
        &'a self,
        key: &'a str,
        value: &'a OpaqueProviderData,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
        self.replay.write(key, value)
    }
    fn read_asset<'a>(
        &'a self,
        key: &'a str,
    ) -> BoxFuture<'a, Result<Option<OpaqueProviderData>, ProviderStoreError>> {
        Box::pin(async move { Ok(self.entries.lock().unwrap().get(key).cloned()) })
    }
    fn store_asset<'a>(
        &'a self,
        key: &'a str,
        value: &'a OpaqueProviderData,
    ) -> BoxFuture<'a, Result<OpaqueProviderData, ProviderStoreError>> {
        Box::pin(async move {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .entry(key.into())
                .or_insert_with(|| value.clone())
                .clone())
        })
    }
    fn invalidate_asset<'a>(
        &'a self,
        key: &'a str,
        value: &'a OpaqueProviderData,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            let mut entries = self.entries.lock().unwrap();
            if entries.get(key) == Some(value) {
                entries.remove(key);
            }
            Ok(())
        })
    }
}

async fn scoped(
    server: &MockServer,
    store: Arc<dyn ProviderReplayPort>,
    owner: &str,
) -> CodexResponsesRequest {
    let input = json!([{"role":"user","content":[image()]}]);
    let mut req = request(format!("{}{RESPONSES_PATH}", server.uri()), input.clone());
    req.excel.as_mut().unwrap().replay = Some(
        replay::restore(
            store,
            owner.into(),
            "conversation".into(),
            None,
            &body(input),
        )
        .await
        .unwrap()
        .capture,
    );
    req
}

async fn uploader(server: &MockServer, field: &str, expected: u64) {
    Mock::given(path("/basispoints/api/attachments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({field:"file_fixture"})))
        .expect(expected)
        .mount(server)
        .await;
}

async fn run(server: &MockServer, request: &CodexResponsesRequest) -> Result<(), CodexClientError> {
    client(&server.uri())
        .create_response_stream_with_pool_account(
            request,
            CodexRequestContext::auxiliary("Bearer fixture", Some("workspace"), "req", None),
            Some("account"),
        )
        .await?
        .body
        .try_collect::<Vec<_>>()
        .await?;
    Ok(())
}

#[tokio::test]
async fn excel_encrypted_recovery_reuses_uploaded_attachments_without_reupload() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let server = MockServer::start().await;
    uploader(&server, "id", 1).await;
    let attempts = AtomicUsize::new(0);
    Mock::given(path(RESPONSES_PATH))
        .respond_with(move |_: &wiremock::Request| {
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(400).set_body_json(json!({
                    "error":{"code":"invalid_encrypted_content"}
                }))
            } else {
                completed()
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let original = json!([
        {"role":"user","content":[image()]},
        {"type":"reasoning","encrypted_content":"short-reference"}
    ]);
    let req = request(format!("{}{RESPONSES_PATH}", server.uri()), original);
    let prepared = req.excel.as_ref().unwrap().body.clone();
    run(&server, &req).await.unwrap();
    assert_eq!(req.excel.as_ref().unwrap().body, prepared);
    let requests = server.received_requests().await.unwrap();
    let generations: Vec<_> = requests
        .iter()
        .filter(|request| request.url.path() == RESPONSES_PATH)
        .map(|request| request.body_json::<Value>().unwrap())
        .collect();
    assert_eq!(generations.len(), 2);
    let mut expected = generations[0].clone();
    expected["input"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item["type"] != "reasoning");
    assert_eq!(generations[1], expected);
    let image_message = generations[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["role"] == "user")
        .unwrap();
    assert_eq!(image_message["content"][0]["file_id"], "file_fixture");
    assert_eq!(generations[0]["input"].as_array().unwrap().len(), 3);
    assert_eq!(generations[1]["input"].as_array().unwrap().len(), 2);
}

#[test]
fn excel_image_validation_checks_carrier_detail_and_private_errors() {
    let mut authenticated = url::Url::parse("https://images.example.com/a").unwrap();
    authenticated.set_username("PRIVATE").unwrap();
    authenticated.set_password(Some("SECRET")).unwrap();
    for invalid in [
        json!({"image_url":"data:image/png;base64,PRIVATE"}),
        json!({"image_url":"http://images.example.com/a"}),
        json!({"image_url":"/a"}),
        json!({"image_url":"file:///PRIVATE/a"}),
        json!({"image_url":"https:///a"}),
        json!({"image_url":authenticated.as_str()}),
        json!({"image_url":{"url":"https://images.example.com/a"}}),
        json!({"file_id":""}),
        json!({"file_id":22}),
        json!({"file_id":"file/PRIVATE"}),
        json!({"image_url":"https://images.example.com/a","file_id":"file_PRIVATE"}),
        json!({"image_url":"https://images.example.com/a","detail":"invalid"}),
        json!({"image_url":" https://images.example.com/a"}),
        json!({"image_url":PNG,"detail":"invalid"}),
        json!({"file_id":"file_fixture","detail":"invalid"}),
    ] {
        let mut invalid = invalid;
        invalid["type"] = "input_image".into();
        for item in [
            json!({"role":"user","content":[invalid.clone()]}),
            json!({"type":"function_call_output","call_id":"fixture","output":[invalid.clone()]}),
            json!({"type":"custom_tool_call_output","call_id":"fixture","output":[invalid.clone()]}),
        ] {
            let error = images::validate(&body(json!([item])))
                .unwrap_err()
                .to_string();
            assert!(!error.contains("PRIVATE"));
            assert!(!error.contains("SECRET"));
        }
    }
    for detail in [
        Value::Null,
        json!("auto"),
        json!("low"),
        json!("high"),
        json!("original"),
    ] {
        let source = body(json!([{"role":"user","content":[{"type":"input_image",
            "image_url":"https://images.example.com/a?signature=unchanged%2Fvalue&expires=123","detail":detail}]}]));
        let before = source.clone();
        images::validate(&source).unwrap();
        assert_eq!(source, before);
    }
}

#[test]
fn excel_original_detail_preserves_position_specific_image_carriers() {
    for image in [
        json!({"type":"input_image","image_url":PNG,"detail":"original"}),
        json!({"type":"input_image","image_url":"https://images.example.com/a","detail":"original"}),
    ] {
        for item in [
            json!({"role":"user","content":[image.clone()]}),
            json!({"type":"function_call_output","call_id":"fixture","output":[image.clone()]}),
        ] {
            let source = body(json!([item]));
            let before = source.clone();
            images::validate_references(&source).unwrap();
            images::validate(&source).unwrap();
            assert_eq!(source, before);
        }
    }
    images::validate(&body(json!([{"role":"user","content":[{
        "type":"input_image","file_id":"file_fixture","detail":"original"}]}])))
    .unwrap();
    assert!(
        images::validate(&body(json!([{"type":"function_call_output","output":[{
        "type":"input_image","file_id":"file_fixture","detail":"original"}]}])))
        .is_err()
    );
}

#[test]
fn excel_image_validation_bounds_inline_bytes_and_ignores_metadata() {
    for url in [
        "data:image/png;base64,".to_owned(),
        "data:image/png;base64,AQID".to_owned(),
        "data:image/png;base64,!!!!".to_owned(),
        PNG.replace("image/png", "image/jpeg"),
        format!(
            "data:image/png;base64,{}",
            "A".repeat(4 * 1024 * 1024 * 4 / 3 + 128)
        ),
    ] {
        for (kind, field) in [("message", "content"), ("function_call_output", "output")] {
            let source = body(json!([{"type":kind,"role":"user",field:[
                {"type":"input_image","image_url":url}]}]));
            assert!(images::validate(&source).is_err());
        }
    }
    let source = body(json!([
        {"role":"user","content":[{"type":"input_text","text":"fixture","metadata":image()}]},
        {"type":"function_call","arguments":{"image":image()},"metadata":image()}
    ]));
    assert!(!images::has_images(&source));
    assert!(!images::has_user_inline(&source));
    images::validate(&source).unwrap();
    assert_eq!(images::decoded_budget_user(&source["input"]).unwrap(), 0);
}

#[test]
fn excel_tool_only_images_do_not_use_relay_and_reference_checks_precede_staging() {
    let relay = Arc::new(ImageRelay::new(Some("https://images.example.com".into())));
    let mut source = body(json!([{"type":"function_call_output","output":[image()]}]));
    let original = source.clone();
    images::validate_references(&source).unwrap();
    assert!(relay.stage(&mut source).unwrap().is_none());
    images::validate(&source).unwrap();
    assert_eq!(source, original);
    let rejected = body(json!([
        {"role":"user","content":[image()]},
        {"type":"function_call_output","output":[{"type":"input_image","file_id":"file_fixture"}]}
    ]));
    assert!(images::validate_references(&rejected).is_err());
    for role in ["system", "developer", "assistant"] {
        assert!(images::validate(&body(json!([{"role":role,"content":[image()]}]))).is_err());
    }
}

#[test]
fn excel_repeated_user_images_share_budget_across_messages() {
    let source = body(json!([
        {"role":"user","content":[image()]},
        {"role":"user","content":[image()]},
        {"type":"function_call_output","output":[image()]}
    ]));
    let single = body(json!([{"role":"user","content":[image()]}]));
    assert_eq!(
        images::decoded_budget_user(&source["input"]).unwrap(),
        images::decoded_budget_user(&single["input"]).unwrap()
    );
    let mut pictures = Vec::new();
    images::collect_user(&source["input"], &mut pictures).unwrap();
    assert_eq!(pictures.len(), 1);
}

#[tokio::test]
async fn excel_tool_file_id_rejected_before_upload_or_generation() {
    let server = MockServer::start().await;
    for kind in ["function_call_output", "custom_tool_call_output"] {
        let mut req = request(
            format!("{}{RESPONSES_PATH}", server.uri()),
            json!("fixture"),
        );
        req.excel.as_mut().unwrap().body = body(json!([
            {"role":"user","content":[image()]},
            {"type":kind,"call_id":"fixture","output":[{"type":"input_image","file_id":"file_fixture"}]}
        ]));
        assert!(run(&server, &req).await.is_err());
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    let disabled = Arc::new(ImageRelay::new(None));
    let mut source = body(json!([{"role":"user","content":[image()]}]));
    let before = source.clone();
    assert!(disabled.stage(&mut source).unwrap().is_none());
    assert_eq!(source, before);
    assert!(disabled.stage(&mut body(json!("text"))).unwrap().is_none());
}

#[tokio::test]
async fn excel_mixed_tool_and_user_images_relay_without_mutating_history_or_identity() {
    let server = MockServer::start().await;
    Mock::given(path(RESPONSES_PATH))
        .respond_with(completed())
        .expect(1)
        .mount(&server)
        .await;
    let source = json!({"model":VERIFIED_MODEL,"input":[
        {"role":"user","content":[{"type":"input_text","text":"inspect"},image()]},
        {"type":"function_call","name":"screenshot","arguments":"{}","call_id":"call_fixture"},
        {"type":"function_call_output","call_id":"call_fixture","output":[{"type":"input_text","text":"screenshot"},image()]}
    ],"tools":[{"type":"function","name":"screenshot","parameters":{"type":"object"}}]});
    let store = Arc::new(MemoryReplay::default());
    let restored = replay::restore(
        store.clone(),
        "owner".into(),
        "conversation".into(),
        None,
        source.as_object().unwrap(),
    )
    .await
    .unwrap();
    let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
    let mut wire = prepare_request(
        source.as_object().unwrap(),
        &tools,
        &restored.native_calls,
        None,
    )
    .unwrap();
    let relay = Arc::new(ImageRelay::new(Some("https://images.example.com".into())));
    let lease = relay.stage(&mut wire).unwrap().unwrap();
    images::validate(&wire).unwrap();
    let items = wire["input"].as_array().unwrap();
    let user = items.iter().find(|item| item["role"] == "user").unwrap();
    let output = items
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap();
    let url = user["content"][1]["image_url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("https://images.example.com/_cpr/excel-images/"));
    assert_eq!(output["output"][1]["image_url"], PNG);
    assert_eq!(output["output"][1]["detail"], "high");
    assert!(output["output"][1].get("file_id").is_none());
    let token = url.rsplit('/').next().unwrap();
    assert!(relay.read(token).unwrap().bytes.starts_with(b"\x89PNG"));
    let mut req = request(
        format!("{}{RESPONSES_PATH}", server.uri()),
        json!("fixture"),
    );
    req.excel.as_mut().unwrap().body = wire;
    req.excel.as_mut().unwrap()._image_lease = Some(lease);
    run(&server, &req).await.unwrap();
    let calls = server.received_requests().await.unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].url.path(), RESPONSES_PATH);
    assert_eq!(calls[0].headers["authorization"], "Bearer fixture");
    assert_eq!(calls[0].headers["chatgpt-account-id"], "workspace");
    assert!(!calls[0].headers.contains_key("cookie"));
    assert!(!calls[0].headers.contains_key("x-codex-turn-state"));
    restored
        .capture
        .commit(
            &json!({"id":"resp_images","status":"completed","output":[]}),
            &tools,
        )
        .await
        .unwrap();
    drop(req);
    assert!(relay.read(token).is_none());
    let next = replay::restore(
        store,
        "owner".into(),
        "conversation".into(),
        Some("resp_images"),
        &body(json!([])),
    )
    .await
    .unwrap();
    assert_eq!(next.input[0]["content"][1]["image_url"], PNG);
    assert_eq!(next.input[2]["output"][1]["image_url"], PNG);
    let mut next_body = body(Value::Array(next.input));
    let _next_lease = relay.stage(&mut next_body).unwrap().unwrap();
    assert_ne!(next_body["input"][0]["content"][1]["image_url"], url);
    assert_eq!(next_body["input"][2]["output"][1]["image_url"], PNG);
}

#[tokio::test]
async fn excel_user_attachment_preserves_mixed_tool_images_and_original_history() {
    let server = MockServer::start().await;
    Mock::given(path("/basispoints/api/attachments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"openai_file_id":"file_fixture"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path(RESPONSES_PATH))
        .respond_with(completed())
        .expect(1)
        .mount(&server)
        .await;
    let input = json!([
        {"role":"user","content":[image(), image()]},
        {"type":"function_call","name":"screenshot","arguments":"{}","call_id":"call_fixture"},
        {"type":"function_call_output","call_id":"call_fixture","output":[image(),
            {"type":"input_image","image_url":"https://images.example.com/other.png?signature=unchanged"}]}
    ]);
    let source = json!({"model":VERIFIED_MODEL,"input":input,
        "tools":[{"type":"function","name":"screenshot","parameters":{"type":"object"}}]});
    let store = Arc::new(MemoryReplay::default());
    let restored = replay::restore(
        store.clone(),
        "owner".into(),
        "conversation".into(),
        None,
        source.as_object().unwrap(),
    )
    .await
    .unwrap();
    let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
    let wire = prepare_request(
        source.as_object().unwrap(),
        &tools,
        &restored.native_calls,
        None,
    )
    .unwrap();
    let mut req = request(
        format!("{}{RESPONSES_PATH}", server.uri()),
        json!("fixture"),
    );
    req.excel.as_mut().unwrap().body = wire.clone();
    req.excel.as_mut().unwrap().tools = tools;
    req.excel.as_mut().unwrap().replay = Some(restored.capture);
    run(&server, &req).await.unwrap();
    assert_eq!(req.excel.as_ref().unwrap().body, wire);
    let calls = server.received_requests().await.unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].url.path(), "/basispoints/api/attachments");
    let sent: Value = calls[1].body_json().unwrap();
    let items = sent["input"].as_array().unwrap();
    let user = items.iter().find(|item| item["role"] == "user").unwrap();
    assert_eq!(user["content"][0]["file_id"], "file_fixture");
    assert_eq!(user["content"][1]["file_id"], "file_fixture");
    assert!(user["content"][0].get("image_url").is_none());
    let output = items
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap();
    assert_eq!(output["output"], input[2]["output"]);
    for call in &calls {
        assert_eq!(call.headers["authorization"], "Bearer fixture");
        assert_eq!(call.headers["chatgpt-account-id"], "workspace");
        assert_eq!(call.headers["copilot-vision-request"], "true");
        assert!(!call.headers.contains_key("cookie"));
        assert!(!call.headers.contains_key("x-codex-turn-state"));
    }
    let next = replay::restore(
        store,
        "owner".into(),
        "conversation".into(),
        Some("resp_fixture"),
        &body(json!([])),
    )
    .await
    .unwrap();
    assert_eq!(next.input[0]["content"], input[0]["content"]);
    assert_eq!(next.input[2]["output"], input[2]["output"]);
}

#[tokio::test]
async fn excel_tool_inline_and_user_file_id_pass_without_attachment_calls() {
    let server = MockServer::start().await;
    Mock::given(path(RESPONSES_PATH))
        .respond_with(completed())
        .expect(2)
        .mount(&server)
        .await;
    for input in [
        json!([{"role":"user","content":[{"type":"input_image","file_id":"file_fixture","detail":"high"}]}]),
        json!([{"type":"function_call_output","call_id":"fixture","output":[image()]}]),
    ] {
        let mut req = request(
            format!("{}{RESPONSES_PATH}", server.uri()),
            json!("fixture"),
        );
        req.excel.as_mut().unwrap().body = body(input.clone());
        run(&server, &req).await.unwrap();
        let calls = server.received_requests().await.unwrap();
        let wire: Value = calls.last().unwrap().body_json().unwrap();
        assert_eq!(wire["input"], input);
        assert_eq!(
            calls.last().unwrap().headers["copilot-vision-request"],
            "true"
        );
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn excel_user_upload_failure_never_generates_or_falls_back() {
    for status in [401, 403, 429, 422] {
        let server = MockServer::start().await;
        Mock::given(path("/basispoints/api/attachments"))
            .respond_with(
                ResponseTemplate::new(status).set_body_json(json!({"error":{"code":"fixture"}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let req = request(
            format!("{}{RESPONSES_PATH}", server.uri()),
            json!([{"role":"user","content":[image()]}]),
        );
        let error = run(&server, &req).await.unwrap_err();
        assert!(
            matches!(error, CodexClientError::Upstream {status:actual,..} if actual.as_u16()==status)
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn excel_https_image_wire_preserves_urls_and_text_without_asset_calls() {
    let server = MockServer::start().await;
    Mock::given(path(RESPONSES_PATH))
        .respond_with(completed())
        .expect(2)
        .mount(&server)
        .await;
    for content in [
        json!({"type":"input_image","image_url":"https://images.example.com/a?signature=unchanged%2Fvalue","detail":"low"}),
        json!({"type":"input_text","text":"text"}),
    ] {
        let req = request(
            format!("{}{RESPONSES_PATH}", server.uri()),
            json!([{"role":"user","content":[content.clone()]}]),
        );
        run(&server, &req).await.unwrap();
        let calls = server.received_requests().await.unwrap();
        let wire: Value = calls.last().unwrap().body_json().unwrap();
        assert_eq!(
            wire["input"].as_array().unwrap().last().unwrap()["content"][0],
            content
        );
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn excel_images_reuse_file_ids_across_cold_handles_but_not_owners() {
    let server = MockServer::start().await;
    uploader(&server, "openai_file_id", 2).await;
    Mock::given(path(RESPONSES_PATH))
        .respond_with(completed())
        .expect(3)
        .mount(&server)
        .await;
    let store = Arc::new(Assets::default());
    for owner in ["owner-a", "owner-a", "owner-b"] {
        let req = scoped(&server, store.clone(), owner).await;
        run(&server, &req).await.unwrap();
    }
    assert_eq!(store.entries.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn excel_same_user_image_concurrency_uploads_once() {
    let server = MockServer::start().await;
    Mock::given(path("/basispoints/api/attachments"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"file_fixture"}))
                .set_delay(Duration::from_millis(40)),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path(RESPONSES_PATH))
        .respond_with(completed())
        .expect(8)
        .mount(&server)
        .await;
    let req = scoped(&server, Arc::new(Assets::default()), "owner").await;
    let results = futures::future::join_all((0..8).map(|_| run(&server, &req))).await;
    assert!(results.into_iter().all(|result| result.is_ok()));
}

#[tokio::test]
async fn excel_attachment_errors_invalidate_only_used_cache_without_replaying() {
    for code in [
        "file_not_found",
        "invalid_file_id",
        "attachment_expired",
        "unknown",
    ] {
        let server = MockServer::start().await;
        uploader(&server, "openai_file_id", 1).await;
        Mock::given(path(RESPONSES_PATH))
            .respond_with(
                ResponseTemplate::new(422).set_body_json(
                    json!({"error":{"code":code,"message":"Invalid request body."}}),
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
        let store = Arc::new(Assets::default());
        let req = scoped(&server, store.clone(), "owner").await;
        assert!(
            matches!(run(&server, &req).await, Err(CodexClientError::Upstream{status,..}) if status.as_u16()==422)
        );
        assert_eq!(
            store.entries.lock().unwrap().len(),
            usize::from(code == "unknown")
        );
    }
}

#[tokio::test]
async fn excel_invalid_attachment_ids_never_start_generation() {
    for value in [
        json!({"id":" "}),
        json!({"id":42}),
        json!({"file_id":"https://images.example.com/a"}),
        json!({"file_id":"x".repeat(513)}),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/basispoints/api/attachments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(value))
            .expect(1)
            .mount(&server)
            .await;
        let req = scoped(&server, Arc::new(Assets::default()), "owner").await;
        assert!(run(&server, &req).await.is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn excel_cancelled_upload_releases_key_and_keeps_no_cache_entry() {
    let server = MockServer::start().await;
    Mock::given(path("/basispoints/api/attachments"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"file_fixture"}))
                .set_delay(Duration::from_millis(200)),
        )
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path(RESPONSES_PATH))
        .respond_with(completed())
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(Assets::default());
    let req = scoped(&server, store.clone(), "owner").await;
    let transport = client(&server.uri());
    let mut future = Box::pin(async {
        transport
            .create_response_stream_with_pool_account(
                &req,
                CodexRequestContext::auxiliary("Bearer fixture", Some("workspace"), "cancel", None),
                Some("account"),
            )
            .await
    });
    // Cancel only after the mock has received the upload; do not race client setup.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            tokio::select! {
                _ = &mut future => panic!("upload completed before cancellation"),
                _ = tokio::time::sleep(Duration::from_millis(5)) => {
                    if !server.received_requests().await.unwrap().is_empty() { break; }
                }
            }
        }
    })
    .await
    .unwrap();
    drop(future);
    assert!(store.entries.lock().unwrap().is_empty());
    tokio::time::timeout(Duration::from_secs(3), run(&server, &req))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(store.entries.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn excel_user_upload_accepts_id_aliases_and_preserves_multipart_contract() {
    for field in ["openai_file_id", "file_id", "id"] {
        let server = MockServer::start().await;
        uploader(&server, field, 1).await;
        Mock::given(path(RESPONSES_PATH))
            .respond_with(completed())
            .expect(1)
            .mount(&server)
            .await;
        let req = scoped(&server, Arc::new(Assets::default()), "owner").await;
        run(&server, &req).await.unwrap();
        let calls = server.received_requests().await.unwrap();
        assert!(String::from_utf8_lossy(&calls[0].body).contains("name=\"purpose\"\r\n\r\nvision"));
        let wire: Value = calls[1].body_json().unwrap();
        assert_eq!(
            wire["input"].as_array().unwrap().last().unwrap()["content"][0]["file_id"],
            "file_fixture"
        );
    }
}
#[test]
fn configured_image_limits_cover_references_and_inline_content() {
    let limits = images::ImageLimits {
        single: 10,
        total: 20,
        count: 1,
    };
    let urls = serde_json::json!({"input":[{"role":"user","content":[
        {"type":"input_image","image_url":"https://example.com/image.png"},
        {"type":"input_image","file_id":"file_fixture"}]}]});
    assert!(images::validate_with_limits(urls.as_object().unwrap(), false, limits).is_err());
    let inline = serde_json::json!({"input":[{"role":"user","content":[
        {"type":"input_image","image_url":"data:image/png;base64,AQIDBAUGBwgJCgsM"}]}]});
    assert!(images::validate_with_limits(inline.as_object().unwrap(), false, limits).is_err());
    let larger = images::ImageLimits {
        single: 12,
        total: 12,
        count: 1,
    };
    assert!(images::validate_with_limits(inline.as_object().unwrap(), false, larger).is_ok());
    let tiny_total = images::ImageLimits {
        total: 11,
        ..larger
    };
    assert!(images::validate_with_limits(inline.as_object().unwrap(), false, tiny_total).is_err());
    // Raising byte limits does not skip validation of actual image content.
    assert!(images::validate_with_limits(inline.as_object().unwrap(), true, larger).is_err());
}
