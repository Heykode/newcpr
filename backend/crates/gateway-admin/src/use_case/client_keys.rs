//! Client API Key 管理用例。

use crate::model::client_keys::{
    BatchUpdateClientKeys, BudgetReferenceAccountPage, ClientKeyBudgetWindow,
    ClientKeyBudgetWindowPeriod, ClientKeyRecord, SetClientKeyBudgetWindow,
};
use crate::ports::store::{AdminStoreError, AdminStoreErrorKind, BatchClientKeyProfileMerger};
use gateway_core::engine::budget::ClientBudgetStatus;
use gateway_core::routing::ProviderKind;
use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use gateway_core::policy::ClientApiKeyId;
use gateway_core::runtime::SnapshotControl;
use rand_core::{OsRng, RngCore as _};
use uuid::Uuid;

use crate::{
    model::{
        AdminError, MutationContext,
        client_keys::{
            ClientKeyCursorValue, ClientKeyListQuery, ClientKeyMutation, ClientKeyPage,
            ClientKeySecret, ClientKeySortField, CreateClientKey, CreatedClientKey,
            DeleteClientKey, NewClientKey, ResetClientKeyBudget, SetClientKeyEnabled,
            UpdateClientKey,
        },
    },
    ports::store::ClientKeyStore,
};

use super::{map_store_error, publish_committed};

/// API 消费的 Client Key 管理服务。
#[async_trait]
pub trait ClientKeyService: Send + Sync {
    async fn budget_reference_accounts(
        &self,
        id: &gateway_core::policy::ClientApiKeyId,
        search: &str,
        after: &str,
    ) -> Result<BudgetReferenceAccountPage, AdminError>;

    async fn budget_window(
        &self,
        id: &ClientApiKeyId,
        period: ClientKeyBudgetWindowPeriod,
    ) -> Result<ClientKeyBudgetWindow, AdminError>;

    async fn set_budget_window(
        &self,
        context: &MutationContext,
        command: SetClientKeyBudgetWindow,
    ) -> Result<u64, AdminError>;

    async fn get(&self, id: &ClientApiKeyId) -> Result<ClientKeyRecord, AdminError>;

    async fn budget(&self, id: &ClientApiKeyId) -> Result<ClientBudgetStatus, AdminError>;

    async fn batch_update(
        &self,
        context: &MutationContext,
        command: BatchUpdateClientKeys,
    ) -> Result<crate::model::Revision, AdminError>;

    async fn reset_budget(
        &self,
        context: &MutationContext,
        command: ResetClientKeyBudget,
    ) -> Result<(), AdminError>;

    async fn list(&self, query: ClientKeyListQuery) -> Result<ClientKeyPage, AdminError>;
    async fn reveal(&self, id: &ClientApiKeyId) -> Result<ClientKeySecret, AdminError>;
    async fn create(
        &self,
        context: &MutationContext,
        command: CreateClientKey,
    ) -> Result<CreatedClientKey, AdminError>;
    async fn update(
        &self,
        context: &MutationContext,
        command: UpdateClientKey,
    ) -> Result<ClientKeyMutation, AdminError>;
    async fn set_enabled(
        &self,
        context: &MutationContext,
        command: SetClientKeyEnabled,
    ) -> Result<ClientKeyMutation, AdminError>;
    async fn delete(
        &self,
        context: &MutationContext,
        command: DeleteClientKey,
    ) -> Result<ClientKeyMutation, AdminError>;
}

pub(crate) struct DefaultClientKeyService {
    providers: crate::ports::provider::ProviderAdminRegistry,
    store: Arc<dyn ClientKeyStore>,
    snapshot: Arc<dyn SnapshotControl>,
}

impl DefaultClientKeyService {
    fn batch_profile_merger(
        &self,
        command: &BatchUpdateClientKeys,
    ) -> Result<BatchClientKeyProfileMerger, AdminError> {
        let mut kinds = command
            .request_profile_override_updates
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let seed_kind = ProviderKind::new("openai").expect("static Provider kind is valid");
        if command.installation_seed.is_some() {
            kinds.insert(seed_kind.clone());
        }
        let mut changes = Vec::with_capacity(kinds.len());
        for kind in kinds {
            let provider = self
                .providers
                .require(&kind)
                .map_err(|error| super::map_provider_error(error, "client profile"))?;
            let update = command.request_profile_override_updates.get(&kind).cloned();
            let seed = if kind == seed_kind {
                command.installation_seed
            } else {
                None
            };
            // 先校验输入，行锁内只依据各把密钥的现值完成合并
            if let Some(Some(profile)) = update.as_ref() {
                provider
                    .merge_batch_key_profile(None, update.as_ref(), seed)
                    .and_then(|_| provider.validate_key_client_profile(profile))
                    .map_err(|error| super::map_provider_error(error, "client profile"))?;
            }
            changes.push((kind, provider, update, seed));
        }
        Ok(Box::new(move |profiles| {
            for (kind, provider, update, seed) in &changes {
                let next = provider
                    .merge_batch_key_profile(profiles.get(kind), update.as_ref(), *seed)
                    .map_err(|error| {
                        AdminStoreError::new(
                            AdminStoreErrorKind::Invalid,
                            "client profile",
                            "invalid stored client profile",
                        )
                        .with_source(error)
                    })?;
                if let Some(profile) = next {
                    profiles.insert(kind.clone(), profile);
                } else {
                    profiles.remove(kind);
                }
            }
            Ok(())
        }))
    }

    #[must_use]
    pub(crate) fn new(
        store: Arc<dyn ClientKeyStore>,
        snapshot: Arc<dyn SnapshotControl>,
        providers: crate::ports::provider::ProviderAdminRegistry,
    ) -> Self {
        Self {
            store,
            snapshot,
            providers,
        }
    }
}

#[async_trait]
impl ClientKeyService for DefaultClientKeyService {
    async fn budget_reference_accounts(
        &self,
        id: &gateway_core::policy::ClientApiKeyId,
        search: &str,
        after: &str,
    ) -> Result<BudgetReferenceAccountPage, AdminError> {
        self.store
            .budget_reference_accounts(id, search.trim(), after)
            .await
            .map_err(|error| map_store_error(error, "client API key"))
    }

    async fn budget_window(
        &self,
        id: &ClientApiKeyId,
        period: ClientKeyBudgetWindowPeriod,
    ) -> Result<ClientKeyBudgetWindow, AdminError> {
        self.store
            .client_key_budget_window(id, period)
            .await
            .map_err(|error| map_store_error(error, "client API key"))
    }

    async fn set_budget_window(
        &self,
        context: &MutationContext,
        command: SetClientKeyBudgetWindow,
    ) -> Result<u64, AdminError> {
        if command
            .window
            .is_some_and(|window| window.starts_at >= window.ends_at)
        {
            return Err(AdminError::invalid("预算窗口结束时间必须晚于开始时间"));
        }
        self.store
            .set_client_key_budget_window(command, context)
            .await
            .map_err(|error| map_store_error(error, "client API key"))
    }

    async fn get(&self, id: &ClientApiKeyId) -> Result<ClientKeyRecord, AdminError> {
        self.store
            .get_client_key(id)
            .await
            .map_err(|error| map_store_error(error, "client API key"))?
            .ok_or_else(|| AdminError::not_found("API Key 不存在"))
    }

    async fn budget(&self, id: &ClientApiKeyId) -> Result<ClientBudgetStatus, AdminError> {
        self.store
            .client_key_budget(id)
            .await
            .map_err(|error| map_store_error(error, "client API key"))?
            .ok_or_else(|| AdminError::not_found("API Key 不存在"))
    }

    async fn batch_update(
        &self,
        context: &MutationContext,
        command: BatchUpdateClientKeys,
    ) -> Result<crate::model::Revision, AdminError> {
        if command.ids.is_empty()
            || command.ids.len() > 1000
            || command
                .ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != command.ids.len()
        {
            return Err(AdminError::invalid("请选择 1 至 1000 个不同的 API Key"));
        }
        if !command.has_changes() {
            return Err(AdminError::invalid("请至少选择一项要修改的密钥设置"));
        }
        if let Some(groups) = &command.group_ids
            && (groups.len() > 1000 || groups.iter().collect::<BTreeSet<_>>().len() != groups.len())
        {
            return Err(AdminError::invalid("API Key 分组不合法"));
        }
        let merge_profiles = self.batch_profile_merger(&command)?;
        let revision = self
            .store
            .batch_update_client_keys(command, merge_profiles, context)
            .await
            .map_err(|error| map_store_error(error, "client API key"))?;
        publish_committed(self.snapshot.as_ref(), revision).await?;
        Ok(revision)
    }

    async fn reset_budget(
        &self,
        context: &MutationContext,
        command: ResetClientKeyBudget,
    ) -> Result<(), AdminError> {
        self.store
            .reset_client_key_budget(command, context)
            .await
            .map_err(|error| map_store_error(error, "client API key budget"))
    }

    async fn list(&self, query: ClientKeyListQuery) -> Result<ClientKeyPage, AdminError> {
        validate_cursor(&query)?;
        self.store
            .list_client_keys(query)
            .await
            .map_err(|error| map_store_error(error, "client API key"))
    }

    async fn reveal(&self, id: &ClientApiKeyId) -> Result<ClientKeySecret, AdminError> {
        self.store
            .reveal_client_key(id)
            .await
            .map_err(|error| map_store_error(error, "client API key"))?
            .ok_or_else(|| AdminError::not_found("API Key 不存在"))
    }

    async fn create(
        &self,
        context: &MutationContext,
        command: CreateClientKey,
    ) -> Result<CreatedClientKey, AdminError> {
        for (provider, profile) in &command.request_profile_overrides {
            self.providers
                .require(provider)
                .and_then(|provider| provider.validate_key_client_profile(profile))
                .map_err(|error| super::map_provider_error(error, "client profile"))?;
        }
        let id = ClientApiKeyId::new(format!("key_{}", Uuid::now_v7().simple()))
            .map_err(|_| AdminError::internal("创建 Client API Key ID 失败"))?;
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let plaintext = format!("sk_{}", URL_SAFE_NO_PAD.encode(bytes));
        let (config_revision, record) = self
            .store
            .create_client_key(
                NewClientKey {
                    request_profile_overrides: command.request_profile_overrides,
                    id,
                    name: command.name,
                    label: command.label,
                    group_ids: command.group_ids,
                    limits: command.limits,
                    budget: command.budget,
                    plaintext: plaintext.clone(),
                },
                context,
            )
            .await
            .map_err(|error| map_store_error(error, "client API key"))?;
        publish_committed(self.snapshot.as_ref(), config_revision).await?;
        Ok(CreatedClientKey {
            config_revision,
            secret: ClientKeySecret::new(record, plaintext),
        })
    }

    async fn update(
        &self,
        context: &MutationContext,
        command: UpdateClientKey,
    ) -> Result<ClientKeyMutation, AdminError> {
        for (provider, profile) in &command.request_profile_override_updates {
            if let Some(profile) = profile {
                self.providers
                    .require(provider)
                    .and_then(|provider| provider.validate_key_client_profile(profile))
                    .map_err(|error| super::map_provider_error(error, "client profile"))?;
            }
        }
        let id = command.id.clone();
        let (config_revision, record) = self
            .store
            .update_client_key(command, context)
            .await
            .map_err(|error| map_store_error(error, "client API key"))?;
        publish_committed(self.snapshot.as_ref(), config_revision).await?;
        Ok(ClientKeyMutation {
            config_revision,
            record: Some(record),
            id,
        })
    }

    async fn set_enabled(
        &self,
        context: &MutationContext,
        command: SetClientKeyEnabled,
    ) -> Result<ClientKeyMutation, AdminError> {
        let id = command.id.clone();
        let (config_revision, record) =
            self.store
                .set_client_key_enabled(command, context)
                .await
                .map_err(|error| map_store_error(error, "client API key"))?;
        publish_committed(self.snapshot.as_ref(), config_revision).await?;
        Ok(ClientKeyMutation {
            config_revision,
            record: Some(record),
            id,
        })
    }

    async fn delete(
        &self,
        context: &MutationContext,
        command: DeleteClientKey,
    ) -> Result<ClientKeyMutation, AdminError> {
        let id = command.id.clone();
        let config_revision = self
            .store
            .delete_client_key(command, context)
            .await
            .map_err(|error| map_store_error(error, "client API key"))?;
        publish_committed(self.snapshot.as_ref(), config_revision).await?;
        Ok(ClientKeyMutation {
            config_revision,
            record: None,
            id,
        })
    }
}

fn validate_cursor(query: &ClientKeyListQuery) -> Result<(), AdminError> {
    let Some(cursor) = &query.cursor else {
        return Ok(());
    };
    if cursor.sort != query.sort {
        return Err(AdminError::invalid("分页游标与排序条件不一致"));
    }
    let matches = matches!(
        (cursor.sort.field, &cursor.value),
        (ClientKeySortField::Name, ClientKeyCursorValue::Name(value)) if !value.trim().is_empty()
    ) || matches!(
        (cursor.sort.field, &cursor.value),
        (
            ClientKeySortField::Enabled,
            ClientKeyCursorValue::Enabled(_)
        ) | (
            ClientKeySortField::CreatedAt,
            ClientKeyCursorValue::CreatedAt(_)
        ) | (
            ClientKeySortField::LastUsedAt,
            ClientKeyCursorValue::LastUsedAt(_)
        )
    );
    if matches {
        Ok(())
    } else {
        Err(AdminError::invalid("API Key 分页游标不合法"))
    }
}
