//! Official admission CAS and persistent-primary migration, using NewCPR binding generations.
use super::*;
use gateway_core::account::AccountAffinity;

impl CodexCredentialSelector {
    pub(super) async fn scoped_accounts(
        &self,
        request: &CredentialSelectionInput<'_>,
    ) -> Result<Vec<ProviderAccount>, CredentialSelectionError> {
        if let Some(scope) = request.attempt.account_scope() {
            let ids = scope.allowed_account_ids().cloned().collect::<Vec<_>>();
            let omissions = request
                .upstream_model
                .filter(|_| {
                    request.responses_model && !request.attempt.is_diagnostic_required_account()
                })
                .and_then(|model| {
                    self.catalog.omissions(model, &ids, |id| {
                        scope.directory().account(id).is_some_and(|a| {
                            a.model_access().mode() == AccountModelAccessMode::Allowlist
                        })
                    })
                });
            if let Some(omissions) = omissions {
                let provider = gateway_core::routing::ProviderKind::new("openai")
                    .map_err(|_| CredentialSelectionError::InvalidCredential(None))?;
                self.repository
                    .store()
                    .list_catalog_candidates(&provider, &ids, &omissions)
                    .await
                    .map_err(|error| CredentialSelectionError::Store(Some(error.into())))
            } else {
                Ok(self.repository.list_for_accounts(&ids).await?)
            }
        } else {
            Ok(self.repository.list_for_provider().await?)
        }
    }

    pub(super) fn session_round_robin_cursor(
        request: &CredentialSelectionInput<'_>,
        binding: Option<&ProviderSessionAffinityBinding>,
        cursor: u64,
    ) -> u64 {
        if request
            .attempt
            .account_selection_policy()
            .openai_account_affinity()
            == AccountAffinity::Preferred
            && binding.is_some()
        {
            use sha2::{Digest, Sha256};
            request.affinity_key().map_or(cursor, |key| {
                let digest = Sha256::digest(key.expose_to_store().as_bytes());
                let mut bytes = [0; 8];
                bytes.copy_from_slice(&digest[..8]);
                u64::from_be_bytes(bytes)
            })
        } else {
            cursor
        }
    }

    pub(super) async fn admit_selected_session(
        &self,
        request: &CredentialSelectionInput<'_>,
        expected: Option<&ProviderSessionAffinityBinding>,
        account: &ProviderAccountId,
        pinned: bool,
        queue_rejection: Option<gateway_core::concurrency::QueueRejection>,
    ) -> Result<Option<ProviderSessionAffinityBinding>, CredentialSelectionError> {
        let Some(key) = request
            .affinity_key()
            .filter(|_| !request.attempt.is_diagnostic_required_account())
        else {
            return Ok(Some(ProviderSessionAffinityBinding::legacy(
                account.clone(),
            )));
        };
        let policy = request.attempt.account_selection_policy();
        let mut binding_account = account;
        if policy.openai_account_affinity() == AccountAffinity::Preferred
            && let Some(primary) = expected
            && (primary.account_id() == account
                || pinned
                || !self
                    .preferred_primary_needs_migration(
                        primary,
                        Duration::from_secs(
                            request
                                .attempt
                                .request_tuning()
                                .account_concurrency_wait_timeout_seconds,
                        ),
                    )
                    .await?)
        {
            binding_account = primary.account_id();
        }
        // Exact continuation is authorized independently and must not restore an old owner.
        if pinned && expected.is_some_and(|binding| binding.account_id() != account) {
            binding_account = expected.expect("observed binding").account_id();
        }
        let result = tokio::time::timeout(
            SESSION_AFFINITY_TIMEOUT,
            self.session_affinity.admit_binding(
                &self.provider_kind,
                key,
                expected,
                binding_account,
                policy.openai_session_binding_ttl(),
            ),
        )
        .await
        .map_err(binding_store_error)
        .and_then(|result| result.map_err(binding_store_error));
        if let Ok(Some(binding)) = &result
            && let Some(reason) = queue_rejection
            && let Some(previous) =
                expected.filter(|previous| previous.account_id() != binding.account_id())
        {
            tracing::info!(
                request_id = %request.attempt.request_id(),
                attempt_index = request.attempt.attempt_index().get(),
                reason = %reason,
                from_account_id = %previous.account_id(),
                to_account_id = %binding.account_id(),
                "OpenAI 账号排队失败后已迁移绑定"
            );
            request.attempt.trace().record_evidence(
                "account.binding.migrated",
                serde_json::json!({
                    "reason": reason.to_string(),
                    "fromAccountId": previous.account_id().as_str(),
                    "toAccountId": binding.account_id().as_str(),
                }),
            );
        }
        match result {
            Err(_) if request.discard_optional_cache_preference() => Ok(Some(
                ProviderSessionAffinityBinding::legacy(account.clone()),
            )),
            result => result,
        }
    }

    /// 只按账号整体的当前事实迁移主绑定，临时容量和单模型限制仍允许恢复热缓存
    async fn preferred_primary_needs_migration(
        &self,
        binding: &ProviderSessionAffinityBinding,
        recovery_window: Duration,
    ) -> Result<bool, CredentialSelectionError> {
        let Some(account) = self
            .repository
            .store()
            .get_account(binding.account_id())
            .await
            .map_err(|source| CredentialSelectionError::Store(Some(source.into())))?
        else {
            return Ok(true);
        };
        if !account.enabled()
            || account.provider() != &self.provider_kind
            || matches!(
                account.credential_state(),
                CredentialState::Expired | CredentialState::Banned | CredentialState::Invalid
            )
        {
            return Ok(true);
        }
        let now = SystemTime::now();
        // 接近 reset 时保留主账号，并给已到 reset 的额度复核同长度的恢复宽限
        Ok(account.quota().is_exhausted()
            && account.quota().reset_at().is_none_or(|reset| {
                reset
                    .duration_since(now)
                    .unwrap_or_else(|error| error.duration())
                    > recovery_window
            }))
    }
}
