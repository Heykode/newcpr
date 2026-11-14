//! OpenAI 管理资源的中立 ProviderAdmin 委托。

use std::sync::Arc;

use async_trait::async_trait;
use gateway_core::{account::ProviderAccountId, runtime::SnapshotControl};

use crate::{
    model::{
        AdminError,
        provider_credentials::{
            AuthorizationStarted, CompleteAuthorization, CredentialDeletion,
            CredentialDeletionResult, CredentialDetails, CredentialImportCommit,
            CredentialImportResult, CredentialListQuery, CredentialMutationResult, CredentialPage,
            ImportCredentials, PrepareCredentialImport, PrepareCredentialRotation,
            ProviderQuotaRequest, RotateCredential, StartAuthorization,
        },
    },
    ports::{provider::ProviderAdmin, store::AccountStore},
};

use super::{
    commit_authorization, commit_credential_rotation, delete_credentials, map_provider_error,
    map_store_error, pending_authorization, publish_committed, required_credential,
    validate_authorization_commit, validate_prepared_import, validate_prepared_rotation,
};

/// OpenAI 固定管理路由消费的服务。
#[async_trait]
pub trait OpenAiService: Send + Sync {
    async fn import_excel_document(
        &self,
        _command: ImportCredentials,
        _target: Option<crate::model::accounts::ExcelCredentialImportTarget>,
    ) -> Result<Vec<crate::model::accounts::AccountRecord>, AdminError> {
        Err(AdminError::invalid(
            "Excel credential import is unavailable",
        ))
    }

    async fn start_excel_authorization(
        &self,
        _command: StartAuthorization,
    ) -> Result<AuthorizationStarted, AdminError> {
        Err(AdminError::invalid("Excel authorization is unavailable"))
    }

    async fn complete_excel_authorization(
        &self,
        _command: CompleteAuthorization,
    ) -> Result<Vec<crate::model::accounts::AccountRecord>, AdminError> {
        Err(AdminError::invalid("Excel authorization is unavailable"))
    }

    async fn refresh_excel_credential(
        &self,
        _account_id: &ProviderAccountId,
    ) -> Result<(), AdminError> {
        Err(AdminError::invalid(
            "Excel credential refresh is unavailable",
        ))
    }

    async fn excel_models(
        &self,
        _account_id: &ProviderAccountId,
    ) -> Result<crate::model::provider_credentials::ProviderModels, AdminError> {
        Err(AdminError::invalid("Excel model catalog is unavailable"))
    }

    async fn export_excel_credential(
        &self,
        _account_id: &ProviderAccountId,
    ) -> Result<crate::model::provider_credentials::ProviderDocument, AdminError> {
        Err(AdminError::invalid(
            "Excel credential export is unavailable",
        ))
    }

    async fn list(&self, query: CredentialListQuery) -> Result<CredentialPage, AdminError>;
    async fn details(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<CredentialDetails, AdminError>;
    async fn import_document(
        &self,
        command: ImportCredentials,
    ) -> Result<CredentialImportResult, AdminError>;
    async fn import_new_document(
        &self,
        command: ImportCredentials,
    ) -> Result<CredentialImportResult, AdminError>;
    async fn start_authorization(
        &self,
        command: StartAuthorization,
    ) -> Result<AuthorizationStarted, AdminError>;
    async fn complete_authorization(
        &self,
        command: CompleteAuthorization,
    ) -> Result<CredentialMutationResult, AdminError>;
    async fn rotate(
        &self,
        command: RotateCredential,
    ) -> Result<CredentialMutationResult, AdminError>;
    async fn delete(
        &self,
        command: CredentialDeletion,
    ) -> Result<CredentialDeletionResult, AdminError>;
}

pub(crate) struct DefaultOpenAiService {
    provider: Arc<dyn ProviderAdmin>,
    accounts: Arc<dyn AccountStore>,
    proxies: Arc<dyn crate::ports::proxy::ProxyStore>,
    snapshot: Arc<dyn SnapshotControl>,
}

impl DefaultOpenAiService {
    #[must_use]
    pub(crate) fn new(
        provider: Arc<dyn ProviderAdmin>,
        accounts: Arc<dyn AccountStore>,
        proxies: Arc<dyn crate::ports::proxy::ProxyStore>,
        snapshot: Arc<dyn SnapshotControl>,
    ) -> Self {
        Self {
            provider,
            accounts,
            proxies,
            snapshot,
        }
    }

    fn observe_initial_quotas(&self, account_ids: &[ProviderAccountId], request_id: &str) {
        observe_account_quotas(&self.provider, account_ids, request_id);
    }
    async fn import(
        &self,
        command: ImportCredentials,
        require_new: bool,
    ) -> Result<CredentialImportResult, AdminError> {
        let context = command.context;
        let proxy_reservation = super::import_proxy_binding(
            self.proxies.as_ref(),
            command.outbound_proxy_id.as_deref(),
        )
        .await?;
        let outbound_proxy = proxy_reservation
            .as_ref()
            .map(|reservation| reservation.binding.clone());
        let prepared = self
            .provider
            .prepare_import(PrepareCredentialImport {
                default_outbound_proxy: outbound_proxy
                    .as_ref()
                    .map(|binding| binding.proxy.clone()),
                document: command.document,
            })
            .await
            .map_err(|error| map_provider_error(error, "OpenAI credential import"))?;
        validate_prepared_import(
            self.provider.provider_kind(),
            &prepared,
            "OpenAI credential import",
        )?;
        let command = CredentialImportCommit {
            outbound_proxy,
            prepared,
            settings: command.settings,
        };
        let result = if require_new {
            self.accounts
                .commit_new_credential_import(command, &context)
                .await
        } else {
            self.accounts
                .commit_credential_import(command, &context)
                .await
        }
        .map_err(|error| map_store_error(error, "OpenAI credential import"))?;
        drop(proxy_reservation);
        self.provider
            .account_facts_changed(&result.credential_ids)
            .await;
        publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
        // 导入完成后立即做一次观察；失败不回滚已经提交的 credential。
        self.observe_initial_quotas(&result.credential_ids, &context.request_id);
        Ok(result)
    }
}

#[async_trait]
impl OpenAiService for DefaultOpenAiService {
    async fn import_excel_document(
        &self,
        command: ImportCredentials,
        target: Option<crate::model::accounts::ExcelCredentialImportTarget>,
    ) -> Result<Vec<crate::model::accounts::AccountRecord>, AdminError> {
        let reservation = super::import_proxy_binding(
            self.proxies.as_ref(),
            command.outbound_proxy_id.as_deref(),
        )
        .await?;
        let prepared = self
            .provider
            .prepare_excel_import(PrepareCredentialImport {
                default_outbound_proxy: reservation
                    .as_ref()
                    .map(|value| value.binding.proxy.clone()),
                document: command.document,
            })
            .await
            .map_err(|error| map_provider_error(error, "Excel credential import"))?;
        validate_prepared_import(
            self.provider.provider_kind(),
            &prepared,
            "Excel credential import",
        )?;
        if prepared.credentials.len() != 1 {
            return Err(AdminError::invalid(
                "Excel import must contain exactly one credential",
            ));
        }
        let credential = prepared
            .credentials
            .into_iter()
            .next()
            .ok_or_else(|| AdminError::invalid("Excel credential is missing"))?;
        if credential.authentication_kind != "excel_oauth" {
            return Err(AdminError::invalid(
                "Independent Excel authorization is required",
            ));
        }
        let result = self
            .accounts
            .commit_excel_credential_import(credential, target, command.settings, &command.context)
            .await
            .map_err(|error| map_store_error(error, "Excel credential import"))?;
        drop(reservation);
        let id = ProviderAccountId::new(result.account.id.clone())
            .map_err(|_| AdminError::invalid("Invalid account identity"))?;
        self.provider.account_facts_changed(&[id]).await;
        publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
        Ok(vec![result.account])
    }

    async fn start_excel_authorization(
        &self,
        command: StartAuthorization,
    ) -> Result<AuthorizationStarted, AdminError> {
        let pending = pending_authorization(
            self.accounts.as_ref(),
            self.proxies.as_ref(),
            self.provider.provider_kind(),
            &command,
            "Excel credential",
        )
        .await?;
        self.provider
            .start_excel_authorization(pending)
            .await
            .map_err(|error| map_provider_error(error, "Excel authorization"))
    }

    async fn complete_excel_authorization(
        &self,
        mut command: CompleteAuthorization,
    ) -> Result<Vec<crate::model::accounts::AccountRecord>, AdminError> {
        let context = command.context.clone();
        let settings = command.settings.take();
        let prepared = self
            .provider
            .complete_excel_authorization(command)
            .await
            .map_err(|error| map_provider_error(error, "Excel authorization"))?;
        let valid = prepared.pending.provider_kind() == self.provider.provider_kind()
            && prepared.pending.owner_binding().matches_context(&context)
            && prepared.credential.provider_kind == *self.provider.provider_kind()
            && prepared.credential.authentication_kind == "excel_oauth";
        if !valid {
            prepared.authorization_guard.abort().await?;
            return Err(AdminError::conflict(
                "Excel authorization ownership does not match",
            ));
        }
        let result = self
            .accounts
            .commit_excel_credential_import(
                prepared.credential,
                prepared.target,
                settings,
                &context,
            )
            .await;
        match result {
            Ok(result) => {
                prepared.authorization_guard.commit().await?;
                let id = ProviderAccountId::new(result.account.id.clone())
                    .map_err(|_| AdminError::invalid("Invalid account identity"))?;
                self.provider.account_facts_changed(&[id]).await;
                publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
                Ok(vec![result.account])
            }
            Err(error) => {
                prepared.authorization_guard.abort().await?;
                Err(map_store_error(error, "Excel authorization"))
            }
        }
    }

    async fn refresh_excel_credential(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<(), AdminError> {
        self.provider
            .refresh_excel_credential(account_id)
            .await
            .map_err(|error| map_provider_error(error, "Excel refresh"))?;
        self.provider
            .account_facts_changed(std::slice::from_ref(account_id))
            .await;
        Ok(())
    }

    async fn excel_models(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<crate::model::provider_credentials::ProviderModels, AdminError> {
        self.provider
            .excel_models(account_id)
            .await
            .map_err(|error| map_provider_error(error, "Excel models"))
    }

    async fn export_excel_credential(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<crate::model::provider_credentials::ProviderDocument, AdminError> {
        self.provider
            .export_excel_credential(account_id)
            .await
            .map_err(|error| map_provider_error(error, "Excel export"))
    }

    async fn list(&self, query: CredentialListQuery) -> Result<CredentialPage, AdminError> {
        self.accounts
            .list_credentials(self.provider.provider_kind(), query)
            .await
            .map_err(|error| map_store_error(error, "OpenAI credential"))
    }

    async fn details(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<CredentialDetails, AdminError> {
        required_credential(
            self.accounts.as_ref(),
            self.provider.provider_kind(),
            account_id,
            "OpenAI credential",
        )
        .await
    }

    async fn import_document(
        &self,
        command: ImportCredentials,
    ) -> Result<CredentialImportResult, AdminError> {
        self.import(command, false).await
    }

    async fn import_new_document(
        &self,
        command: ImportCredentials,
    ) -> Result<CredentialImportResult, AdminError> {
        self.import(command, true).await
    }

    async fn start_authorization(
        &self,
        command: StartAuthorization,
    ) -> Result<AuthorizationStarted, AdminError> {
        let pending = pending_authorization(
            self.accounts.as_ref(),
            self.proxies.as_ref(),
            self.provider.provider_kind(),
            &command,
            "OpenAI credential",
        )
        .await?;
        self.provider
            .start_authorization(pending)
            .await
            .map_err(|error| map_provider_error(error, "OpenAI authorization"))
    }

    async fn complete_authorization(
        &self,
        mut command: CompleteAuthorization,
    ) -> Result<CredentialMutationResult, AdminError> {
        let context = command.context.clone();
        let settings = command.settings.take();
        let prepared = self
            .provider
            .complete_authorization(command)
            .await
            .map_err(|error| map_provider_error(error, "OpenAI authorization"))?;
        let prepared = validate_authorization_commit(
            self.provider.provider_kind(),
            &context,
            prepared,
            "OpenAI authorization",
        )
        .await?;
        let result = commit_authorization(
            self.accounts.as_ref(),
            prepared,
            settings,
            &context,
            "OpenAI authorization",
        )
        .await?;
        self.provider
            .account_facts_changed(std::slice::from_ref(&result.account_id))
            .await;
        publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
        self.observe_initial_quotas(
            std::slice::from_ref(&result.account_id),
            &context.request_id,
        );
        Ok(result)
    }

    async fn rotate(
        &self,
        command: RotateCredential,
    ) -> Result<CredentialMutationResult, AdminError> {
        let context = command.mutation.context;
        let account_id = command.mutation.account_id;
        let details = required_credential(
            self.accounts.as_ref(),
            self.provider.provider_kind(),
            &account_id,
            "OpenAI credential rotation",
        )
        .await?;
        let account = details.credential;
        let prepared = self
            .provider
            .prepare_rotation(PrepareCredentialRotation {
                account: account.clone(),
                provider_material: command.provider_material,
            })
            .await
            .map_err(|error| map_provider_error(error, "OpenAI credential rotation"))?;
        validate_prepared_rotation(&account, &prepared, "OpenAI credential rotation")?;
        let result = commit_credential_rotation(
            self.accounts.as_ref(),
            prepared,
            &context,
            "OpenAI credential rotation",
            None,
        )
        .await?;
        self.provider
            .account_facts_changed(std::slice::from_ref(&result.account_id))
            .await;
        publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
        Ok(result)
    }

    async fn delete(
        &self,
        command: CredentialDeletion,
    ) -> Result<CredentialDeletionResult, AdminError> {
        let result = delete_credentials(
            self.accounts.as_ref(),
            self.provider.as_ref(),
            command,
            "OpenAI credential",
        )
        .await?;
        publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
        Ok(result)
    }
}

pub(super) fn observe_account_quotas(
    provider: &Arc<dyn ProviderAdmin>,
    account_ids: &[ProviderAccountId],
    request_id: &str,
) {
    let provider = Arc::clone(provider);
    let account_ids = account_ids.to_vec();
    let request_id = request_id.to_owned();
    tokio::spawn(async move {
        for account_id in account_ids {
            if let Err(error) = provider
                .quota(ProviderQuotaRequest {
                    account_id: account_id.clone(),
                    refresh: true,
                    rolling_usage: None,
                })
                .await
            {
                tracing::warn!(
                    request_id,
                    account_id = %account_id.as_str(),
                    quota_error = ?error.kind(),
                    "OpenAI account quota observation failed"
                );
            }
        }
    });
}
