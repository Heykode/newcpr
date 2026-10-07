use gateway_core::error::{
    ErrorDetails, ErrorSource, GatewayError, ProviderError, ProviderErrorKind,
};
use gateway_core::upstream::UpstreamSendState;

#[test]
fn raw_capture_and_source_details_remain_separate_and_preserve_typed_causes() {
    use gateway_core::error::RawUpstreamError;
    use std::error::Error;
    let original = "raw upstream fixture";
    let error = ProviderError::new(ProviderErrorKind::Transport, UpstreamSendState::NotSent)
        .with_raw_upstream_error(RawUpstreamError::new(original))
        .with_source(std::io::Error::other("restricted transport cause"));
    let cloned = error.clone();
    assert_eq!(error.raw_upstream_error().unwrap().as_str(), original);
    assert_eq!(cloned.raw_upstream_error().unwrap().as_str(), original);
    assert!(cloned.source().unwrap().is::<std::io::Error>());
    let details: serde_json::Value =
        serde_json::from_str(&cloned.error_details().unwrap()).unwrap();
    assert_eq!(details["upstream"], original);
    assert_eq!(
        details["causes"]["messages"][0],
        "restricted transport cause"
    );
    assert!(!format!("{cloned:?}").contains(original));
    assert!(!format!("{cloned:?}").contains("restricted transport cause"));
}

#[test]
fn source_and_cleanup_survive_boundaries_but_never_enter_regular_formatting() {
    let source = ErrorSource::new(std::io::Error::other("protected-root-cause"))
        .with_cleanup(std::io::Error::other("protected-cleanup-cause"));
    let error = ProviderError::new(ProviderErrorKind::Transport, UpstreamSendState::NotSent)
        .with_source(source);
    let gateway = GatewayError::from_provider(&error);
    for formatted in [
        format!("{error:?}"),
        format!("{error}"),
        format!("{gateway:?}"),
        format!("{gateway}"),
    ] {
        assert!(!formatted.contains("protected-"));
    }
    let details = gateway.error_details().unwrap();
    assert!(details.as_str().contains("protected-root-cause"));
    assert!(details.as_str().contains("protected-cleanup-cause"));
    assert!(details.as_str().contains("cleanup"));
}

#[test]
fn protected_source_capture_is_bounded_at_utf8_boundaries() {
    let source = ErrorSource::new(std::io::Error::other("secret".repeat(30_000)));
    let details = ErrorDetails::capture(Some(&source), None, true).unwrap();
    assert!(details.as_str().len() < 66_000);
    let value: serde_json::Value = serde_json::from_str(details.as_str()).unwrap();
    assert_eq!(value["causes"]["truncated"], true);
    assert!(!format!("{details:?}").contains("secret"));
}
