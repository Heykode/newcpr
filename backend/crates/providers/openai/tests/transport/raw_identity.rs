use bytes::Bytes;
use provider_openai::transport::request::scope_raw_json_to_account;
use serde_json::{Value, json};

#[test]
fn clean_and_nonobject_standalone_bodies_keep_the_original_bytes() {
    for raw in [
        r#" { "prompt":"中文", "future":1.2300000000000000000001, "future":1e+999, "text":"\/\u0061" } "#,
        r#"{"client_metadata":null,"metadata":{"account_id":"business"},"input":[{"installation_id":"business"}]}"#,
        r#"{"client_metadata":[{"account_id":"business"}]}"#,
        r#"{"installation_id":"current","client_metadata":{"installationId":"current"}}"#,
        r#"{"installation\u005fid":"\u0063urrent"}"#,
        r#"["account_id",{"installation_id":"old"}]"#,
        "null",
        "42",
        r#""not an object""#,
        r#"{"account_id":"old",}"#,
        r#"{"account_id":"old"} trailing"#,
        "",
    ] {
        let original = Bytes::copy_from_slice(raw.as_bytes());
        let scoped = scope_raw_json_to_account(&original, "current");
        assert_eq!(scoped, original, "{raw}");
        assert_eq!(scoped.as_ptr(), original.as_ptr(), "{raw}");
    }
    let original = Bytes::from_static(&[0xff, 0xfe]);
    let scoped = scope_raw_json_to_account(&original, "current");
    assert_eq!(scoped.as_ptr(), original.as_ptr());
    assert_eq!(scoped, original);
}

#[test]
fn removes_all_known_identity_and_account_state_aliases_only_at_the_owned_scopes() {
    let keys = [
        "authorization",
        "Authorization",
        "cookie",
        "Cookie",
        "chatgpt-account-id",
        "chatgpt_account_id",
        "chatgptAccountId",
        "account_id",
        "accountId",
        "user_id",
        "userId",
        "chatgpt_user_id",
        "chatgptUserId",
        "access_token",
        "accessToken",
        "session_token",
        "sessionToken",
        "refresh_token",
        "refreshToken",
        "id_token",
        "idToken",
        "token",
        "cookies",
        "cookie_header",
        "cookieHeader",
        "cf_clearance",
        "turnState",
        "turn_state",
        "x-codex-turn-state",
        "previous_response_id",
        "previousResponseId",
        "response_id",
        "responseId",
        "conversation",
    ];
    let business = json!({
        "account_id": "business",
        "installation_id": "business",
        "previous_response_id": "business"
    });
    let mut body = json!({
        "session_id": "session",
        "id": "request",
        "conversation_id": "conversation",
        "client_metadata": {"nested": business},
        "metadata": business,
        "input": [business],
        "images": [business],
        "tools": [business],
        "query": business
    });
    for key in keys {
        body[key] = json!("old");
        body["client_metadata"][key] = json!("old");
    }
    let original = Bytes::from(serde_json::to_vec(&body).expect("original JSON"));
    let scoped: Value = serde_json::from_slice(&scope_raw_json_to_account(&original, "current"))
        .expect("scoped JSON");
    for key in keys {
        assert!(scoped.get(key).is_none(), "{key}");
        assert!(scoped["client_metadata"].get(key).is_none(), "{key}");
    }
    assert_eq!(scoped["session_id"], "session");
    assert_eq!(scoped["id"], "request");
    assert_eq!(scoped["conversation_id"], "conversation");
    assert_eq!(scoped["metadata"], business);
    assert_eq!(scoped["client_metadata"]["nested"], business);
    for key in ["input", "images", "tools"] {
        assert_eq!(scoped[key][0], business, "{key}");
    }
    assert_eq!(scoped["query"], business);
    assert!(scoped.get("installation_id").is_none());
    assert!(scoped["client_metadata"].get("installation_id").is_none());
}

#[test]
fn preserves_duplicate_unknown_members_raw_keys_and_numeric_lexemes_when_scoping() {
    let original = Bytes::from_static(
        concat!(
            " \n",
            r#"{"account_id":"old-a","future\u005fkey" : 1.2300000000000000000001, "account\u005fid":"old-b","future_key": 1e+999,"escaped":"\/\u0061","client_metadata":{"token":"old","f":-0,"f":2.300,"installationId":null},"installation_id":"old","installation_id":"older"}"#,
            "\n ",
        )
        .as_bytes(),
    );
    let scoped = scope_raw_json_to_account(&original, "current");
    let text = std::str::from_utf8(&scoped).expect("UTF-8");
    assert_eq!(
        text,
        " \n{\"future\\u005fkey\" : 1.2300000000000000000001,\"future_key\": 1e+999,\"escaped\":\"\\/\\u0061\",\"client_metadata\":{\"f\":-0,\"f\":2.300,\"installationId\":\"current\"},\"installation_id\":\"current\",\"installation_id\":\"current\"}\n "
    );
    let parsed: Value = serde_json::from_slice(&scoped).expect("valid transformed JSON");
    assert!(parsed.get("account_id").is_none());
    assert_eq!(parsed["installation_id"], "current");
    assert!(
        std::str::from_utf8(&original)
            .expect("original")
            .contains("old-a")
    );
}

#[test]
fn removing_first_middle_last_or_all_members_keeps_a_valid_object() {
    for (raw, expected) in [
        (r#"{"token":1,"x":2}"#, r#"{"x":2}"#),
        (r#"{"x":1,"token":2,"x":3}"#, r#"{"x":1,"x":3}"#),
        (r#"{"x":1,"token":2}"#, r#"{"x":1}"#),
        (r#"{"token":1,"token":2}"#, "{}"),
        (r#" { "token" : 1 } "#, " { } "),
        (
            r#"{"client_metadata":{"account_id":1,"token":2}}"#,
            r#"{"client_metadata":{}}"#,
        ),
        (r#"{"account_id":1}"#, "{}"),
    ] {
        let scoped = scope_raw_json_to_account(&Bytes::copy_from_slice(raw.as_bytes()), "current");
        assert_eq!(std::str::from_utf8(&scoped).expect("UTF-8"), expected);
        serde_json::from_slice::<Value>(&scoped).expect("valid object");
    }
}

#[test]
fn turn_metadata_preserves_duplicate_business_fields_and_escapes_unicode_for_headers() {
    let metadata = r#" { "account_id":"old","installationId":"old","future":1.2300000000000000000001,"future":1e+999,"path":"中文/😀","escaped":"\/\u0061","metadata":{"token":"business"},"turn_metadata":"nested-old-state" } "#;
    let mut body = json!({"client_metadata":{}});
    for key in ["turnMetadata", "turn_metadata", "x-codex-turn-metadata"] {
        body[key] = json!(metadata);
        body["client_metadata"][key] = json!(metadata);
    }
    let scoped = scope_raw_json_to_account(
        &Bytes::from(serde_json::to_vec(&body).expect("JSON")),
        "current",
    );
    let parsed: Value = serde_json::from_slice(&scoped).expect("scoped JSON");
    for container in [&parsed, &parsed["client_metadata"]] {
        for key in ["turnMetadata", "turn_metadata", "x-codex-turn-metadata"] {
            let metadata = container[key].as_str().expect("metadata string");
            assert!(metadata.is_ascii());
            assert!(metadata.contains(r#""future":1.2300000000000000000001,"future":1e+999"#));
            assert!(metadata.contains(r#""escaped":"\/\u0061""#));
            assert!(!metadata.contains("old"));
            let metadata: Value = serde_json::from_str(metadata).expect("metadata JSON");
            assert_eq!(metadata["installationId"], "current");
            assert_eq!(metadata["path"], "中文/😀");
            assert_eq!(metadata["metadata"]["token"], "business");
            assert!(metadata.get("installation_id").is_none());
            assert!(metadata.get("turn_metadata").is_none());
        }
    }
}

#[test]
fn unchanged_ascii_turn_metadata_keeps_the_entire_original_body() {
    let original = Bytes::from_static(
        br#"{"turnMetadata":" { \"future\":1e+999,\"future\":2.300,\"path\":\"\\u4e2d\" } "}"#,
    );
    let scoped = scope_raw_json_to_account(&original, "current");
    assert_eq!(scoped.as_ptr(), original.as_ptr());
    assert_eq!(scoped, original);
}

#[test]
fn body_turn_metadata_preserves_pretty_whitespace_and_escaped_newlines_when_scoping() {
    let metadata = concat!(
        "{\r\n  \"account_id\":\"old\",",
        "\r\n  \"installationId\":\"old\",",
        "\r\n  \"future\":1e+999,",
        "\n  \"future\":2.300,",
        "\r\n  \"text\":\"first\\nsecond\\rthird\"",
        "\r\n}\n"
    );
    let expected = concat!(
        "{\r\n  \"installationId\":\"current\",",
        "\r\n  \"future\":1e+999,",
        "\n  \"future\":2.300,",
        "\r\n  \"text\":\"first\\nsecond\\rthird\"",
        "\r\n}\n"
    );
    let body = json!({
        "turnMetadata":metadata,
        "client_metadata":{"x-codex-turn-metadata":metadata}
    });
    let scoped = scope_raw_json_to_account(
        &Bytes::from(serde_json::to_vec(&body).expect("JSON")),
        "current",
    );
    let parsed: Value = serde_json::from_slice(&scoped).expect("scoped JSON");
    assert_eq!(parsed["turnMetadata"], expected);
    assert_eq!(parsed["client_metadata"]["x-codex-turn-metadata"], expected);
    let metadata: Value = serde_json::from_str(expected).expect("metadata JSON");
    assert_eq!(metadata["text"], "first\nsecond\rthird");

    // A second projection has no identity changes and must not normalize the
    // formatting newlines that are legal inside the body metadata string.
    let unchanged = scope_raw_json_to_account(&scoped, "current");
    assert_eq!(unchanged.as_ptr(), scoped.as_ptr());
    assert_eq!(unchanged, scoped);
}

#[test]
fn opaque_turn_metadata_body_shapes_keep_the_existing_passthrough_contract() {
    for metadata in [
        json!("not JSON"),
        json!("[]"),
        json!("{"),
        json!(null),
        json!(["opaque"]),
        json!({"account_id":"old"}),
    ] {
        let body = json!({
            "prompt":"keep",
            "turnMetadata":metadata,
            "client_metadata":{"x-codex-turn-metadata":metadata,"future":1}
        });
        let original = Bytes::from(serde_json::to_vec(&body).expect("JSON"));
        let scoped = scope_raw_json_to_account(&original, "current");
        assert_eq!(scoped.as_ptr(), original.as_ptr());
        assert_eq!(scoped, original);
    }
}

#[test]
fn each_account_attempt_uses_the_immutable_original_body_and_replaces_all_installation_aliases() {
    let original = Bytes::from_static(
        br#"{"installation_id":"old","installationId":false,"x-codex-installation-id":null,"client_metadata":{"installation_id":"old","installationId":2,"x-codex-installation-id":[]}}"#,
    );
    for installation_id in ["account-a", "account-b", "account-\"quoted\""] {
        let scoped = scope_raw_json_to_account(&original, installation_id);
        let parsed: Value = serde_json::from_slice(&scoped).expect("scoped JSON");
        for container in [&parsed, &parsed["client_metadata"]] {
            for key in [
                "installation_id",
                "installationId",
                "x-codex-installation-id",
            ] {
                assert_eq!(container[key], installation_id);
            }
        }
    }
    assert!(
        std::str::from_utf8(&original)
            .expect("original")
            .contains("old")
    );
}
