//! HTTP 入口校验保留字段级错误；目标协议映射归 Provider 和 gateway-protocol。
use super::{
    chat_request,
    http::{Fixture, Session, json_body},
};
use axum::http::StatusCode;
use serde_json::json;
#[tokio::test]
async fn refuses_known_unrepresentable_features_before_starting_execution() {
    for (name, value) in [
        ("n", json!(2)),
        ("store", json!(true)),
        ("functions", json!([{"name":"old"}])),
        ("function_call", json!("auto")),
        ("input", json!("not chat")),
        ("previous_response_id", json!("resp_previous")),
        ("audio", json!({"voice":"alloy","format":"wav"})),
    ] {
        let fixture = Fixture::new(Session::new(Vec::new())).await;
        let mut body = chat_request(false);
        body[name] = value;
        let response = fixture.post(body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name}");
        let body = json_body(response).await;
        assert_eq!(body["error"]["param"], name, "{body}");
        assert!(
            fixture.execution.captured.lock().unwrap().is_none(),
            "{name}"
        );
    }
}

#[tokio::test]
async fn rejects_unmapped_message_content_and_identity_fields() {
    for message in [
        json!({"role":"tool","tool_call_id":"call-lookup","name":42,"content":"result"}),
        json!({"role":"assistant","content":[{"type":"file","file":{"filename":"a.pdf","file_data":"data:application/pdf;base64,JVBERg=="}}]}),
        json!({"role":"tool","content":"unassociated output"}),
    ] {
        let fixture = Fixture::new(Session::new(Vec::new())).await;
        let response = fixture
            .post(json!({"model":"model-a","messages":[message]}))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = json_body(response).await;
        assert!(
            body["error"]["param"]
                .as_str()
                .unwrap()
                .starts_with("messages")
        );
        assert!(fixture.execution.captured.lock().unwrap().is_none());
    }
}
