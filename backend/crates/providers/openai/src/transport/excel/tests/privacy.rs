//! 隐私规则在 Excel 最终 HTTP JSON、SSE 和 multipart 文本字段的行为。
use super::*;
use gateway_core::settings::privacy::CodexPrivacyPolicy;

fn privacy_client(base: &str, selector: &str, pattern: &str) -> CodexBackendClient {
    let mut client = client(base);
    let policy: CodexPrivacyPolicy = serde_json::from_value(json!({
        "enabled":true,"onError":"reject_request","rules":[{
            "id":"fixture","name":"fixture","enabled":true,"steps":[
                {"scope":"request_header","selector":"authorization","action":"set_value","value":"Bearer rewritten-fixture","pattern":null,"replacement":"","replaceAll":true,"caseInsensitive":false,"multiLine":false},
                {"scope":"request_body","selector":selector,"action":"regex_replace","pattern":pattern,"replacement":format!("{pattern}{pattern}"),"replaceAll":true,"value":null,"caseInsensitive":false,"multiLine":false}
            ]
        }]
    })).unwrap();
    client.privacy = Some((
        crate::transport::privacy::compile(&policy).unwrap(),
        gateway_core::lifecycle::CancellationToken::new(),
        None,
    ));
    client
}

#[tokio::test]
async fn privacy_excel_sse_rewrites_each_send_once_without_mutating_replay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(completed())
        .expect(2)
        .mount(&server)
        .await;
    let mut request = request(format!("{}{RESPONSES_PATH}", server.uri()), json!("hello"));
    request
        .excel
        .as_mut()
        .unwrap()
        .body
        .insert("privacy_fixture".into(), json!("source"));
    let client = privacy_client(&server.uri(), "$.privacy_fixture", "source");
    for _ in 0..2 {
        client
            .create_response_stream_with_pool_account(
                &request,
                CodexRequestContext::auxiliary(
                    "Bearer original-fixture",
                    Some("workspace"),
                    "req",
                    None,
                ),
                Some("fixture-account"),
            )
            .await
            .unwrap()
            .body
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
    }
    assert_eq!(
        request.excel.as_ref().unwrap().body["privacy_fixture"],
        "source"
    );
    for sent in server.received_requests().await.unwrap() {
        assert_eq!(sent.headers["authorization"], "Bearer rewritten-fixture");
        assert_eq!(
            sent.body_json::<Value>().unwrap()["privacy_fixture"],
            "sourcesource"
        );
    }
}

#[tokio::test]
async fn privacy_excel_image_rewrites_json_and_multipart_text_fields() {
    use super::super::image_generation::PreparedImage;
    use crate::transport::{CODEX_IMAGE_EDITS_PATH, CODEX_IMAGE_GENERATIONS_PATH};
    let server = MockServer::start().await;
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"b64_json":png}]})))
        .expect(2)
        .mount(&server)
        .await;
    let client = privacy_client(&server.uri(), "$.prompt", "draw");
    for (path, body) in [
        (CODEX_IMAGE_GENERATIONS_PATH, json!({"prompt":"draw"})),
        (
            CODEX_IMAGE_EDITS_PATH,
            json!({"prompt":"draw","images":[{"image_url":format!("data:image/png;base64,{png}")}]}),
        ),
    ] {
        let mut image = PreparedImage::parse(body.to_string().as_bytes(), path).unwrap();
        image.endpoint = format!("{}/images", server.uri());
        client
            .post_excel_image(
                &image,
                CodexRequestContext::auxiliary(
                    "Bearer original-fixture",
                    Some("workspace"),
                    "req",
                    None,
                ),
            )
            .await
            .unwrap();
    }
    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent[0].headers["authorization"], "Bearer rewritten-fixture");
    assert_eq!(sent[0].body_json::<Value>().unwrap()["prompt"], "drawdraw");
    assert_eq!(sent[1].headers["authorization"], "Bearer rewritten-fixture");
    assert!(String::from_utf8_lossy(&sent[1].body).contains("name=\"prompt\"\r\n\r\ndrawdraw\r\n"));
    assert!(String::from_utf8_lossy(&sent[1].body).contains("name=\"image\""));
}
