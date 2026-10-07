use bytes::Bytes;
use gateway_core::operation::{
    ProviderHttpHeader, ProviderHttpMethod, ProviderHttpRequest, RawHttpPayload,
};

fn payload() -> RawHttpPayload {
    RawHttpPayload::new("openai", Bytes::from_static(b"opaque-live-body")).unwrap()
}

#[test]
fn provider_http_requests_bound_metadata_and_preserve_opaque_payloads() {
    let request = ProviderHttpRequest::new(
        "live",
        ProviderHttpMethod::Post,
        Some("model=voice".to_owned()),
        vec![ProviderHttpHeader::new(
            "Content-Type",
            Bytes::from_static(b"application/sdp"),
        )],
        payload(),
    )
    .unwrap();
    assert_eq!(request.payload().body().as_ref(), b"opaque-live-body");
    assert!(!format!("{request:?}").contains("opaque-live-body"));
    for endpoint in ["", "..", "../live", "/live", "https://example.com"] {
        assert!(
            ProviderHttpRequest::new(endpoint, ProviderHttpMethod::Post, None, vec![], payload())
                .is_err()
        );
    }
    for query in [
        "q=bad\r\nvalue".to_owned(),
        "q=bad\0value".to_owned(),
        "q".repeat(8193),
    ] {
        assert!(
            ProviderHttpRequest::new(
                "live",
                ProviderHttpMethod::Post,
                Some(query),
                vec![],
                payload()
            )
            .is_err()
        );
    }
    for headers in [
        vec![ProviderHttpHeader::new("invalid:name", Bytes::new())],
        vec![ProviderHttpHeader::new(
            "X-Test",
            Bytes::from_static(b"bad\r\nheader"),
        )],
        vec![ProviderHttpHeader::new("X-Test", Bytes::new()); 65],
    ] {
        assert!(
            ProviderHttpRequest::new("live", ProviderHttpMethod::Post, None, headers, payload())
                .is_err()
        );
    }
}
