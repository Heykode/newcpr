use gateway_protocol::openai::{
    OPENAI_SCHEDULING_SESSION_HINT_CONTEXT_KEY, OPENAI_SCHEDULING_SESSION_HINT_HEADERS,
    OpenAiSchedulingSessionHint, is_openai_scheduling_session_hint_header,
    is_transport_managed_request_header,
};
use serde_json::{Map, Value, json};

#[test]
fn scheduling_hint_sources_have_explicit_order_and_are_local_only() {
    assert_eq!(
        OPENAI_SCHEDULING_SESSION_HINT_HEADERS,
        [
            "x-session-affinity",
            "x-session-id",
            "x-opencode-session",
            "x-conversation-id",
            "x-qx-affinity-codex-session",
            "x-qx-affinity-claude-session",
        ]
    );
    for source in OPENAI_SCHEDULING_SESSION_HINT_HEADERS {
        assert!(is_openai_scheduling_session_hint_header(source));
        assert!(is_transport_managed_request_header(source));
        let hint = OpenAiSchedulingSessionHint::new(source, "  session-1\t").expect("valid hint");
        assert_eq!(hint.source(), source);
        assert_eq!(hint.id(), "session-1");
    }
    for source in [
        "",
        "X-Session-Id",
        " x-session-id",
        "session-id",
        "session_id",
        "conversation-id",
        "thread-id",
        "x-codex-turn-metadata",
        "x-future-session",
    ] {
        assert!(!is_openai_scheduling_session_hint_header(source));
        assert!(OpenAiSchedulingSessionHint::new(source, "session-1").is_none());
    }
}

#[test]
fn scheduling_hint_values_are_bounded_ascii_graphic_after_trimming() {
    for invalid in ["", " \t\r\n ", "a b", "a\tb", "a\nb", "a\0b", "caf\u{e9}"] {
        assert!(
            OpenAiSchedulingSessionHint::new("x-session-id", invalid).is_none(),
            "accepted {invalid:?}"
        );
    }
    for byte in 0..=127_u8 {
        let value = format!("a{}b", char::from(byte));
        assert_eq!(
            OpenAiSchedulingSessionHint::new("x-session-id", &value).is_some(),
            byte.is_ascii_graphic(),
            "ASCII byte {byte}"
        );
    }
    assert!(OpenAiSchedulingSessionHint::new("x-session-id", "a").is_some());
    let longest = "a".repeat(1024);
    let hint = OpenAiSchedulingSessionHint::new("x-session-id", &format!("\t{longest} "))
        .expect("1024 normalized bytes are allowed");
    assert_eq!(hint.id(), longest);
    assert!(OpenAiSchedulingSessionHint::new("x-session-id", &"a".repeat(1025)).is_none());
}

#[test]
fn scheduling_hint_context_roundtrip_revalidates_source_and_identifier() {
    for source in OPENAI_SCHEDULING_SESSION_HINT_HEADERS {
        let hint = OpenAiSchedulingSessionHint::new(source, "session-1").expect("valid hint");
        let context = Map::from_iter([(
            OPENAI_SCHEDULING_SESSION_HINT_CONTEXT_KEY.to_owned(),
            hint.to_context_value(),
        )]);
        assert_eq!(
            OpenAiSchedulingSessionHint::from_context(&context),
            Some(hint)
        );
        assert_eq!(
            context[OPENAI_SCHEDULING_SESSION_HINT_CONTEXT_KEY],
            json!({"source":source,"id":"session-1"})
        );
    }
    for invalid in [
        Value::Null,
        json!("session-1"),
        json!([]),
        json!({}),
        json!({"source":"x-session-id"}),
        json!({"id":"session-1"}),
        json!({"source":17,"id":"session-1"}),
        json!({"source":"x-session-id","id":17}),
        json!({"source":"session-id","id":"session-1"}),
        json!({"source":"x-session-id","id":"contains spaces"}),
        json!({"source":"x-session-id","id":"x".repeat(1025)}),
    ] {
        let context = Map::from_iter([(
            OPENAI_SCHEDULING_SESSION_HINT_CONTEXT_KEY.to_owned(),
            invalid,
        )]);
        assert!(OpenAiSchedulingSessionHint::from_context(&context).is_none());
    }
    assert!(OpenAiSchedulingSessionHint::from_context(&Map::new()).is_none());
    assert!(
        OpenAiSchedulingSessionHint::from_context(
            json!({"source":"x-session-id","id":"not-context"})
                .as_object()
                .expect("object")
        )
        .is_none()
    );
}

#[test]
fn scheduling_hints_keep_sources_distinct_and_redact_identifiers_in_debug() {
    let first =
        OpenAiSchedulingSessionHint::new("x-session-id", "private-session-marker").expect("hint");
    let second = OpenAiSchedulingSessionHint::new("x-conversation-id", "private-session-marker")
        .expect("hint");
    assert_ne!(first, second);
    let debug = format!("{first:?}");
    assert!(debug.contains("x-session-id"));
    assert!(!debug.contains("private-session-marker"));
}
