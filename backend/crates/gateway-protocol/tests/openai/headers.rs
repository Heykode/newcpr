use gateway_protocol::openai::{is_transport_managed_request_header, parse_retry_after_seconds};

#[test]
fn retry_after_accepts_seconds_and_http_dates_without_accepting_invalid_durations() {
    for (value, expected) in [
        ("0", Some(0)),
        (" 120 ", Some(120)),
        ("Thu, 01 Jan 1970 00:00:00 GMT", Some(0)),
        ("", None),
        ("-1", None),
        ("1.5", None),
        ("unknown", None),
        ("18446744073709551616", None),
    ] {
        assert_eq!(parse_retry_after_seconds(value), expected, "{value}");
    }
    let future =
        httpdate::fmt_http_date(std::time::SystemTime::now() + std::time::Duration::from_secs(60));
    assert!((59..=60).contains(&parse_retry_after_seconds(&future).unwrap()));
}

#[test]
fn transport_headers_should_include_proxy_namespaces_and_compression() {
    for name in [
        "cf-visitor",
        "cf-connecting-ip",
        "cf-connecting-ipv6",
        "cf-pseudo-ipv4",
        "cf-ray",
        "cf-ipcountry",
        "cf-warp-tag-id",
        "cf-worker",
        "cf-ew-via",
        "cf-future-proxy-field",
        "cdn-loop",
        "via",
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-forwarded-port",
        "x-forwarded-prefix",
        "x-forwarded-future-field",
        "x-real-ip",
        "true-client-ip",
        "x-request-id",
        "accept-encoding",
        "content-encoding",
        "content-length",
        "host",
        "connection",
        "keep-alive",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "sec-websocket-key",
        "sec-websocket-extensions",
    ] {
        assert!(is_transport_managed_request_header(name), "missing {name}");
    }
}

#[test]
fn transport_headers_should_leave_business_extensions_to_the_protocol_owner() {
    for name in [
        "x-openai-future-mode",
        "x-custom-extension",
        "x-client-request-id",
        "session-id",
        "thread-id",
        "x-codex-turn-state",
        "x-codex-beta-features",
        "openai-beta",
        "accept",
        "content-type",
        "x-cf-business-field",
    ] {
        assert!(
            !is_transport_managed_request_header(name),
            "unexpected {name}"
        );
    }
}
