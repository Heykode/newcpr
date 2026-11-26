mod admission;
mod capacity_wait;
mod connection;
mod continuation;
mod coordinator;
mod execution;
mod probe;
mod provider;
mod response_control;
mod response_control_binding;

use gateway_core::engine::AttemptTrigger;

#[test]
fn attempt_trigger_names_should_match_ops_event_contract() {
    assert_eq!(AttemptTrigger::AccountRetry.as_str(), "account_retry");
}

mod account_protection;
