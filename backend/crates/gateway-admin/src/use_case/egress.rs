use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use gateway_core::{
    provider_ports::egress::{
        ProviderEgressAddress, ProviderEgressConfig, is_valid_source_address,
    },
    routing::ProviderKind,
    runtime::SnapshotControl,
};

use super::{map_store_error, publish_committed};
use crate::{
    model::{
        AdminError, MutationContext,
        egress::{ProviderEgressMutation, ReplaceProviderEgress, SetProviderAccountEgress},
    },
    ports::{provider::ProviderAdminRegistry, store::ProviderEgressStore},
};

const MAX_ADDRESSES: usize = 4096;

#[async_trait]
pub trait ProviderEgressService: Send + Sync {
    async fn load(&self) -> Result<ProviderEgressConfig, AdminError>;
    async fn replace(
        &self,
        command: ReplaceProviderEgress,
        context: &MutationContext,
    ) -> Result<ProviderEgressMutation, AdminError>;
    async fn set_account(
        &self,
        command: SetProviderAccountEgress,
        context: &MutationContext,
    ) -> Result<ProviderEgressMutation, AdminError>;
}

pub(crate) struct DefaultProviderEgressService {
    store: Option<Arc<dyn ProviderEgressStore>>,
    providers: ProviderAdminRegistry,
    snapshot: Arc<dyn SnapshotControl>,
}

impl DefaultProviderEgressService {
    pub(crate) fn new(
        store: Option<Arc<dyn ProviderEgressStore>>,
        providers: ProviderAdminRegistry,
        snapshot: Arc<dyn SnapshotControl>,
    ) -> Self {
        Self {
            store,
            providers,
            snapshot,
        }
    }

    fn store(&self) -> Result<&dyn ProviderEgressStore, AdminError> {
        self.store
            .as_deref()
            .ok_or_else(|| AdminError::unavailable("IPv6 出口存储未初始化"))
    }

    async fn publish(&self, mutation: &ProviderEgressMutation) -> Result<(), AdminError> {
        let provider = ProviderKind::new("openai".to_owned())
            .map_err(|_| AdminError::internal("内置 Provider 类型不合法"))?;
        self.providers
            .egress_configuration_changed(&provider)
            .await
            .map_err(|_| AdminError::unavailable("Provider IPv6 出口状态刷新失败"))?;
        publish_committed(self.snapshot.as_ref(), mutation.config_revision).await
    }
}

#[async_trait]
impl ProviderEgressService for DefaultProviderEgressService {
    async fn load(&self) -> Result<ProviderEgressConfig, AdminError> {
        self.store()?
            .load()
            .await
            .map_err(|error| map_store_error(error, "provider IPv6 egress"))
    }

    async fn replace(
        &self,
        command: ReplaceProviderEgress,
        context: &MutationContext,
    ) -> Result<ProviderEgressMutation, AdminError> {
        validate_addresses(&command.addresses)?;
        let enabled = command
            .addresses
            .iter()
            .filter(|address| address.enabled)
            .map(|address| address.address)
            .collect::<Vec<_>>();
        let provider = ProviderKind::new("openai".to_owned())
            .map_err(|_| AdminError::internal("内置 Provider 类型不合法"))?;
        if !enabled.is_empty() {
            self.providers
                .validate_egress_sources(&provider, &enabled)
                .map_err(|_| AdminError::invalid("IPv6 出口地址无法绑定到本机"))?;
        }
        let mutation = self
            .store()?
            .replace(command, context)
            .await
            .map_err(|error| map_store_error(error, "provider IPv6 egress"))?;
        self.publish(&mutation).await?;
        Ok(mutation)
    }

    async fn set_account(
        &self,
        command: SetProviderAccountEgress,
        context: &MutationContext,
    ) -> Result<ProviderEgressMutation, AdminError> {
        let config = self.load().await?;
        if config.revision != command.expected_revision.get() {
            return Err(AdminError::conflict("IPv6 配置已变化，请刷新后重试"));
        }
        if command.mode.unwrap_or(config.default_mode).is_active() {
            let enabled = config
                .addresses
                .iter()
                .filter(|address| address.enabled)
                .map(|address| address.address)
                .collect::<Vec<_>>();
            let provider = ProviderKind::new("openai".to_owned())
                .map_err(|_| AdminError::internal("内置 Provider 类型不合法"))?;
            if !enabled.is_empty() {
                self.providers
                    .validate_egress_sources(&provider, &enabled)
                    .map_err(|_| AdminError::invalid("IPv6 出口地址无法绑定到本机"))?;
            }
        }
        let mutation = self
            .store()?
            .set_account(command, context)
            .await
            .map_err(|error| map_store_error(error, "provider account IPv6 egress"))?;
        self.publish(&mutation).await?;
        Ok(mutation)
    }
}

fn validate_addresses(addresses: &[ProviderEgressAddress]) -> Result<(), AdminError> {
    if addresses.len() > MAX_ADDRESSES {
        return Err(AdminError::invalid("IPv6 地址池最多支持 4096 个地址"));
    }
    let mut ids = BTreeSet::new();
    let mut values = BTreeSet::new();
    for address in addresses {
        if address.id.is_empty()
            || address.id.len() > 128
            || !address
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || !ids.insert(&address.id)
            || !values.insert(address.address)
            || !is_valid_source_address(address.address)
        {
            return Err(AdminError::invalid("IPv6 地址必须是有效且不重复的单播地址"));
        }
    }
    Ok(())
}
