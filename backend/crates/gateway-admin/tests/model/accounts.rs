use gateway_admin::model::accounts::normalize_custom_name;

#[test]
fn custom_names_normalize_without_using_provider_identity() {
    assert_eq!(normalize_custom_name(None).unwrap(), None);
    assert_eq!(normalize_custom_name(Some("   ")).unwrap(), None);
    assert_eq!(
        normalize_custom_name(Some("  Batch one  "))
            .unwrap()
            .as_deref(),
        Some("Batch one")
    );
    assert_eq!(
        normalize_custom_name(Some(&"名".repeat(128)))
            .unwrap()
            .unwrap()
            .chars()
            .count(),
        128
    );
    assert!(normalize_custom_name(Some(&"名".repeat(129))).is_err());
    for value in ["bad\nname", "\t", "name\u{7f}", "name\u{85}"] {
        assert!(normalize_custom_name(Some(value)).is_err());
    }
}
