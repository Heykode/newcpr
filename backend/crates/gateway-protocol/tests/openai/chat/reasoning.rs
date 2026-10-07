use gateway_protocol::openai::chat::decode_chat_request;
use serde_json::{Value, json};

fn with_options(options: Value) -> Value {
    let mut request = super::request();
    request
        .as_object_mut()
        .unwrap()
        .extend(options.as_object().unwrap().clone());
    request
}

#[test]
fn reasoning_aliases_preserve_effective_effort_and_explicit_summary() {
    for (options, expected) in [
        (json!({"reasoning_effort":"high"}), json!({"effort":"high"})),
        (
            json!({"reasoning":{"effort":"medium"}}),
            json!({"effort":"medium"}),
        ),
        (
            json!({"reasoning_effort":"high","reasoning":{"effort":"medium","summary":"detailed"}}),
            json!({"effort":"medium","summary":"detailed"}),
        ),
        (
            json!({"reasoning_effort":"low","reasoning":{}}),
            json!({"effort":"low"}),
        ),
        (
            json!({"reasoning_effort":"low","reasoning":{"effort":null}}),
            json!({"effort":"low"}),
        ),
        (
            json!({"reasoning_effort":"low","reasoning":{"effort":"  "}}),
            json!({"effort":"low"}),
        ),
        (
            json!({"reasoning_effort":"max","reasoning":null}),
            json!({"effort":"max"}),
        ),
        (
            json!({"reasoning":{"effort":"max","summary":null}}),
            json!({"effort":"max","summary":null}),
        ),
        (
            json!({"reasoning":{"summary":"concise"}}),
            json!({"summary":"concise"}),
        ),
        (
            json!({"reasoning":{"summary":"auto"}}),
            json!({"summary":"auto"}),
        ),
    ] {
        let decoded = decode_chat_request(with_options(options)).unwrap();
        assert_eq!(decoded.responses["reasoning"], expected);
        assert!(!decoded.responses.contains_key("reasoning_effort"));
        assert_eq!(decoded.responses["input"][0]["content"][0]["text"], "hello");
    }
    for effort in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
        let flat = decode_chat_request(with_options(json!({"reasoning_effort":effort}))).unwrap();
        let nested =
            decode_chat_request(with_options(json!({"reasoning":{"effort":effort}}))).unwrap();
        assert_eq!(flat, nested);
    }
}

#[test]
fn reasoning_omission_does_not_invent_defaults() {
    for options in [
        json!({}),
        json!({"reasoning":null}),
        json!({"reasoning":{}}),
        json!({"reasoning_effort":null,"reasoning":{"effort":null}}),
    ] {
        assert!(
            !decode_chat_request(with_options(options))
                .unwrap()
                .responses
                .contains_key("reasoning")
        );
    }
}

#[test]
fn reasoning_rejects_malformed_fields_without_silently_dropping_them() {
    for (options, param) in [
        (json!({"reasoning":[]}), "reasoning"),
        (json!({"reasoning":"high"}), "reasoning"),
        (json!({"reasoning":{"effort":true}}), "reasoning.effort"),
        (
            json!({"reasoning":{"effort":"unknown"}}),
            "reasoning.effort",
        ),
        (
            json!({"reasoning":{"summary":"secret-value"}}),
            "reasoning.summary",
        ),
        (json!({"reasoning":{"summary":false}}), "reasoning.summary"),
        (json!({"reasoning":{"future":"secret-value"}}), "reasoning"),
        (
            json!({"reasoning_effort":3,"reasoning":{"effort":"high"}}),
            "reasoning_effort",
        ),
        (json!({"reasoning_effort":""}), "reasoning_effort"),
    ] {
        let error = decode_chat_request(with_options(options)).unwrap_err();
        assert_eq!(error.param(), Some(param));
        assert!(!error.message().contains("secret-value"));
    }
}
