use bytes::Bytes;
use futures::{StreamExt, TryStreamExt, stream};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

use super::{
    RESPONSES_PATH,
    recovery::{MAX_BODY_BYTES, MAX_LINE_BYTES, verify_stream},
    tests::{client, request},
};
use crate::transport::{
    CodexBackendSseStream, CodexClientError, CodexRequestContext,
    protocol::responses::CodexResponsesRequest,
};

const NONCE: &str = "fixture-random-nonce";

fn message() -> Value {
    json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":NONCE}]})
}

fn completion(output: Value) -> String {
    format!(
        "data: {}\n\n",
        json!({"type":"response.completed","response":{"id":"resp_recovery","status":"completed","model":"fixture-model","output":output}})
    )
}

async fn verify(wire: &str, chunk_size: usize) -> Result<Vec<Bytes>, CodexClientError> {
    let chunks: Vec<_> = wire
        .as_bytes()
        .chunks(chunk_size)
        .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
        .collect();
    verify_stream(Box::pin(stream::iter(chunks)), NONCE.into())
        .try_collect()
        .await
}

#[tokio::test]
async fn recovery_accepts_final_exact_text_reasoning_and_arbitrary_chunks() {
    let output = json!([
        {"type":"reasoning","encrypted_content":"fixture-not-returned"},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"  fixture-"}]},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"random-nonce\n"}]}
    ]);
    for wire in [
        completion(output.clone()),
        format!(
            ": heartbeat\r\n{}data: [DONE]\r\n\r\n",
            completion(output).replace('\n', "\r\n")
        ),
    ] {
        for size in [1, 7, wire.len()] {
            let chunks = verify(&wire, size).await.unwrap();
            assert_eq!(chunks.concat(), wire.as_bytes());
        }
    }
}

#[tokio::test]
async fn recovery_rejects_correct_nonce_with_tools_or_non_text_output() {
    for extra in [
        json!({"type":"function_call","name":"tool","arguments":"{}"}),
        json!({"type":"image_generation_call","result":"fixture"}),
        json!({"type":"message","role":"user","content":[{"type":"output_text","text":""}]}),
        json!({"type":"message","role":"assistant","content":[{"type":"refusal","refusal":"fixture"}]}),
        json!({"type":"message","role":"assistant","content":[{"type":"output_image","image_url":"fixture"}]}),
        json!({"type":"unknown"}),
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":123}]}),
    ] {
        assert!(
            verify(&completion(json!([message(), extra])), 13)
                .await
                .is_err(),
            "{extra}"
        );
    }
    for output in [
        json!([]),
        json!([{"type":"reasoning"}]),
        json!([{"type":"message","role":"assistant","content":[]}]),
        json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"wrong"}]}]),
    ] {
        assert!(verify(&completion(output), 11).await.is_err());
    }
}

#[tokio::test]
async fn recovery_requires_one_completed_response_and_checks_the_tail() {
    let good = completion(json!([message()]));
    let delta = format!(
        "data: {}\n\n",
        json!({"type":"response.output_text.delta","delta":NONCE})
    );
    for wire in [
        String::new(),
        delta,
        "data: [DONE]\n\n".into(),
        good.replace("\"status\":\"completed\"", "\"status\":\"incomplete\""),
        "data: {\"type\":\"response.completed\"}\n\n".into(),
        format!("{good}{good}"),
        format!("{good}data: {{\"type\":\"response.failed\"}}\n\n"),
        format!("{good}data: {{\"type\":\"response.incomplete\"}}\n\n"),
        format!("{good}data: {{broken\n\n"),
        format!("{good}data: []\n\n"),
    ] {
        assert!(
            verify(&wire, 17).await.is_err(),
            "must reject invalid fixture"
        );
    }
}

#[tokio::test]
async fn recovery_never_releases_completion_before_eof_or_after_transport_failure() {
    let good = Bytes::from(completion(json!([message()])));
    let chunks = vec![
        Ok(good.clone()),
        Err(CodexClientError::InvalidSse(
            gateway_protocol::openai::sse::SseError::ParseError("fixture transport failure".into()),
        )),
    ];
    let mut verified = verify_stream(Box::pin(stream::iter(chunks)), NONCE.into());
    assert!(verified.next().await.unwrap().is_err());
    assert!(verified.next().await.is_none());

    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    let source: CodexBackendSseStream = Box::pin(async_stream::try_stream! {
        yield good;
        let _ = rx.await;
    });
    let mut verified = verify_stream(source, NONCE.into());
    assert!(futures::poll!(verified.next()).is_pending());
    tx.send(()).unwrap();
    assert!(verified.next().await.unwrap().is_ok());
}

#[tokio::test]
async fn recovery_enforces_reference_body_and_line_limits() {
    let good = completion(json!([message()]));
    let mut at_limit = ":\n".repeat((MAX_BODY_BYTES - good.len()) / 2);
    if (MAX_BODY_BYTES - good.len()) % 2 == 1 {
        at_limit.push('\n');
    }
    at_limit.push_str(&good);
    assert_eq!(at_limit.len(), MAX_BODY_BYTES);
    assert!(verify(&at_limit, 4096).await.is_ok());
    at_limit.push('\n');
    assert!(verify(&at_limit, 4096).await.is_err());
    for (length, valid) in [(MAX_LINE_BYTES - 1, true), (MAX_LINE_BYTES, false)] {
        let wire = format!("{}\n{good}", ":".repeat(length));
        assert_eq!(verify(&wire, 8192).await.is_ok(), valid);
    }
}

#[tokio::test]
async fn recovery_http_gate_checks_raw_tail_and_status_without_changing_ordinary_excel() {
    let good = completion(json!([message()]));
    for (status, wire, expected) in [
        (200, good.clone(), true),
        (200, format!("{good}{good}"), false),
        (
            200,
            completion(json!([message(), {"type":"function_call","name":"tool"}])),
            false,
        ),
        (
            200,
            format!("{good}data: {{\"type\":\"response.failed\"}}\n\n"),
            false,
        ),
        (201, good.clone(), false),
    ] {
        for recovery in [false, true] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(RESPONSES_PATH))
                .respond_with(
                    ResponseTemplate::new(status)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(wire.clone()),
                )
                .expect(1)
                .mount(&server)
                .await;
            let mut req = request(
                format!("{}{RESPONSES_PATH}", server.uri()),
                json!("fixture"),
            );
            req.excel_recovery_nonce = recovery.then(|| NONCE.into());
            let response = client(&server.uri())
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
                .await;
            let result = match response {
                Ok(response) => response.body.try_collect::<Vec<_>>().await,
                Err(error) => Err(error),
            };
            if recovery {
                assert_eq!(result.is_ok(), expected, "status={status}: {result:?}");
            } else if !wire.contains("function_call") {
                assert!(
                    result.is_ok(),
                    "ordinary Excel adapter remains unchanged: {result:?}"
                );
            }
            server.verify().await;
        }
    }
}

#[test]
fn recovery_expectation_cannot_be_forged_by_wire_json_or_leak_to_upstream() {
    let mut request = CodexResponsesRequest::from_body(
        json!({"model":"fixture", "excel_recovery_nonce":"forged"})
            .as_object()
            .unwrap()
            .clone(),
    );
    assert!(request.excel_recovery_nonce.is_none());
    request.body_mut().remove("excel_recovery_nonce");
    request.excel_recovery_nonce = Some(NONCE.into());
    assert!(!serde_json::to_string(&request).unwrap().contains(NONCE));
}
