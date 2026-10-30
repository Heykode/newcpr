use gateway_core::account::RequestLocation;

#[test]
fn changing_or_removing_proxy_drops_its_location() {
    use gateway_core::account::OutboundProxy;
    let location = RequestLocation::default();
    let direct = super::account("acct_location").with_request_location(Some(location.clone()));
    assert!(direct.request_location().is_none());
    let proxied = direct
        .with_outbound_proxy(Some(OutboundProxy::parse("http://localhost:8080").unwrap()))
        .with_request_location(Some(location.clone()));
    assert_eq!(proxied.request_location(), Some(&location));
    assert!(
        proxied
            .clone()
            .with_outbound_proxy(None)
            .request_location()
            .is_none()
    );
    assert!(
        proxied
            .with_outbound_proxy(Some(OutboundProxy::parse("http://localhost:8081").unwrap()))
            .request_location()
            .is_none()
    );
}

#[test]
fn location_validation_rejects_empty_controls_and_invalid_timezones() {
    assert!(RequestLocation::default().validate());
    for country in ["", "USA", "us", "U1"] {
        assert!(
            !RequestLocation {
                country: country.into(),
                ..Default::default()
            }
            .validate()
        );
    }
    for text in ["", " ", "Ohio\n"] {
        assert!(
            !RequestLocation {
                region: text.into(),
                ..Default::default()
            }
            .validate()
        );
        assert!(
            !RequestLocation {
                city: text.into(),
                ..Default::default()
            }
            .validate()
        );
    }
    let mut value = serde_json::to_value(RequestLocation::default()).unwrap();
    value["timezone"] = serde_json::json!("Invalid/Zone");
    assert!(serde_json::from_value::<RequestLocation>(value).is_err());
}
