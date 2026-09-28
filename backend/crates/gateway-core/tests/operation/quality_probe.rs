use super::*;
use gateway_core::operation::quality_probe::{
    QualityProbeExchange, StateProbeReason, StateProbeShot, StateProbeVerdict,
};

#[test]
fn quality_probe_is_not_enabled_by_client_json_and_debug_never_leaks_state() {
    let request = generate(json!({"qualityProbe": {"continuation": true}, "input": "fixture"}));
    assert!(request.quality_probe().is_none());
    let exchange = QualityProbeExchange::default();
    let first = exchange.step(false);
    assert!(first.begin());
    first.observe(
        StateProbeShot::default(),
        Some(
            ProviderSessionState::new(
                "openai",
                json!({"ticket": "private-ticket", "cookies": "private-cookie"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap(),
        ),
    );
    let second = exchange.step(true);
    assert!(second.input().is_some());
    let text = format!("{exchange:?} {first:?} {second:?}");
    assert!(!text.contains("private-ticket"));
    assert!(!text.contains("private-cookie"));
    let report = serde_json::to_string(&exchange.report()).unwrap();
    assert!(!report.contains("private-ticket"));
    assert!(!report.contains("private-cookie"));
}

#[test]
fn quality_probe_requires_two_valid_observations_and_fences_retries() {
    for changed in [false, true] {
        let exchange = QualityProbeExchange::default();
        let first = exchange.step(false);
        assert!(first.begin());
        first.observe(StateProbeShot::default(), None);
        assert_eq!(exchange.report().verdict, StateProbeVerdict::Inconclusive);
        let second = exchange.step(true);
        assert!(second.begin());
        second.observe(
            StateProbeShot {
                changed: Some(changed),
                ..Default::default()
            },
            None,
        );
        assert_eq!(
            exchange.report().verdict,
            if changed {
                StateProbeVerdict::Degraded
            } else {
                StateProbeVerdict::Healthy
            }
        );
        assert!(!second.begin());
        assert_eq!(exchange.report().verdict, StateProbeVerdict::Inconclusive);
        assert_eq!(exchange.report().reason, StateProbeReason::RepeatedAttempt);
    }
}
