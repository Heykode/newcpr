//! Offline protocol checks: these do not assert real upstream model permissions.

use gateway_core::operation::{Feature, GenerateRequest, Operation, ProtocolPayload};
use gateway_protocol::openai::chat::{ChatStreamEncoder, decode_chat_request};
use provider_openai::encode_generate_request;
use provider_openai::transport::canonical::{CodexCanonicalDecoder, CodexCanonicalOutcome};
use provider_openai::transport::protocol::websocket::{
    websocket_event_to_sse_frame, websocket_response_create_payload_text,
};
use serde_json::{Value, json};

fn request(body: &Value) -> GenerateRequest {
    GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", body.as_object().expect("object").clone())
            .expect("payload"),
    )
}

#[test]
fn vision_input_survives_http_and_websocket_without_image_generation_tool() {
    for image_url in [
        "https://example.invalid/image.png",
        "data:image/png;base64,iVBORw0KGgo=",
    ] {
        let body = json!({
            "model": "vision-fixture",
            "input": [{"role": "user", "content": [
                {"type": "input_text", "text": "Describe the image."},
                {"type": "input_image", "image_url": image_url, "detail": "high"},
            ]}],
        });
        let generate = request(&body);
        let requirements = Operation::Generate(generate.clone()).capability_requirements();
        assert!(requirements.features().contains(&Feature::Vision));
        assert!(!requirements.features().contains(&Feature::Tools));
        assert!(!generate.image_generation_requested());

        let encoded = encode_generate_request(&generate, "vision-fixture", &Default::default())
            .expect("encode");
        assert_eq!(encoded.body().get("input"), body.get("input"));
        assert!(encoded.body().get("tools").is_none());
        let frame = websocket_response_create_payload_text(&encoded).expect("WS frame");
        let frame: Value = serde_json::from_str(&frame).expect("JSON");
        assert_eq!(frame["input"], body["input"]);
        assert_eq!(frame["type"], "response.create");
    }
}

#[test]
fn native_search_and_image_tools_survive_http_and_websocket_encoding() {
    let body = json!({
        "model": "fixture",
        "input": "Use the provided tools.",
        "tools": [
            {"type": "web_search", "search_context_size": "high"},
            {"type": "image_generation", "quality": "low"},
            {"type": "function", "name": "lookup", "parameters": {"type": "object"}},
        ],
        "tool_choice": "auto",
    });
    let encoded =
        encode_generate_request(&request(&body), "fixture", &Default::default()).expect("encode");
    let mut expected_tools = body["tools"].clone();
    expected_tools[0]["user_location"] = json!({
        "type": "approximate",
        "country": "US",
        "region": "Ohio",
        "city": "Piketon",
        "timezone": "America/New_York"
    });
    assert_eq!(encoded.body().get("tools"), Some(&expected_tools));
    let frame = websocket_response_create_payload_text(&encoded).expect("WS frame");
    let frame: Value = serde_json::from_str(&frame).expect("JSON");
    assert_eq!(frame["tools"], expected_tools);
    assert_eq!(frame["tool_choice"], body["tool_choice"]);
}

#[test]
fn websocket_tool_events_retain_image_result_and_search_action_in_sse() {
    let events = [
        json!({"type": "response.image_generation_call.partial_image",
            "partial_image_b64": "TEST_IMAGE_BYTES", "output_index": 0}),
        json!({"type": "response.output_item.done", "output_index": 0,
            "item": {"type": "image_generation_call", "id": "ig_1",
                "result": "TEST_IMAGE_BYTES", "status": "completed"}}),
        json!({"type": "response.output_item.done", "output_index": 1,
            "item": {"type": "web_search_call", "id": "ws_1", "status": "completed",
                "action": {"type": "search", "query": "fixture"}}}),
    ];
    for event in events {
        let raw = serde_json::to_string(&event).expect("JSON");
        let sse = websocket_event_to_sse_frame(&raw).expect("SSE event");
        let data = sse
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("data");
        assert_eq!(serde_json::from_str::<Value>(data).expect("JSON"), event);
    }
}

#[test]
fn chat_extensions_reach_existing_http_and_websocket_request_encoders() {
    let decoded = decode_chat_request(json!({
        "model":"fixture","stream":false,
        "messages":[
            {"role":"user","content":"draw and calculate"},
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"call_1","type":"function","function":{"name":"code","arguments":"print(1)"}}]},
            {"role":"tool","tool_call_id":"call_1","content":"1"}
        ],
        "tools":[
            {"type":"image_generation","output_format":"png","quality":"high"},
            {"type":"web_search","filters":{"allowed_domains":["example.test"]}},
            {"type":"custom","custom":{"name":"code","format":{"type":"grammar",
                "grammar":{"syntax":"regex","definition":"[a-z]+"}}}}
        ]
    }))
    .unwrap();
    let body = Value::Object(decoded.responses);
    let generate = request(&body);
    assert!(generate.image_generation_requested());
    let encoded = encode_generate_request(&generate, "fixture", &Default::default()).unwrap();
    let websocket: Value =
        serde_json::from_str(&websocket_response_create_payload_text(&encoded).unwrap()).unwrap();
    let mut expected_tools = body["tools"].clone();
    expected_tools[1]["user_location"] = json!({
        "type": "approximate",
        "country": "US",
        "region": "Ohio",
        "city": "Piketon",
        "timezone": "America/New_York"
    });
    for key in ["tools", "input"] {
        let expected = if key == "tools" {
            &expected_tools
        } else {
            &body[key]
        };
        assert_eq!(encoded.body().get(key), Some(expected));
        assert_eq!(websocket[key], *expected);
    }
    assert_eq!(websocket["type"], "response.create");
    assert_eq!(body["stream"], false);
}

#[test]
fn chat_extensions_survive_real_canonical_http_and_websocket_decoders() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../gateway-api/tests/openai/fixtures/newapi-chat-extensions.json"
    )))
    .unwrap();
    let mut response = fixture["image_response_source"].clone();
    response["output"].as_array_mut().unwrap().extend([
        json!({"type":"custom_tool_call","id":"ct_1","call_id":"call_1","name":"code","input":"print(1)"}),
        json!({"type":"web_search_call","id":"ws_1","status":"completed","action":{
            "type":"search","query":"fixture","sources":[{"type":"url","url":"https://example.test/"}]}})
    ]);
    for websocket in [false, true] {
        let mut canonical = CodexCanonicalDecoder::new("gpt-test").with_raw_sse_passthrough();
        let mut chat = ChatStreamEncoder::new(true);
        let mut chunks = Vec::new();
        let mut canonical_completed = false;
        let mut source_events = vec![
            json!({"type":"codex.response.metadata","headers":{"x-request-id":"fixture"}}),
            json!({"type":"response.created","response":{"id":"resp_test","model":"gpt-test","created_at":123}}),
        ];
        for (index, item) in response["output"].as_array().unwrap().iter().enumerate() {
            source_events.push(json!({
                "type":"response.output_item.done","output_index":index,"item":item
            }));
        }
        source_events.push(json!({"type":"response.completed","response":response}));
        for event in source_events {
            let frame = if websocket {
                websocket_event_to_sse_frame(&event.to_string()).unwrap()
            } else {
                format!(
                    "event: {}\ndata: {event}\n\n",
                    event["type"].as_str().unwrap()
                )
            };
            let events = match canonical.push(frame.as_bytes()) {
                CodexCanonicalOutcome::Events(events) => events,
                CodexCanonicalOutcome::Failed(failure) => {
                    panic!("canonical tool event: {failure:?}")
                }
            };
            for event in events {
                canonical_completed |= event
                    .canonical_facts()
                    .iter()
                    .any(|fact| matches!(fact, gateway_core::event::GatewayEvent::Completed(_)));
                if let Some(wire) = event.wire_event().filter(|wire| wire.has_json_data()) {
                    chunks.extend(chat.push(wire.event_type(), wire.data()).unwrap());
                }
            }
        }
        assert!(canonical_completed);
        assert!(chat.is_completed());
        let content: String = chunks
            .iter()
            .filter_map(|chunk| chunk["choices"][0]["delta"]["content"].as_str())
            .collect();
        assert!(content.is_empty());
        assert!(chunks.iter().any(|chunk| {
            chunk["choices"][0]["delta"]["images"][0]["image_url"]["url"]
                .as_str()
                .is_some_and(|url| url.starts_with("data:image/png;base64,"))
        }));
        assert!(chunks.iter().any(
            |chunk| chunk["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]
                == "print(1)"
        ));
        assert_eq!(
            chunks
                .iter()
                .filter(|chunk| chunk["usage"]["total_tokens"] == 28)
                .count(),
            1
        );
    }
}
