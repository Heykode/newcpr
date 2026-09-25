use gateway_admin::model::token_guard::TokenGuardConfig;

#[test]
fn guard_defaults_off_and_enforces_bounded_work() {
    let config = TokenGuardConfig::default();
    assert!(!config.enabled);
    assert!(config.validate().is_ok());
    for concurrency in [0, 17] {
        assert!(
            TokenGuardConfig {
                concurrency,
                ..config.clone()
            }
            .validate()
            .is_err()
        );
    }
    for max_per_cycle in [0, 101] {
        assert!(
            TokenGuardConfig {
                max_per_cycle,
                ..config.clone()
            }
            .validate()
            .is_err()
        );
    }
    for interval_seconds in [0, 29, 86_401] {
        assert!(
            TokenGuardConfig {
                interval_seconds,
                ..config.clone()
            }
            .validate()
            .is_err()
        );
    }
    for timeout_seconds in [0, 4, 901] {
        assert!(
            TokenGuardConfig {
                timeout_seconds,
                ..config.clone()
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        serde_json::to_string(&config)
            .unwrap()
            .find("endpoint")
            .is_none()
    );
    assert!(
        TokenGuardConfig {
            model: "".into(),
            ..config
        }
        .validate()
        .is_err()
    );
}
