//! Actual NewAPI relaykit fixtures, not host-adaptor or live transport assertions.

use gateway_protocol::openai::chat::{DecodedChatRequest, decode_chat_request};
use serde_json::{Value, json};

struct Fixture {
    chat: Value,
    reference: Value,
    decoded: DecodedChatRequest,
}

macro_rules! fixture {
    ($name:literal) => {{
        let chat: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../gateway-api/tests/openai/fixtures/newapi-",
            $name,
            ".json"
        )))
        .expect("NewAPI Chat fixture");
        let reference: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../gateway-api/tests/openai/fixtures/newapi-",
            $name,
            "-responses.json"
        )))
        .expect("NewAPI Responses reference");
        let decoded = decode_chat_request(chat.clone()).expect("supported NewAPI Chat fixture");
        assert_eq!(decoded.responses["model"], chat["model"]);
        assert_eq!(decoded.responses["model"], reference["model"]);
        assert_eq!(decoded.stream, chat["stream"].as_bool().unwrap_or(false));
        assert_eq!(
            decoded.include_usage,
            chat.pointer("/stream_options/include_usage")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        );
        assert!(!decoded.responses.contains_key("stream_options"));
        Fixture {
            chat,
            reference,
            decoded,
        }
    }};
}

fn typed_items(input: &Value, kind: &str) -> Vec<Value> {
    input
        .as_array()
        .expect("Responses input")
        .iter()
        .filter(|item| item["type"] == kind)
        .cloned()
        .collect()
}

fn text_part(item: &Value) -> &Value {
    &item["content"][0]["text"]
}

#[test]
fn newapi_chat_text_preserves_system_and_default_buffered_delivery() {
    let fixture = fixture!("chat-text");
    let input = &fixture.decoded.responses["input"];
    assert!(!fixture.decoded.stream);
    assert!(!fixture.decoded.include_usage);
    assert_eq!(input[0]["role"], "system");
    assert_eq!(text_part(&input[0]), &fixture.reference["instructions"]);
    assert_eq!(input[1]["role"], "user");
    assert_eq!(
        text_part(&input[1]),
        &fixture.reference["input"][0]["content"]
    );
    assert_eq!(input.as_array().unwrap().len(), 2);
}

#[test]
fn newapi_chat_stream_preserves_developer_parallel_and_usage_options() {
    let fixture = fixture!("chat-text-stream");
    let input = &fixture.decoded.responses["input"];
    assert!(fixture.decoded.stream && fixture.decoded.include_usage);
    assert_eq!(
        fixture.decoded.responses["stream"],
        fixture.reference["stream"]
    );
    assert_eq!(
        fixture.decoded.responses["parallel_tool_calls"],
        fixture.reference["parallel_tool_calls"]
    );
    assert_eq!(fixture.decoded.responses["parallel_tool_calls"], false);
    assert_eq!(input[0]["role"], "developer");
    assert_eq!(text_part(&input[0]), &fixture.reference["instructions"]);
    assert_eq!(
        text_part(&input[1]),
        &fixture.reference["input"][0]["content"]
    );
}

#[test]
fn newapi_chat_images_preserve_urls_and_original_details() {
    let fixture = fixture!("chat-image");
    let content = &fixture.decoded.responses["input"][0]["content"];
    assert_eq!(content.as_array().unwrap().len(), 3);
    assert_eq!(content[0], fixture.reference["input"][0]["content"][0]);
    for index in [1, 2] {
        let source = &fixture.chat["messages"][0]["content"][index]["image_url"];
        assert_eq!(content[index]["type"], "input_image");
        assert_eq!(content[index]["image_url"], source["url"]);
        assert_eq!(
            content[index]["image_url"],
            fixture.reference["input"][0]["content"][index]["image_url"]
        );
        // NewAPI's reference omits detail; the source contract remains authoritative.
        assert_eq!(content[index]["detail"], source["detail"]);
    }
    assert_eq!(content[1]["detail"], "low");
    assert_eq!(content[2]["detail"], "high");
}

#[test]
fn newapi_chat_function_preserves_parallel_calls_results_and_tool_names() {
    let fixture = fixture!("chat-function");
    let input = &fixture.decoded.responses["input"];
    for kind in ["function_call", "function_call_output"] {
        let actual = typed_items(input, kind);
        assert_eq!(actual.len(), 2);
        assert_eq!(actual, typed_items(&fixture.reference["input"], kind));
    }
    assert_eq!(
        fixture.decoded.responses["tools"],
        fixture.reference["tools"]
    );
    assert_eq!(
        fixture.decoded.responses["tool_choice"],
        fixture.reference["tool_choice"]
    );
    assert_eq!(fixture.decoded.responses["parallel_tool_calls"], true);
    for index in [2, 3] {
        assert_eq!(fixture.chat["messages"][index]["name"], "lookup_fixture");
    }
    assert_eq!(
        text_part(&input[0]),
        &fixture.chat["messages"][0]["content"]
    );
    assert_eq!(
        text_part(input.as_array().unwrap().last().unwrap()),
        &fixture.chat["messages"][4]["content"]
    );
}

#[test]
fn newapi_chat_structured_preserves_full_json_schema_contract() {
    let fixture = fixture!("chat-structured");
    assert_eq!(fixture.decoded.responses["text"], fixture.reference["text"]);
    let format = &fixture.decoded.responses["text"]["format"];
    assert_eq!(format["type"], "json_schema");
    assert_eq!(format["strict"], true);
    assert_eq!(format["schema"]["required"], json!(["ready"]));
    assert_eq!(format["schema"]["additionalProperties"], false);
    assert_eq!(
        format["schema"],
        fixture.chat["response_format"]["json_schema"]["schema"]
    );
}

#[test]
fn newapi_messages_multiturn_preserves_tool_strings_at_protocol_layer() {
    let fixture = fixture!("messages-tool-multiturn");
    let input = &fixture.decoded.responses["input"];
    for kind in ["function_call", "function_call_output"] {
        let actual = typed_items(input, kind);
        assert_eq!(actual.len(), 3);
        assert_eq!(actual, typed_items(&fixture.reference["input"], kind));
    }
    let last = input.as_array().unwrap().last().unwrap();
    assert_eq!(last["call_id"], "toolu_fixture_c");
    assert_eq!(last["output"], fixture.chat["messages"][7]["content"]);
    assert!(last["output"].is_string());
    assert_eq!(
        last["output"],
        "[{\"type\":\"text\",\"text\":\"Synthetic record missing.\"}]"
    );
    assert_eq!(input[0]["role"], "system");
    assert_eq!(text_part(&input[0]), &fixture.reference["instructions"]);
    assert_eq!(fixture.chat["messages"][3]["name"], "lookup_fixture");
    assert_eq!(fixture.chat["messages"][4]["name"], "lookup_fixture");
    assert_eq!(fixture.chat["messages"][7]["name"], "lookup_fixture");
    assert_eq!(
        fixture.decoded.responses["tools"][0]["parameters"],
        fixture.reference["tools"][0]["parameters"]
    );
    assert_eq!(fixture.decoded.responses["tools"][0]["strict"], false);
    // This tests mapping only; Provider normalization may reject or remove a limit.
    assert_eq!(
        fixture.decoded.responses["max_output_tokens"],
        fixture.reference["max_output_tokens"]
    );
    assert_eq!(fixture.decoded.responses["max_output_tokens"], 256);
}

#[test]
fn newapi_messages_image_accepts_empty_go_metadata_without_changing_image_data() {
    let fixture = fixture!("messages-image");
    let content = &fixture.decoded.responses["input"][0]["content"];
    assert_eq!(
        fixture.chat["messages"][0]["content"][1]["image_url"]["MimeType"],
        ""
    );
    assert_eq!(content, &fixture.reference["input"][0]["content"]);
    assert!(fixture.decoded.stream);
    assert!(!fixture.decoded.include_usage);
    // As above, this is a pure protocol mapping assertion, not an upstream guarantee.
    assert_eq!(
        fixture.decoded.responses["max_output_tokens"],
        fixture.reference["max_output_tokens"]
    );
    assert_eq!(fixture.decoded.responses["max_output_tokens"], 128);
}
