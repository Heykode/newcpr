//! Core Provider 的能力投影和单次请求执行

mod affinity;
mod candidates;
mod execution;
mod feedback;
mod selection;
mod translation;
pub(crate) mod workers;

use crate::{
    credential::CredentialService, credential::catalog::CatalogService,
    credential::quota::QuotaService, profile::ReleaseService,
};
use async_trait::async_trait;
use gateway_core::{
    account::{AccountFeedbackStats, ProviderAccountId, ProviderAccountStore},
    concurrency::ConcurrencyWaitQueue,
    engine::{
        AttemptContext,
        provider::{Provider, ProviderRequest, ProviderStream},
    },
    error::{ErrorSource, ProviderError, ProviderErrorKind},
    provider_ports::{
        ProviderCooldownPort, ProviderLeasePort, ProviderSessionAffinityPort, ProviderStorePorts,
    },
    routing::{ProviderCatalogGeneration, ProviderKind, ProviderModelCapabilities},
};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct AntigravityProvider {
    kind: ProviderKind,
    credentials: Arc<CredentialService>,
    quota: Arc<QuotaService>,
    accounts: Arc<dyn ProviderAccountStore>,
    leases: Arc<dyn ProviderLeasePort>,
    cooldowns: Arc<dyn ProviderCooldownPort>,
    session_affinity: Arc<dyn ProviderSessionAffinityPort>,
    waiting: Arc<ConcurrencyWaitQueue<ProviderAccountId>>,
    catalog: Arc<CatalogService>,
    release: Arc<ReleaseService>,
    origin: url::Url,
    clients: Arc<crate::transport::ClientPool>,
    feedback: Arc<AccountFeedbackStats>,
}

impl AntigravityProvider {
    pub(crate) fn new(
        kind: ProviderKind,
        ports: &ProviderStorePorts,
        credentials: Arc<CredentialService>,
        catalog: Arc<CatalogService>,
        quota: Arc<QuotaService>,
        release: Arc<ReleaseService>,
        origin: url::Url,
    ) -> Self {
        Self {
            credentials,
            quota,
            accounts: ports.accounts(),
            leases: ports.leases(),
            cooldowns: ports.cooldowns(),
            session_affinity: ports.session_affinity(),
            waiting: Default::default(),
            kind,
            catalog,
            release,
            origin,
            clients: Default::default(),
            feedback: ports.account_feedback(),
        }
    }
}

#[async_trait]
impl Provider for AntigravityProvider {
    fn resolve_request_profile(
        &self,
    ) -> Result<Option<gateway_core::account::OpaqueProviderData>, ProviderError> {
        Ok(Some(
            crate::profile::default_profile(&self.release).frozen(),
        ))
    }

    fn resolve_key_request_profile(
        &self,
        configuration: Option<&gateway_core::account::OpaqueProviderData>,
    ) -> Result<Option<gateway_core::account::OpaqueProviderData>, ProviderError> {
        match configuration {
            Some(configuration) => crate::profile::resolve(configuration, &self.release)
                .map(|profile| Some(profile.frozen()))
                .map_err(|_| crate::transport::unsent(ProviderErrorKind::InvalidRequest)),
            None => self.resolve_request_profile(),
        }
    }

    fn name(&self) -> &'static str {
        "antigravity"
    }

    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::new(
            self.catalog
                .generation
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        self.catalog.capabilities().await
    }

    async fn execute(
        &self,
        request: ProviderRequest,
        context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        Arc::new(self.clone())
            .execute_native(request, context)
            .await
    }
}

fn unavailable(error: impl Into<ErrorSource>) -> ProviderError {
    crate::transport::unsent(ProviderErrorKind::ProviderInfrastructureUnavailable)
        .with_source(error)
}

mod web_search;
