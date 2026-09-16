//! Characterization of current behavior, not approval of the overwrite policy.

use chrono::Utc;
use provider_openai::transport::{
    CodexRequestContext, profile::CodexRequestLocation, protocol::responses::CodexResponsesRequest,
    qx_application::project_response_request,
};

use super::*;

fn west_coast() -> CodexRequestLocation {
    serde_json::from_value(json!({
        "country": "US", "region": "California", "city": "Los Angeles",
        "timezone": "America/Los_Angeles"
    }))
    .unwrap()
}

fn encode(body: Value, location: &CodexRequestLocation) -> CodexResponsesRequest {
    let generate = request(body.as_object().unwrap().clone());
    let mut encoded = encode_generate_request(&generate, "gpt-test", location).unwrap();
    assert_eq!(
        generate.protocol_payload().body(),
        body.as_object().unwrap()
    );
    encoded.client_api_key_id = Some("key-location-review".to_owned());
    encoded
}

fn context(request_id: &str) -> CodexRequestContext<'_> {
    CodexRequestContext::auxiliary(
        "Bearer synthetic-token",
        Some("account-location-review"),
        request_id,
        Some("installation-location-review"),
    )
}

fn cache(request: &CodexResponsesRequest, request_id: &str) -> String {
    project_response_request(request, context(request_id))
        .prompt_cache_key()
        .unwrap()
        .to_owned()
}

fn environment(text: &str) -> Value {
    json!({
        "role": "user",
        "content": [{"type": "input_text", "text": text}],
        "internal_chat_message_metadata_passthrough": {
            "content_item_kinds": ["environments.environment_context"]
        }
    })
}

#[test]
fn current_search_policy_replaces_explicit_locations_and_fills_missing_ones() {
    for kind in [
        "web_search",
        "web_search_preview",
        "web_search_preview_2025_03_11",
    ] {
        for supplied in [
            None,
            Some(Value::Null),
            Some(json!({
                "type": "approximate", "country": "GB", "city": "London",
                "timezone": "Europe/London", "future_location_field": "preserve?"
            })),
        ] {
            let mut tool = json!({"type": kind, "search_context_size": "low"});
            if let Some(location) = supplied {
                tool["user_location"] = location;
            }
            let encoded = encode(json!({"input": "hello", "tools": [tool]}), &west_coast());
            assert_eq!(
                encoded.body()["tools"][0],
                json!({
                    "type": kind,
                    "search_context_size": "low",
                    "user_location": {
                        "type": "approximate", "country": "US", "region": "California",
                        "city": "Los Angeles", "timezone": "America/Los_Angeles"
                    }
                })
            );
        }
    }
}

#[test]
fn location_does_not_invent_search_tools_or_rewrite_unmarked_user_content() {
    let text = "<environment_context><current_date>2000-01-01</current_date><timezone>UTC</timezone></environment_context>";
    let bodies = [
        json!({"input": text}),
        json!({"input": [{"role": "user", "content": [{"type": "input_text", "text": text}]}]}),
        json!({"input": "hello", "tools": [
            {"type": "function", "name": "web_search", "user_location": {"city": "London"}},
            {"type": "image_generation", "user_location": {"city": "Paris"}}
        ]}),
    ];
    for body in bodies {
        let encoded = encode(body.clone(), &west_coast());
        assert_eq!(encoded.body()["input"], body["input"]);
        assert_eq!(encoded.body().get("tools"), body.get("tools"));
    }
}

#[test]
fn marked_environment_only_rewrites_date_and_timezone_in_valid_user_xml() {
    let text = "<environment_context><current_date>2000-01-01</current_date><timezone>UTC</timezone><cwd>/synthetic/work</cwd><epoch>946684800</epoch></environment_context>";
    let mut assistant = environment(text);
    assistant["role"] = json!("assistant");
    let invalid = environment("<environment_context><timezone>UTC</environment_context>");
    let body = json!({"input": [environment(text), assistant, invalid], "epoch": 946684800});
    let location = west_coast();
    let before = Utc::now().with_timezone(&location.timezone).date_naive();
    let encoded = encode(body.clone(), &location);
    let after = Utc::now().with_timezone(&location.timezone).date_naive();
    let text = encoded.body()["input"][0]["content"][0]["text"]
        .as_str()
        .unwrap();
    let document = roxmltree::Document::parse(text).unwrap();
    let field = |name: &str| {
        document
            .root_element()
            .children()
            .find(|node| node.has_tag_name(name))
            .unwrap()
            .text()
            .unwrap()
    };
    assert!(
        [before.to_string(), after.to_string()]
            .iter()
            .any(|date| date == field("current_date"))
    );
    assert_eq!(field("timezone"), "America/Los_Angeles");
    assert_eq!(field("cwd"), "/synthetic/work");
    assert_eq!(field("epoch"), "946684800");
    assert_eq!(encoded.body()["epoch"], 946684800);
    assert_eq!(encoded.body()["input"][1], body["input"][1]);
    assert_eq!(encoded.body()["input"][2], body["input"][2]);
}

#[test]
fn unmarked_plain_chat_keeps_its_cache_across_location_configuration_changes() {
    let body = json!({"input": "hello", "instructions": "be concise"});
    let east = encode(body.clone(), &Default::default());
    let west = encode(body, &west_coast());
    assert_eq!(east.body(), west.body());
    assert_eq!(cache(&east, "request-a"), cache(&west, "request-b"));
}

#[test]
fn unmarked_search_cache_is_repeatable_but_changes_with_configured_location() {
    let body = json!({"input": "hello", "tools": [{"type": "web_search"}]});
    let east = encode(body.clone(), &Default::default());
    let west = encode(body, &west_coast());
    assert_eq!(cache(&east, "request-a"), cache(&east, "request-b"));
    assert_ne!(cache(&east, "request-a"), cache(&west, "request-a"));
}

#[test]
fn explicit_identity_keeps_cache_stable_when_search_location_changes() {
    for marker in [
        "conversation_id",
        "session_id",
        "thread_id",
        "prompt_cache_key",
    ] {
        let mut body = json!({"input": "hello", "tools": [{"type": "web_search"}]});
        body[marker] = json!("stable-client-marker");
        let east = encode(body.clone(), &Default::default());
        let west = encode(body, &west_coast());
        assert_ne!(east.body()["tools"], west.body()["tools"]);
        assert_eq!(
            cache(&east, "request-a"),
            cache(&west, "request-b"),
            "{marker}"
        );
    }
}

#[test]
fn current_overwrite_policy_collapses_location_only_differences_within_one_owner() {
    let make = |city| {
        encode(
            json!({
                "input": "hello",
                "tools": [{"type": "web_search", "user_location": {"city": city}}]
            }),
            &west_coast(),
        )
    };
    let london = make("London");
    let mut tokyo = make("Tokyo");
    assert_eq!(london.body(), tokyo.body());
    assert_eq!(cache(&london, "request-a"), cache(&tokyo, "request-b"));
    tokyo.client_api_key_id = Some("different-key".to_owned());
    assert_ne!(cache(&london, "request-a"), cache(&tokyo, "request-b"));
}

#[test]
fn current_unmarked_cache_changes_when_an_encoded_environment_crosses_a_date_boundary() {
    // Fixed post-encoding dates exercise the real hash without changing the process clock
    // or adding a clock override to production code.
    let make = |date| {
        let encoded = encode(
            json!({"input": [environment(
                "<environment_context><current_date>2000-01-01</current_date><timezone>UTC</timezone></environment_context>"
            )]}),
            &west_coast(),
        );
        let mut body = encoded.body().clone();
        body["input"][0]["content"][0]["text"] = json!(format!(
            "<environment_context><current_date>{date}</current_date><timezone>America/Los_Angeles</timezone></environment_context>"
        ));
        let mut fixture = CodexResponsesRequest::from_body(body);
        fixture.client_api_key_id = encoded.client_api_key_id;
        fixture
    };
    let today = make("2026-09-15");
    let tomorrow = make("2026-09-16");
    assert_ne!(cache(&today, "request-a"), cache(&tomorrow, "request-b"));

    let retry = project_response_request(&today, context("request-a"));
    assert_eq!(cache(&today, "request-a"), cache(&retry, "retry"));
    let mut other_account = context("retry");
    other_account.account_id = Some("another-account");
    assert_ne!(
        cache(&retry, "retry"),
        project_response_request(&retry, other_account)
            .prompt_cache_key()
            .unwrap()
    );
}

#[test]
fn current_first_user_fallback_cannot_distinguish_prompts_after_the_same_environment() {
    let make = |prompt| {
        encode(
            json!({"input": [
                environment("<environment_context><timezone>UTC</timezone></environment_context>"),
                {"role": "user", "content": prompt}
            ]}),
            &west_coast(),
        )
    };
    let first = make("first independent question");
    let second = make("different independent question");
    assert_ne!(first.body()["input"], second.body()["input"]);
    assert_eq!(cache(&first, "request-a"), cache(&second, "request-b"));
}
