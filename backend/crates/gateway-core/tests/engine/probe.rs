use gateway_core::engine::probe::AccountProbe;

#[test]
fn account_probe_is_an_object_safe_execution_chain_port() {
    fn accepts(_: Option<&dyn AccountProbe>) {}
    accepts(None);
}

#[test]
fn provider_probe_hint_requires_no_upstream_facts() {
    use gateway_core::engine::probe::{AccountProbeError, AccountProbeErrorSource};
    use gateway_core::error::{
        ProviderDiagnostic, ProviderError, ProviderErrorKind, ProxyConnectionHint,
    };
    use gateway_core::upstream::UpstreamSendState;
    let local = ProviderError::new(ProviderErrorKind::Transport, UpstreamSendState::NotSent)
        .with_diagnostic(
            ProviderDiagnostic::new("connection failed")
                .with_proxy_hint(ProxyConnectionHint::CheckProxy),
        );
    let error = AccountProbeError::from_provider(&local);
    assert_eq!(error.source(), AccountProbeErrorSource::Provider);
    assert_eq!(error.proxy_hint(), Some(ProxyConnectionHint::CheckProxy));
    let error = AccountProbeError::from_provider(&local.with_status(502));
    assert_eq!(error.source(), AccountProbeErrorSource::Upstream);
    assert_eq!(error.proxy_hint(), None);
    assert_eq!(error.send_state(), Some(UpstreamSendState::NotSent));
    assert_eq!(
        error.provider_error_kind(),
        Some(ProviderErrorKind::Transport)
    );
}
