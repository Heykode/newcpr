use gateway_core::account::FastMode;

#[test]
fn group_policy_merge_is_order_independent_and_disabled_wins() {
    use FastMode::{Default, Disabled, Enabled};
    for (left, right, expected) in [
        (Default, Default, Default),
        (Default, Enabled, Enabled),
        (Default, Disabled, Disabled),
        (Enabled, Enabled, Enabled),
        (Enabled, Disabled, Disabled),
        (Disabled, Disabled, Disabled),
    ] {
        assert_eq!(left.merge(right), expected);
        assert_eq!(right.merge(left), expected);
    }
}

#[test]
fn wire_values_are_strict_and_round_trip() {
    for mode in [FastMode::Default, FastMode::Enabled, FastMode::Disabled] {
        assert_eq!(FastMode::parse(mode.as_str()), Some(mode));
        assert_eq!(
            serde_json::from_value::<FastMode>(serde_json::json!(mode.as_str())).unwrap(),
            mode
        );
    }
    for value in ["fast", "Enabled", "", "enabled "] {
        assert!(FastMode::parse(value).is_none());
        assert!(serde_json::from_value::<FastMode>(serde_json::json!(value)).is_err());
    }
}
