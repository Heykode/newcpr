use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures::{TryStreamExt, future::BoxFuture};
use gateway_core::{
    account::OpaqueProviderData,
    provider_ports::{ProviderReplayPort, ProviderStoreError},
};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

use super::{
    RESPONSES_PATH, replay,
    tests::{MemoryReplay, client, completed, request},
};
use crate::transport::{
    CodexClientError, CodexRequestContext, protocol::responses::CodexResponsesRequest,
};

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

fn image(data: &str) -> Value {
    json!({"type":"input_image","image_url":format!("data:image/png;base64,{data}")})
}

fn input() -> Value {
    json!([{"role":"user","content":[image("AQID")]}])
}

async fn scoped(
    server: &MockServer,
    store: Arc<dyn ProviderReplayPort>,
    owner: &str,
    body: Value,
) -> CodexResponsesRequest {
    let mut request = request(format!("{}{RESPONSES_PATH}", server.uri()), body.clone());
    request.excel.as_mut().unwrap().replay = Some(
        replay::restore(store, owner.into(), "conversation".into(), None, &body)
            .await
            .unwrap()
            .capture,
    );
    request
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

async fn uploader(server: &MockServer, field: &str, expected: u64) {
    let payload = json!({field:"file_fixture"});
    Mock::given(method("POST"))
        .and(path("/basispoints/api/attachments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(payload))
        .expect(expected)
        .mount(server)
        .await;
}

#[tokio::test]
async fn excel_tool_images_upload_first_with_vision_and_purpose_and_all_id_aliases() {
    for field in ["openai_file_id", "file_id", "id"] {
        let server = MockServer::start().await;
        uploader(&server, field, 1).await;
        Mock::given(method("POST"))
            .and(path(RESPONSES_PATH))
            .respond_with(|request: &wiremock::Request| {
                let body: Value = request.body_json().unwrap();
                let last = body["input"].as_array().unwrap().last().unwrap();
                assert_eq!(last["output"][1]["file_id"], "file_fixture");
                assert!(last["output"][1].get("image_url").is_none());
                completed()
            })
            .expect(1)
            .mount(&server)
            .await;
        let mut req = request(format!("{}{RESPONSES_PATH}", server.uri()), json!("test"));
        req.excel.as_mut().unwrap().body["input"] = json!([{
            "type":"function_call_output","call_id":"fixture", "output":[
                {"type":"input_text","text":"screenshot"}, image("AQID")
            ]
        }]);
        run(&server, &req).await.unwrap();
        let calls = server.received_requests().await.unwrap();
        assert_eq!(calls[0].url.path(), "/basispoints/api/attachments");
        let multipart = String::from_utf8_lossy(&calls[0].body);
        assert!(multipart.contains("name=\"purpose\"\r\n\r\nvision"));
        for call in calls {
            assert_eq!(call.headers["copilot-vision-request"], "true");
            assert_eq!(call.headers["authorization"], "Bearer fixture");
            assert_eq!(call.headers["chatgpt-account-id"], "workspace");
            assert!(!call.headers.contains_key("cookie"));
            assert!(!call.headers.contains_key("x-codex-turn-state"));
        }
    }
}

#[tokio::test]
async fn excel_vision_header_covers_https_and_file_ids_but_not_text() {
    for (content, vision) in [
        (
            json!({"type":"input_image","image_url":"https://images.example.com/a.png"}),
            true,
        ),
        (json!({"type":"input_image","file_id":"file_fixture"}), true),
        (json!({"type":"input_text","text":"text"}), false),
    ] {
        let server = MockServer::start().await;
        Mock::given(path(RESPONSES_PATH))
            .respond_with(completed())
            .expect(1)
            .mount(&server)
            .await;
        let req = request(
            format!("{}{RESPONSES_PATH}", server.uri()),
            json!([{"role":"user","content":[content]}]),
        );
        run(&server, &req).await.unwrap();
        let calls = server.received_requests().await.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].headers.contains_key("copilot-vision-request"),
            vision
        );
    }
}

#[tokio::test]
async fn excel_images_reuse_file_ids_across_turns_and_cold_handles_but_not_owners() {
    let server = MockServer::start().await;
    uploader(&server, "openai_file_id", 2).await;
    Mock::given(path(RESPONSES_PATH))
        .respond_with(completed())
        .expect(3)
        .mount(&server)
        .await;
    let store = Arc::new(Assets::default());
    for owner in ["owner-a", "owner-a", "owner-b"] {
        let req = scoped(&server, store.clone(), owner, input()).await;
        // A new transport client and replay capture each time: persistence owns the hit.
        run(&server, &req).await.unwrap();
    }
    assert_eq!(store.entries.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn excel_same_image_concurrency_uploads_once() {
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
    let req = scoped(&server, Arc::new(Assets::default()), "owner", input()).await;
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
        let req = scoped(&server, store.clone(), "owner", input()).await;
        assert!(
            matches!(run(&server,&req).await,Err(CodexClientError::Upstream{status,..}) if status.as_u16()==422)
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
        Mock::given(path(RESPONSES_PATH))
            .respond_with(completed())
            .expect(0)
            .mount(&server)
            .await;
        let req = request(format!("{}{RESPONSES_PATH}", server.uri()), input());
        assert!(run(&server, &req).await.is_err());
    }
}

#[tokio::test]
async fn excel_cancelled_upload_releases_key_and_keeps_no_cache_entry() {
    let server = MockServer::start().await;
    Mock::given(path("/basispoints/api/attachments"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"file_fixture"}))
                .set_delay(Duration::from_millis(120)),
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
    let req = scoped(&server, store.clone(), "owner", input()).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), run(&server, &req))
            .await
            .is_err()
    );
    assert!(store.entries.lock().unwrap().is_empty());
    tokio::time::timeout(Duration::from_secs(3), run(&server, &req))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(store.entries.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn excel_upload_failures_are_not_cached_or_hidden() {
    let server = MockServer::start().await;
    Mock::given(path("/basispoints/api/attachments"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({"error":{"code":"token_expired"}})),
        )
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path(RESPONSES_PATH))
        .respond_with(completed())
        .expect(0)
        .mount(&server)
        .await;
    let store = Arc::new(Assets::default());
    let req = scoped(&server, store.clone(), "owner", input()).await;
    for _ in 0..2 {
        assert!(
            matches!(run(&server,&req).await,Err(CodexClientError::Upstream{status,..}) if status.as_u16()==401)
        );
    }
    assert!(store.entries.lock().unwrap().is_empty());
}
