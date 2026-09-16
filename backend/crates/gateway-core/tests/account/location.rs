use gateway_core::account::RequestLocation;

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
