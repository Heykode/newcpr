//! Excel management never prepares, refreshes or exports Codex material.

use std::time::Duration;

use chrono::TimeDelta;
use gateway_admin::model::{
    AdminError,
    accounts::ExcelCredentialImportTarget,
    provider_credentials::{AuthorizationCommitGuard, PreparedExcelAuthorizationCommit},
};
use gateway_core::account::CredentialState;
use serde_json::json;

use super::*;
use crate::credential::excel::{
    ExcelAuthError, ExcelCredentialCodec, ExcelCredentialData, ExcelIdentity, ExcelModelCatalog,
    ExcelOAuthFlow,
};

impl OpenAiAdminProvider {
    pub(super) async fn prepare_excel_document(
        &self,
        command: PrepareCredentialImport,
    ) -> Result<PreparedCredentialImport, ProviderAdminError> {
        let mut document = command.document.into_provider_data().into_inner();
        if document.get("credentialTarget").and_then(Value::as_str) == Some("excel") {
            document = document
                .remove("document")
                .and_then(|value| value.as_object().cloned())
                .ok_or_else(|| map_error(ExcelAuthError::InvalidCredential))?;
        }
        let data = ExcelCredentialCodec::import_document(&document).map_err(map_error)?;
        data.ensure_usable().map_err(map_error)?;
        let accounts = self
            .accounts
            .list_for_provider(&self.provider_kind)
            .await
            .map_err(map_store_error)?;
        let matching: Vec<_> = accounts
            .iter()
            .filter(|account| {
                account.upstream_user_id() == Some(data.user_id.as_str())
                    && account.upstream_account_id() == Some(data.chatgpt_account_id.as_str())
            })
            .collect();
        if matching.len() > 1 {
            return Err(provider_admin_error(ProviderAdminErrorKind::Conflict));
        }
        let existing = matching.first().copied();
        let proxy = existing
            .and_then(|account| account.outbound_proxy())
            .or_else(|| {
                if existing.is_none() {
                    command.default_outbound_proxy.as_ref()
                } else {
                    None
                }
            });
        let client = self.excel.client(existing, proxy).map_err(map_error)?;
        let models = ExcelModelCatalog::fetch(&client, &data)
            .await
            .map_err(map_error)?;
        if models.is_empty() {
            return Err(map_error(ExcelAuthError::AccessDenied));
        }
        let credential = prepared_excel(
            data,
            None,
            command.default_outbound_proxy,
            &self.provider_kind,
        )?;
        Ok(PreparedCredentialImport {
            provider_kind: self.provider_kind.clone(),
            credentials: vec![credential],
        })
    }

    pub(super) async fn read_excel_models(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<ProviderModels, ProviderAdminError> {
        let account = self.account(account_id).await?;
        let data = self.excel.load(account_id).await.map_err(map_error)?;
        let client = self
            .excel
            .client(Some(&account), account.outbound_proxy())
            .map_err(map_error)?;
        let models = ExcelModelCatalog::fetch(&client, &data)
            .await
            .map_err(map_error)?;
        Ok(ProviderModels {
            models: models
                .into_iter()
                .map(|model| {
                    Ok(ProviderModel {
                        id: UpstreamModelId::new(model.id.clone())
                            .map_err(|_| map_error(ExcelAuthError::InvalidCatalog))?,
                        name: if model.label.is_empty() {
                            model.id
                        } else {
                            model.label
                        },
                    })
                })
                .collect::<Result<Vec<_>, ProviderAdminError>>()?,
            observed_at: Some(Utc::now()),
        })
    }

    pub(super) async fn start_excel_flow(
        &self,
        pending: PendingAuthorizationMutation,
    ) -> Result<AuthorizationStarted, ProviderAdminError> {
        if pending.provider_kind() != &self.provider_kind {
            return Err(provider_admin_error(ProviderAdminErrorKind::Invalid));
        }
        let port = self
            .excel_pending
            .as_ref()
            .ok_or_else(|| provider_admin_error(ProviderAdminErrorKind::Unsupported))?;
        let (identity, expected_revision) = match pending.target() {
            AuthorizationMutationTarget::Create { .. } => (None, None),
            AuthorizationMutationTarget::Reauthorize { account_id } => {
                let account = self.account(account_id).await?;
                (
                    Some(account_identity(&account)?),
                    account
                        .excel_credential()
                        .map(|metadata| metadata.revision.get()),
                )
            }
        };
        let flow = ExcelOAuthFlow::start().map_err(map_error)?;
        let flow_id = uuid::Uuid::new_v4().to_string();
        let owner = oauth_owner_ref(pending.owner_binding().owner());
        let payload = json!({"version":1,"mutation":encode_mutation(&pending),
            "identity":identity,"expected_revision":expected_revision,"expires_at":flow.expires_at,
            "state":flow.state.expose_secret(),"code_verifier":flow.code_verifier.expose_secret()});
        let stored = NewOAuthPendingFlow::try_new(
            excel_namespace()?,
            excel_binding(&flow_id)?,
            excel_binding(&owner)?,
            Duration::from_secs(600),
            OpaqueProviderData::new(
                payload
                    .as_object()
                    .cloned()
                    .ok_or_else(|| map_error(ExcelAuthError::InvalidCredential))?,
            ),
        )
        .map_err(|_| provider_admin_error(ProviderAdminErrorKind::Internal))?;
        match port
            .put_if_absent(stored)
            .await
            .map_err(|_| provider_admin_error(ProviderAdminErrorKind::Unavailable))?
        {
            OAuthPendingPutOutcome::Stored => Ok(AuthorizationStarted {
                flow_id,
                authorization_url: flow.authorization_url,
                expires_at: flow.expires_at,
            }),
            OAuthPendingPutOutcome::AlreadyExists => {
                Err(provider_admin_error(ProviderAdminErrorKind::Conflict))
            }
        }
    }

    pub(super) async fn complete_excel_flow(
        &self,
        command: CompleteAuthorization,
    ) -> Result<PreparedExcelAuthorizationCommit, ProviderAdminError> {
        let port = self
            .excel_pending
            .as_ref()
            .ok_or_else(|| provider_admin_error(ProviderAdminErrorKind::Unsupported))?;
        let namespace = excel_namespace()?;
        let flow_id = excel_binding(&command.flow_id)?;
        let owner = excel_binding(&oauth_owner_ref(
            AuthorizationOwnerBinding::from_context(&command.context).owner(),
        ))?;
        let claim = excel_binding(&uuid::Uuid::new_v4().to_string())?;
        let payload = match port
            .claim_if_owner(
                &namespace,
                &flow_id,
                &owner,
                &claim,
                Duration::from_secs(600),
            )
            .await
            .map_err(|_| provider_admin_error(ProviderAdminErrorKind::Unavailable))?
        {
            OAuthPendingClaimOutcome::Claimed(payload) => payload,
            OAuthPendingClaimOutcome::InProgress => {
                return Err(provider_admin_error(ProviderAdminErrorKind::Conflict));
            }
            OAuthPendingClaimOutcome::NotFound | OAuthPendingClaimOutcome::OwnerMismatch => {
                return Err(map_error(ExcelAuthError::InvalidCallback));
            }
        };
        let prepared = self.validate_excel_pending(payload, &command).await;
        let (pending, identity, target, flow, client) = match prepared {
            Ok(value) => value,
            Err(error) => {
                port.release_claim(&namespace, &flow_id, &owner, &claim)
                    .await
                    .map_err(|_| provider_admin_error(ProviderAdminErrorKind::Unavailable))?;
                return Err(error);
            }
        };
        // The claim outlives the remaining flow TTL, so a process crash cannot reuse the code.
        let guard = ExcelAuthorizationGuard {
            port: Arc::clone(port),
            namespace,
            flow_id,
            owner,
            claim,
        };
        let exchange = async {
            let data = flow
                .complete(
                    &client,
                    &SecretString::from(command.callback_url),
                    identity.as_ref(),
                )
                .await
                .map_err(map_error)?;
            let models = ExcelModelCatalog::fetch(&client, &data)
                .await
                .map_err(map_error)?;
            if models.is_empty() {
                return Err(map_error(ExcelAuthError::AccessDenied));
            }
            let name = match pending.target() {
                AuthorizationMutationTarget::Create { name } => Some(name.as_str()),
                _ => None,
            };
            prepared_excel(
                data,
                name,
                pending.outbound_proxy().cloned(),
                &self.provider_kind,
            )
        }
        .await;
        let credential = match exchange {
            Ok(credential) => credential,
            Err(error) => {
                guard
                    .consume()
                    .await
                    .map_err(|_| provider_admin_error(ProviderAdminErrorKind::Unavailable))?;
                return Err(error);
            }
        };
        Ok(PreparedExcelAuthorizationCommit {
            pending,
            credential,
            target,
            authorization_guard: Box::new(guard),
        })
    }

    async fn validate_excel_pending(
        &self,
        payload: OpaqueProviderData,
        command: &CompleteAuthorization,
    ) -> Result<
        (
            PendingAuthorizationMutation,
            Option<ExcelIdentity>,
            Option<ExcelCredentialImportTarget>,
            ExcelOAuthFlow,
            reqwest::Client,
        ),
        ProviderAdminError,
    > {
        let payload: ExcelPending = serde_json::from_value(Value::Object(payload.into_inner()))
            .map_err(|_| map_error(ExcelAuthError::InvalidCallback))?;
        if payload.version != 1 {
            return Err(map_error(ExcelAuthError::InvalidCallback));
        }
        let pending = decode_mutation(payload.mutation)
            .map_err(|_| map_error(ExcelAuthError::InvalidCallback))?;
        if pending.provider_kind() != &self.provider_kind
            || !pending.owner_binding().matches_context(&command.context)
        {
            return Err(map_error(ExcelAuthError::InvalidCallback));
        }
        let account = match pending.target() {
            AuthorizationMutationTarget::Create { .. } => None,
            AuthorizationMutationTarget::Reauthorize { account_id } => {
                Some(self.account(account_id).await?)
            }
        };
        let target = if let Some(account) = &account {
            if payload.identity.as_ref() != Some(&account_identity(account)?) {
                return Err(map_error(ExcelAuthError::IdentityMismatch));
            }
            if account
                .excel_credential()
                .map(|credential| credential.revision.get())
                != payload.expected_revision
            {
                return Err(provider_admin_error(ProviderAdminErrorKind::Conflict)
                    .with_public_message("Excel 凭据已变化，请重新生成授权链接"));
            }
            Some(ExcelCredentialImportTarget {
                account_id: account.id().to_string(),
                expected_revision: payload
                    .expected_revision
                    .map(CredentialRevision::new)
                    .transpose()
                    .map_err(|_| map_error(ExcelAuthError::InvalidCallback))?,
            })
        } else {
            if payload.identity.is_some() || payload.expected_revision.is_some() {
                return Err(map_error(ExcelAuthError::InvalidCallback));
            }
            None
        };
        let flow = ExcelOAuthFlow {
            state: payload.state.into(),
            code_verifier: payload.code_verifier.into(),
            authorization_url: String::new(),
            expires_at: payload.expires_at,
        };
        flow.callback_code(&command.callback_url)
            .map_err(map_error)?;
        let proxy = match &account {
            Some(account) => account.outbound_proxy(),
            None => pending.outbound_proxy(),
        };
        let client = self
            .excel
            .client(account.as_ref(), proxy)
            .map_err(map_error)?;
        Ok((pending, payload.identity, target, flow, client))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExcelPending {
    version: u32,
    mutation: Map<String, Value>,
    identity: Option<ExcelIdentity>,
    expected_revision: Option<u64>,
    expires_at: DateTime<Utc>,
    state: String,
    code_verifier: String,
}

struct ExcelAuthorizationGuard {
    port: Arc<dyn OAuthPendingFlowPort>,
    namespace: ProviderKind,
    flow_id: OAuthPendingBinding,
    owner: OAuthPendingBinding,
    claim: OAuthPendingBinding,
}

impl ExcelAuthorizationGuard {
    async fn consume(&self) -> Result<(), AdminError> {
        match self
            .port
            .consume_claim(&self.namespace, &self.flow_id, &self.owner, &self.claim)
            .await
        {
            Ok(OAuthPendingConsumeOutcome::Consumed) => Ok(()),
            _ => Err(AdminError::conflict(
                "Excel authorization completion could not be confirmed",
            )),
        }
    }
}

#[async_trait]
impl AuthorizationCommitGuard for ExcelAuthorizationGuard {
    async fn commit(self: Box<Self>) -> Result<(), AdminError> {
        self.consume().await
    }
    // The authorization code was already sent. A rejected local commit requires new authorization.
    async fn abort(self: Box<Self>) -> Result<(), AdminError> {
        self.consume().await
    }
}

fn account_identity(account: &ProviderAccount) -> Result<ExcelIdentity, ProviderAdminError> {
    Ok(ExcelIdentity {
        account_id: account
            .upstream_account_id()
            .ok_or_else(|| map_error(ExcelAuthError::IdentityMismatch))?
            .into(),
        user_id: account
            .upstream_user_id()
            .ok_or_else(|| map_error(ExcelAuthError::IdentityMismatch))?
            .into(),
    })
}

fn prepared_excel(
    data: ExcelCredentialData,
    name: Option<&str>,
    proxy: Option<gateway_core::account::OutboundProxy>,
    provider: &ProviderKind,
) -> Result<PreparedCredentialCreate, ProviderAdminError> {
    let material = ExcelCredentialCodec::encode(&data).map_err(map_error)?;
    let now = Utc::now();
    Ok(PreparedCredentialCreate {
        model_access: None,
        outbound_proxy: proxy,
        account_id: ProviderAccountId::new(uuid::Uuid::new_v4().to_string())
            .map_err(|_| provider_admin_error(ProviderAdminErrorKind::Internal))?,
        provider_kind: provider.clone(),
        name: name
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| data.email.clone())
            .unwrap_or_else(|| "Excel".into()),
        email: data.email,
        upstream_user_id: Some(data.user_id),
        upstream_account_id: Some(data.chatgpt_account_id),
        plan_type: data.plan_type,
        authentication_kind: "excel_oauth".into(),
        provider_material: ProviderDocument::new(OpaqueProviderData::new(material.into_inner())),
        has_refresh_token: data.refresh_token.is_some(),
        access_token_expires_at: Some(data.expires_at),
        next_refresh_at: data
            .refresh_token
            .as_ref()
            .map(|_| (data.expires_at - TimeDelta::seconds(300)).max(now)),
        enabled: true,
        credential_state: CredentialState::Ready,
        credential_observed_at: now,
    })
}

fn excel_namespace() -> Result<ProviderKind, ProviderAdminError> {
    ProviderKind::new("openai-excel")
        .map_err(|_| provider_admin_error(ProviderAdminErrorKind::Internal))
}

fn excel_binding(value: &str) -> Result<OAuthPendingBinding, ProviderAdminError> {
    OAuthPendingBinding::try_new(value)
        .map_err(|_| provider_admin_error(ProviderAdminErrorKind::Invalid))
}

pub(super) fn map_error(error: ExcelAuthError) -> ProviderAdminError {
    use ExcelAuthError::*;
    let (kind, message) = match error {
        RequiresAuthorization | Expired => (
            ProviderAdminErrorKind::CredentialRefreshRequired,
            "需要重新获取独立的 Excel 凭据",
        ),
        IdentityMismatch => (
            ProviderAdminErrorKind::Conflict,
            "Excel 凭据的用户或工作区不一致",
        ),
        InvalidCallback => (
            ProviderAdminErrorKind::Invalid,
            "Excel 授权回调无效或已过期，请重新授权",
        ),
        ExchangeUncertain | RotationMissing => (
            ProviderAdminErrorKind::Ambiguous,
            "Excel 令牌交换结果不确定，请重新授权",
        ),
        ExchangeRejected => (
            ProviderAdminErrorKind::Invalid,
            "Excel 令牌交换被拒绝，请重新授权",
        ),
        AccessDenied => (
            ProviderAdminErrorKind::Unavailable,
            "该账号暂不可访问 Excel 模型目录",
        ),
        InvalidCredential => (
            ProviderAdminErrorKind::Invalid,
            "Excel 凭据格式或授权类型无效",
        ),
        InvalidCatalog => (ProviderAdminErrorKind::BadGateway, "Excel 模型目录格式无效"),
        Transport => (
            ProviderAdminErrorKind::Unavailable,
            "Excel 出口或上游连接暂不可用",
        ),
    };
    ProviderAdminError::new(kind).with_public_message(message)
}
