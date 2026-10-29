//! Admin query service 使用的账号运行态组合 adapter。

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::future::join_all;
use gateway_admin::{
    model::accounts::AccountRuntimeSnapshot,
    ports::store::{AccountRuntimeStore, AdminStoreResult},
};

use super::{
    CredentialCooldownRepository as _, CredentialLeaseRepository as _,
    RedisClientAdmissionRepository, RedisCredentialCooldownRepository,
    RedisCredentialLeaseRepository,
};

/// 只组合可丢失 Redis 事实；不持有 PostgreSQL，也不执行状态投影。
#[derive(Clone)]
pub struct RedisAdminAccountRuntimeStore {
    cooldowns: RedisCredentialCooldownRepository,
    leases: RedisCredentialLeaseRepository,
    clients: RedisClientAdmissionRepository,
}

impl RedisAdminAccountRuntimeStore {
    #[must_use]
    pub const fn new(
        cooldowns: RedisCredentialCooldownRepository,
        leases: RedisCredentialLeaseRepository,
        clients: RedisClientAdmissionRepository,
    ) -> Self {
        Self {
            cooldowns,
            leases,
            clients,
        }
    }
}

#[async_trait]
impl AccountRuntimeStore for RedisAdminAccountRuntimeStore {
    async fn account_protections(
        &self,
        accounts: &[(String, gateway_admin::model::Revision)],
    ) -> AdminStoreResult<BTreeMap<String, gateway_core::provider_ports::AccountProtectionState>>
    {
        let mut result = BTreeMap::new();
        for chunk in accounts.chunks(super::ACCOUNT_STATE_READ_CONCURRENCY) {
            let reads = chunk.iter().map(|(id, revision)| async {
                let revision = gateway_core::account::CredentialRevision::new(revision.get())
                    .map_err(|_| {
                        crate::admin_store_error(
                            "account protection",
                            super::credential_cooldown::invalid("revision"),
                        )
                    })?;
                self.cooldowns
                    .read_account_protection(id, Some(revision))
                    .await
                    .map_err(|e| crate::admin_store_error("account protection", e))
            });
            for state in join_all(reads).await {
                if let Some(state) = state? {
                    result.insert(state.account_id.to_string(), state);
                }
            }
        }
        Ok(result)
    }

    async fn due_protections(
        &self,
    ) -> AdminStoreResult<Vec<gateway_core::provider_ports::AccountProtectionState>> {
        self.cooldowns
            .due_account_protections()
            .await
            .map_err(|e| crate::admin_store_error("claim protection", e))
    }

    async fn finish_protection(
        &self,
        expected: &gateway_core::provider_ports::AccountProtectionState,
        postpone: Option<chrono::DateTime<chrono::Utc>>,
    ) -> AdminStoreResult<bool> {
        self.cooldowns
            .finish_protection(expected.account_id.as_str(), expected, postpone)
            .await
            .map_err(|e| crate::admin_store_error("finish protection", e))
    }

    async fn clear_protection(
        &self,
        account: &str,
        revision: gateway_admin::model::Revision,
    ) -> AdminStoreResult<bool> {
        let revision = crate::Revision::new(revision.get())
            .map_err(|e| crate::admin_store_error("protection revision", e))?;
        self.cooldowns
            .clear_protection(account, revision)
            .await
            .map_err(|e| crate::admin_store_error("clear protection", e))
    }

    async fn concurrency(
        &self,
        account_id: &str,
        pool: gateway_admin::model::accounts::AccountConcurrencyPool,
        page: u32,
        page_size: u32,
    ) -> AdminStoreResult<Option<gateway_admin::model::accounts::AccountConcurrencyPage>> {
        self.leases
            .concurrency_page(account_id, pool, page, page_size)
            .await
            .map(Some)
            .map_err(|error| crate::admin_store_error("account concurrency", error))
    }

    async fn client_in_flight(
        &self,
        client_key_ids: &[String],
    ) -> AdminStoreResult<Option<BTreeMap<String, u64>>> {
        Ok(self.clients.active_counts(client_key_ids).await.ok())
    }

    async fn active_rate_limits(&self) -> AdminStoreResult<AccountRuntimeSnapshot> {
        let mut runtime = self
            .cooldowns
            .active_cooldowns()
            .await
            .map_err(|error| crate::admin_store_error("account runtime", error))?;
        runtime.error_protections = self
            .cooldowns
            .active_protections()
            .await
            .map_err(|error| crate::admin_store_error("account protection", error))?;
        Ok(runtime)
    }

    async fn account_runtime(
        &self,
        account_ids: &[String],
    ) -> AdminStoreResult<AccountRuntimeSnapshot> {
        let mut rate_limited_until = BTreeMap::new();
        for ids in account_ids.chunks(super::ACCOUNT_STATE_READ_CONCURRENCY) {
            let reads = ids.iter().map(|account_id| async move {
                self.cooldowns
                    .read_credential_cooldown(account_id)
                    .await
                    .map(|cooldown| {
                        cooldown.map(|cooldown| (account_id.clone(), cooldown.cooldown_until))
                    })
            });
            for result in join_all(reads).await {
                if let Some((account_id, until)) =
                    result.map_err(|error| crate::admin_store_error("account runtime", error))?
                {
                    rate_limited_until.insert(account_id, until);
                }
            }
        }
        let in_flight = self
            .leases
            .credential_runtime_signals(account_ids)
            .await
            .ok()
            .map(|signals| {
                signals
                    .into_iter()
                    .map(|signal| (signal.resource_id, u64::from(signal.in_flight)))
                    .collect()
            });
        let mut error_protections = BTreeMap::new();
        for chunk in account_ids.chunks(super::ACCOUNT_STATE_READ_CONCURRENCY) {
            for read in join_all(
                chunk
                    .iter()
                    .map(|id| self.cooldowns.read_account_protection(id, None)),
            )
            .await
            {
                if let Some(state) =
                    read.map_err(|error| crate::admin_store_error("account protection", error))?
                {
                    error_protections.insert(state.account_id.to_string(), state);
                }
            }
        }
        Ok(AccountRuntimeSnapshot {
            error_protections,
            rate_limited_until,
            in_flight,
        })
    }
}
