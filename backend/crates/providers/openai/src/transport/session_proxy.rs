//! Managed exit ownership and feedback; never participates in replay decisions.
use std::{fmt, sync::Arc};

use gateway_core::provider_ports::session_proxy::{SessionProxyLease, SessionProxyOutcome};

use super::{CodexBackendClient, CodexClientError, websocket::CodexWebSocketExchangeError};

#[derive(Clone)]
pub(crate) struct SessionProxyHold(pub(crate) Arc<dyn SessionProxyLease>);

impl fmt::Debug for SessionProxyHold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionProxyHold(<redacted>)")
    }
}
impl PartialEq for SessionProxyHold {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for SessionProxyHold {}

impl SessionProxyHold {
    pub(crate) fn report_websocket_error(&self, error: &CodexWebSocketExchangeError) {
        use CodexWebSocketExchangeError as E;
        let outcome = match error.diagnostic_cause() {
            E::Upstream(error) if (500..600).contains(&error.status_code) => {
                SessionProxyOutcome::UpstreamFailure
            }
            E::Connect(_) | E::ConnectTimeout { .. } => SessionProxyOutcome::NetworkFailure,
            E::Transport(_)
            | E::SendTimeout { .. }
            | E::ClosedBeforeTerminal(_)
            | E::StreamEndedBeforeTerminal { .. }
            | E::ReceiveIdleTimeout { .. } => SessionProxyOutcome::StreamFailure,
            // Local capacity, cancellation, continuation and business rejection are
            // not evidence that the proxy node is unhealthy.
            _ => return,
        };
        self.0.report(outcome);
    }
}

impl CodexBackendClient {
    pub(crate) fn report_session_proxy_completed(&self) {
        if let Some(lease) = &self.session_proxy {
            lease.report(SessionProxyOutcome::Completed);
        }
    }

    pub(crate) fn report_session_proxy_stream_error(&self, error: &CodexClientError) {
        let Some(lease) = &self.session_proxy else {
            return;
        };
        match error {
            CodexClientError::WebSocket(error) => {
                SessionProxyHold(lease.clone()).report_websocket_error(error)
            }
            CodexClientError::Http(_) | CodexClientError::HttpJson(_) => {
                lease.report(SessionProxyOutcome::StreamFailure)
            }
            _ => {}
        }
    }
}
