use super::*;

#[test]
fn capacity_projection_preserves_upstream_facts_and_sse_metadata() {
    for code in ["server_is_overloaded", "slow_down"] {
        for kind in ["error", "response.failed"] {
            for explicit_type in [false, true] {
                let error = json!({
                    "code": code, "type": "service_unavailable_error",
                    "message": "synthetic capacity failure", "future": {"keep": true}
                });
                let data = if kind == "error" {
                    json!({"type": kind, "error": error, "sequence_number": 2})
                } else {
                    json!({"type": kind, "response": {
                        "id": "resp_capacity", "status": "failed", "error": error
                    }, "sequence_number": 2})
                };
                let raw = Bytes::from(format!(
                    "id: evt_capacity\r\nretry: 2000\r\ndata: {data}\r\n\r\n"
                ));
                let event = ProviderEvent::wire(
                    ProtocolWireEvent::json_with_raw_sse_metadata(
                        "openai",
                        explicit_type.then(|| kind.to_owned()),
                        data.clone(),
                        raw,
                        Some("evt_capacity".to_owned()),
                        Some(2000),
                    )
                    .unwrap(),
                );
                let mut sse = OpenAiResponsesEncoder::new();
                let frames = sse.push_sse(&event);
                let parsed = parse_sse_events(std::str::from_utf8(&frames[0]).unwrap()).unwrap();
                assert_eq!(parsed[0].id.as_deref(), Some("evt_capacity"));
                assert_eq!(parsed[0].retry, Some(2000));
                let result: Value = serde_json::from_str(&parsed[0].data).unwrap();
                assert_eq!(result["type"], "response.failed");
                assert_eq!(result["response"]["error"]["code"], "server_error");
                assert_eq!(result["response"]["error"]["future"], error["future"]);
                assert_eq!(result["response"]["error"]["message"], error["message"]);
                assert_eq!(result["sequence_number"], 2);
                assert!(sse.has_wire_failure());
                assert!(!sse.is_completed());
                let mut ws = OpenAiResponsesEncoder::new();
                let result: Value = serde_json::from_str(&ws.push_websocket(&event)[0]).unwrap();
                assert_eq!(result["response"]["error"]["code"], "server_error");
                assert_eq!(event.wire_event().unwrap().data(), &data);
            }
        }
    }
}

#[test]
fn websocket_capacity_wrappers_are_retryable_but_control_errors_are_unchanged() {
    for code in [
        "server_is_overloaded",
        "slow_down",
        "rate_limit_exceeded",
        "previous_response_not_found",
        "websocket_connection_limit_reached",
        "invalid_request",
    ] {
        for status_field in ["status", "status_code"] {
            let data = json!({
                "type": "error", status_field: 429,
                "error": {"code": code, "message": "synthetic"},
                "headers": {"x-request-id": "req_capacity", "retry-after": "2"}
            });
            let event = openai_wire_event(vec![], "error", data.clone());
            let mut encoder = OpenAiResponsesEncoder::new();
            let result: Value = serde_json::from_str(&encoder.push_websocket(&event)[0]).unwrap();
            let mut expected = data.clone();
            if matches!(code, "server_is_overloaded" | "slow_down") {
                expected[status_field] = json!(503);
                expected["error"]["code"] = json!("server_error");
            }
            assert_eq!(result, expected);
            assert_eq!(event.wire_event().unwrap().data(), &data);
        }
    }
}

#[test]
fn capacity_like_fields_in_success_events_are_not_projected() {
    let data = json!({"type":"response.completed","response":{
        "id":"resp_success","status":"completed",
        "error":{"code":"slow_down"}
    }});
    let raw = Bytes::from(format!("event: response.completed\r\ndata: {data}\r\n\r\n"));
    let event = ProviderEvent::wire(
        ProtocolWireEvent::json_with_raw_sse_metadata(
            "openai",
            Some("response.completed".to_owned()),
            data.clone(),
            raw.clone(),
            None,
            None,
        )
        .unwrap(),
    );
    let mut encoder = OpenAiResponsesEncoder::new();
    assert_eq!(encoder.push_sse(&event), [raw]);
    assert_eq!(encoder.push_websocket(&event), [data.to_string()]);
}
