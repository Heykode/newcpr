use gateway_core::engine::response_control::{ResponseControl, ResponseInterruptError};

#[test]
fn interrupt_requires_current_owner_and_exact_response() {
    let control = ResponseControl::default();
    assert_eq!(
        control.interrupt("resp_a"),
        Err(ResponseInterruptError::Unavailable)
    );
    let active = control.activate("resp_a".into()).unwrap();
    let waiter = active.requested();
    assert!(control.activate("resp_b".into()).is_none());
    assert_eq!(
        control.interrupt("resp_b"),
        Err(ResponseInterruptError::ResponseMismatch)
    );
    assert_eq!(control.interrupt("resp_a"), Ok(()));
    assert_eq!(control.interrupt("resp_a"), Ok(()));
    futures::executor::block_on(waiter);
    let pending = active.requested();
    drop(active);
    assert_eq!(
        control.interrupt("resp_a"),
        Err(ResponseInterruptError::Unavailable)
    );
    let next = control.activate("resp_b".into()).unwrap();
    assert_eq!(
        control.interrupt("resp_a"),
        Err(ResponseInterruptError::ResponseMismatch)
    );
    drop(pending);
    drop(next);
    assert!(control.activate(String::new()).is_none());
}

#[test]
fn response_controls_are_isolated_between_executions() {
    let first = ResponseControl::default();
    let second = ResponseControl::default();
    let active = first.activate("resp_owner".into()).unwrap();
    assert_eq!(
        second.interrupt("resp_owner"),
        Err(ResponseInterruptError::Unavailable)
    );
    assert_eq!(first.interrupt(active.response_id()), Ok(()));
}
