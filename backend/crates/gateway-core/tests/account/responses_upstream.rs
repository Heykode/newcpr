use gateway_core::account::Excel403Action as Action;

#[test]
fn legacy_flags_and_explicit_actions_are_unambiguous() {
    assert_eq!(Action::resolve(None, None).unwrap(), None);
    assert_eq!(
        Action::resolve(None, Some(true)).unwrap(),
        Some(Action::PauseAccount)
    );
    assert_eq!(
        Action::resolve(None, Some(false)).unwrap(),
        Some(Action::None)
    );
    for action in [Action::None, Action::PauseAccount, Action::DisableExcel] {
        assert_eq!(Action::parse(action.as_str()), Some(action));
        assert_eq!(Action::resolve(Some(action), None).unwrap(), Some(action));
        assert_eq!(
            Action::resolve(Some(action), Some(action == Action::PauseAccount)).unwrap(),
            Some(action)
        );
        assert!(Action::resolve(Some(action), Some(action != Action::PauseAccount)).is_err());
    }
    assert_eq!(Action::parse("unknown"), None);
}
