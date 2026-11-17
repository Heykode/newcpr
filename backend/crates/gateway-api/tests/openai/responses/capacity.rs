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
                expected["headers"] = json!({"x-request-id": "req_capacity"});
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

#[test]
fn capacity_failure_projection_preserves_wire_metadata_and_original_events() {
    for code in ["server_is_overloaded", "slow_down"] {
        for (event_type, explicit_type) in [
            ("error", true),
            ("error", false),
            ("response.failed", true),
            ("response.failed", false),
        ] {
            for status in [None, Some(400), Some(429), Some(503)] {
                let error = json!({"code": code, "type": "service_unavailable_error", "message": "busy", "future": 42,
                    "headers":{"retry-after":"309", "Retry-After-Ms":"309000", "x-request-id":"req_capacity"}});
                let mut original = if event_type == "error" {
                    json!({"type": event_type, "error": error, "sequence_number": 3})
                } else {
                    json!({"type": event_type, "response": {"id": "resp_capacity", "status": "failed", "error": error}, "sequence_number": 3})
                };
                if let Some(status) = status {
                    original["status_code"] = json!(status);
                }
                let raw = Bytes::from(format!(
                    "id: capacity\r\nretry: 1700\r\nevent: {event_type}\r\ndata: {original}\r\n\r\n"
                ));
                let event = ProviderEvent::wire(
                    ProtocolWireEvent::json_with_raw_sse_metadata(
                        "openai",
                        explicit_type.then(|| event_type.to_owned()),
                        original.clone(),
                        raw.clone(),
                        Some("capacity".to_owned()),
                        Some(1700),
                    )
                    .expect("wire"),
                );
                let mut sse = OpenAiResponsesEncoder::new();
                let mut ws = OpenAiResponsesEncoder::new();
                let created = openai_wire_event(
                    Vec::new(),
                    "response.created",
                    json!({"type": "response.created", "response": {"id": "resp_capacity", "status": "in_progress"}}),
                );
                sse.push_sse(&created);
                ws.push_websocket(&created);
                let frames = sse.push_sse(&event);
                let parsed =
                    parse_sse_events(std::str::from_utf8(&frames[0]).expect("UTF-8")).expect("SSE");
                let sse_data: Value = serde_json::from_str(&parsed[0].data).expect("JSON");
                assert_eq!(parsed[0].id.as_deref(), Some("capacity"));
                assert_eq!(parsed[0].retry, Some(1700));
                assert_eq!(parsed[0].event.as_deref(), Some("response.failed"));
                assert_eq!(sse_data["response"]["error"]["code"], "server_error");
                assert_eq!(sse_data["response"]["error"]["message"], "busy");
                assert_eq!(sse_data["response"]["error"]["future"], 42);
                assert_eq!(
                    sse_data["response"]["error"]["headers"],
                    json!({"x-request-id":"req_capacity"})
                );
                let messages = ws.push_websocket(&event);
                let ws_data: Value = serde_json::from_str(&messages[0]).expect("JSON");
                if event_type == "error" && status.is_some() {
                    let mut expected = original.clone();
                    expected["error"]["code"] = json!("server_error");
                    expected["error"]["headers"] = json!({"x-request-id":"req_capacity"});
                    expected["status_code"] = json!(503);
                    assert_eq!(ws_data, expected);
                } else {
                    assert_eq!(ws_data, sse_data);
                }
                assert!(sse.has_wire_failure());
                assert!(ws.has_wire_failure());
                let wire = event.wire_event().expect("original wire");
                assert_eq!(wire.data(), &original);
                assert_eq!(wire.raw_sse_frame(), Some(&raw));
            }
        }
    }
}
