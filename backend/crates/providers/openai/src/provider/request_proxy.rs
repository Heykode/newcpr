//! Account exit selection shared by Codex and Excel; preserves scheduling identity.
use super::*;
use gateway_core::account::ResponsesUpstream;

impl CodexProvider {
    pub(super) fn account_exit_client(
        &self,
        context: &AttemptContext,
        account: &ProviderAccount,
        excel: bool,
        session: Option<&str>,
    ) -> Result<CodexBackendClient, ProviderError> {
        use gateway_core::provider_ports::session_proxy::RequestProxySource;
        let client = self.client_for_request(context)?;
        let source = account.request_proxy_source();
        if source == RequestProxySource::Account {
            return client.for_account(account).map_err(|_| {
                provider_error(ProviderErrorKind::Unavailable, UpstreamSendState::NotSent)
            });
        }
        let unavailable = |reason: &str| {
            provider_error(ProviderErrorKind::Unavailable, UpstreamSendState::NotSent)
                .with_status(503)
                .with_upstream_code(OpaqueUpstreamValue::new(format!("account_proxy_{reason}")))
        };
        let pool = self
            .session_proxy_pool
            .as_ref()
            .ok_or_else(|| unavailable("manager_unavailable"))?;
        let transient = session.is_none();
        let identity = session
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let scope = json!([
            "account-proxy-v1",
            account.id().as_str(),
            context.client_api_key_ref().as_str(),
            identity
        ])
        .to_string();
        let route = if excel {
            ResponsesUpstream::Excel
        } else {
            ResponsesUpstream::Codex
        };
        let exit = pool
            .acquire(source, route, &scope, transient)
            .map_err(|e| unavailable(&e.to_string()))?;
        client
            .for_session_proxy(account, exit)
            .map_err(|_| unavailable("client_unavailable"))
    }
}
