//! Dedicated outbound UA mutation; unrelated settings are never replaced.

use std::sync::Arc;

use async_trait::async_trait;
use gateway_core::{routing::ProviderKind, runtime::SnapshotControl};
use tokio::sync::Mutex;

use crate::{
    model::{
        AdminError, MutationContext,
        user_agent::{OutboundUserAgentView, ProviderUserAgentOverride},
    },
    ports::{
        provider::{ProviderAdminErrorKind, ProviderAdminRegistry},
        store::SettingsStore,
    },
};

use super::{map_provider_error, map_store_error, publish_committed};

#[async_trait]
pub trait OutboundUserAgentService: Send + Sync {
    fn load(&self, provider: &ProviderKind) -> Result<OutboundUserAgentView, AdminError>;
    fn preview(
        &self,
        provider: &ProviderKind,
        selection: &ProviderUserAgentOverride,
    ) -> Result<OutboundUserAgentView, AdminError>;
    async fn replace(
        &self,
        context: &MutationContext,
        provider: &ProviderKind,
        selection: ProviderUserAgentOverride,
    ) -> Result<OutboundUserAgentView, AdminError>;
}

pub(crate) struct DefaultOutboundUserAgentService {
    store: Arc<dyn SettingsStore>,
    providers: ProviderAdminRegistry,
    snapshot: Arc<dyn SnapshotControl>,
    mutation: Mutex<()>,
}

impl DefaultOutboundUserAgentService {
    pub(crate) fn new(
        store: Arc<dyn SettingsStore>,
        providers: ProviderAdminRegistry,
        snapshot: Arc<dyn SnapshotControl>,
    ) -> Self {
        Self {
            store,
            providers,
            snapshot,
            mutation: Mutex::new(()),
        }
    }

    pub(crate) async fn reconcile(&self, provider_kind: &ProviderKind) -> Result<(), AdminError> {
        // Fence the read as well as publication against saves, including cancelled commits.
        let _mutation = self.mutation.lock().await;
        let provider = self
            .providers
            .require(provider_kind)
            .map_err(|error| map_provider_error(error, "outbound user-agent"))?;
        let current = match provider.outbound_user_agent() {
            Ok(view) => view,
            Err(error) if error.kind() == ProviderAdminErrorKind::Unsupported => return Ok(()),
            Err(error) => return Err(map_provider_error(error, "outbound user-agent")),
        };
        let selection = self
            .store
            .load_user_agent_override(provider_kind)
            .await
            .map_err(|error| map_store_error(error, "outbound user-agent"))?
            .normalized();
        if current.selection != selection {
            provider
                .apply_outbound_user_agent(selection)
                .map_err(|error| map_provider_error(error, "outbound user-agent"))?;
        }
        Ok(())
    }
}

#[async_trait]
impl OutboundUserAgentService for DefaultOutboundUserAgentService {
    fn load(&self, provider: &ProviderKind) -> Result<OutboundUserAgentView, AdminError> {
        self.providers
            .require(provider)
            .and_then(|provider| provider.outbound_user_agent())
            .map_err(|error| map_provider_error(error, "outbound user-agent"))
    }

    fn preview(
        &self,
        provider: &ProviderKind,
        selection: &ProviderUserAgentOverride,
    ) -> Result<OutboundUserAgentView, AdminError> {
        let selection = selection.clone().normalized();
        self.providers
            .require(provider)
            .and_then(|provider| provider.preview_outbound_user_agent(&selection))
            .map_err(|error| map_provider_error(error, "outbound user-agent"))
    }

    async fn replace(
        &self,
        context: &MutationContext,
        provider_kind: &ProviderKind,
        selection: ProviderUserAgentOverride,
    ) -> Result<OutboundUserAgentView, AdminError> {
        // Serialize commit/publication so an older admin save cannot overtake a newer one.
        let _mutation = self.mutation.lock().await;
        let selection = selection.normalized();
        let provider = self
            .providers
            .require(provider_kind)
            .map_err(|error| map_provider_error(error, "outbound user-agent"))?;
        let selection = provider
            .preview_outbound_user_agent(&selection)
            .map_err(|error| map_provider_error(error, "outbound user-agent"))?
            .selection;
        let revision = self
            .store
            .replace_user_agent_override(provider_kind, selection.clone(), context)
            .await
            .map_err(|error| map_store_error(error, "outbound user-agent"))?;
        let view = provider
            .apply_outbound_user_agent(selection)
            .map_err(|error| map_provider_error(error, "outbound user-agent"))?;
        publish_committed(self.snapshot.as_ref(), revision).await?;
        Ok(view)
    }
}
