//! Pg 账号 repository：Core/Admin 端口实现与 admin 事务。

use super::*;

#[async_trait]
pub trait ProviderAccountRepository: Send + Sync {
    async fn load_provider_account(&self, id: &str) -> StoreResult<Option<ProviderAccountRecord>>;
    async fn list_provider_accounts(
        &self,
        provider_kind: Option<&str>,
        include_disabled: bool,
    ) -> StoreResult<Vec<ProviderAccountSummary>>;
    async fn insert_provider_account(&self, account: NewProviderAccount) -> StoreResult<()>;
    async fn update_provider_account(&self, account: UpdateProviderAccount) -> StoreResult<bool>;
    async fn compare_and_swap_credentials(
        &self,
        update: ProviderCredentialUpdate,
    ) -> StoreResult<Revision>;
    async fn apply_provider_account_state(
        &self,
        update: ProviderAccountStateUpdate,
    ) -> StoreResult<bool>;
    async fn apply_provider_account_diagnostic_state(
        &self,
        update: ProviderAccountStateUpdate,
    ) -> StoreResult<bool>;
    async fn set_provider_account_enabled(&self, id: &str, enabled: bool) -> StoreResult<bool>;
    async fn compare_and_swap_provider_quota(
        &self,
        account_id: &str,
        expected_revision: Revision,
        quota: JsonObject,
        observed_at: DateTime<Utc>,
        state: QuotaState,
        plan_type: Option<&str>,
    ) -> StoreResult<bool>;
    async fn touch_provider_quota_observation(
        &self,
        account_id: &str,
        expected_revision: Revision,
        observed_at: DateTime<Utc>,
    ) -> StoreResult<bool>;
    async fn apply_provider_quota_access(
        &self,
        account_id: &str,
        expected_revision: Revision,
        state: QuotaState,
    ) -> StoreResult<bool>;
    async fn delete_provider_account(&self, id: &str) -> StoreResult<bool>;
}

#[async_trait]
pub trait ProviderAccountAdminRepository: Send + Sync {
    async fn export_provider_accounts(
        &self,
        scope: ProviderAccountAdminScope,
        account_ids: Vec<String>,
    ) -> StoreResult<Vec<ProviderAccountRecord>>;

    async fn import_provider_accounts(
        &self,
        command: ImportProviderAccounts,
    ) -> StoreResult<ProviderAccountAdminImport>;

    async fn import_provider_accounts_with_mode(
        &self,
        command: ImportProviderAccounts,
        require_new: bool,
    ) -> StoreResult<ProviderAccountAdminImport> {
        if require_new {
            return Err(postgres_unavailable("create-only import unavailable"));
        }
        self.import_provider_accounts(command).await
    }

    async fn rotate_provider_account(
        &self,
        command: RotateProviderAccount,
    ) -> StoreResult<ProviderAccountAdminRotation>;

    async fn switch_relogin_workspace(
        &self,
        command: RotateProviderAccount,
    ) -> StoreResult<ProviderAccountAdminRotation>;

    async fn rotate_with_workspace_policy(
        &self,
        command: RotateProviderAccount,
        switch_workspace: bool,
    ) -> StoreResult<ProviderAccountAdminRotation>;

    async fn batch_update_provider_accounts_admin(
        &self,
        command: BatchUpdateProviderAccountsAdmin,
    ) -> StoreResult<Revision>;

    async fn recover_provider_account_admin(
        &self,
        command: RecoverProviderAccount,
    ) -> StoreResult<Revision>;

    async fn delete_provider_accounts_admin(
        &self,
        command: DeleteProviderAccounts,
    ) -> StoreResult<Revision>;
}

#[derive(Clone)]
pub struct PgProviderAccountRepository {
    pub(crate) pool: PgPool,
    pub(crate) device_codecs: DeviceCodecs,
}

impl PgProviderAccountRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            device_codecs: DeviceCodecs::default(),
        }
    }
}

#[async_trait]
impl ProviderAccountRepository for PgProviderAccountRepository {
    async fn load_provider_account(&self, id: &str) -> StoreResult<Option<ProviderAccountRecord>> {
        require_nonempty(ENTITY, "id", id)?;
        let row = sqlx::query(ACCOUNT_SELECT)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| postgres_unavailable("load provider account"))?;
        row.map(account_record_from_row).transpose()
    }

    async fn list_provider_accounts(
        &self,
        provider_kind: Option<&str>,
        include_disabled: bool,
    ) -> StoreResult<Vec<ProviderAccountSummary>> {
        let rows = sqlx::query(
            "select
                    (select case when auto_location then detected_location_json -> 'location' else request_location_json end from outbound_proxies where outbound_proxies.id = provider_accounts.outbound_proxy_id) as request_location_json,
                    outbound_proxy_url, id, provider_kind, name, custom_name, email, upstream_user_id,
                    upstream_account_id, plan_type, authentication_kind, credential_revision, turn_state_binding_revision, has_refresh_token,
                    access_token_expires_at, next_refresh_at, enabled, turn_state_injection_enabled, responses_upstream, excel_models, excel_models_follow_global,
            case when excel_models_follow_global then (select excel_default_models from runtime_settings where id = 1) else excel_models end as effective_excel_models, concurrency_limit, weight, model_access_json, credential_state,
                    credential_observed_at, quota_access_state, quota_evidence,
                    quota_access_observed_at, quota_reset_at,
                    quota_observed_at, last_error_reason, last_error_message, created_at, updated_at,
                    relogin_count, last_relogin_at
             from provider_accounts
             where ($1::text is null or provider_kind = $1) and ($2 or enabled)
             order by provider_kind, name, id",
        )
        .bind(provider_kind)
        .bind(include_disabled)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("list provider accounts"))?;
        rows.into_iter().map(account_summary_from_row).collect()
    }

    async fn insert_provider_account(&self, mut account: NewProviderAccount) -> StoreResult<()> {
        account.validate()?;
        let credential_state = if account.upstream_user_id.is_some() {
            account.credential_state
        } else {
            CredentialState::Unknown
        };
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin provider account insert"))?;
        lock_account_egress_in_transaction(&mut transaction).await?;
        let proxy_id = match account.outbound_proxy.as_ref() {
            Some(proxy) => Some(
                super::super::proxies::ensure_proxy_for_url(&mut transaction, proxy, None)
                    .await?
                    .0,
            ),
            None => None,
        };
        account.provider_credentials_json = self
            .prepare_account_device(&mut transaction, &account)
            .await?;
        sqlx::query(
            "insert into provider_accounts (
               outbound_proxy_url, outbound_proxy_id, id, provider_kind, name, email, upstream_user_id,
               upstream_account_id, plan_type, authentication_kind, provider_credentials_json, credential_revision,
               has_refresh_token, access_token_expires_at, next_refresh_at, enabled,
               concurrency_limit, weight, model_access_json, credential_state, provider_quota_json,
               credential_observed_at, quota_access_observed_at, quota_observed_at, created_at, updated_at
             ) values (
               $18, $19, $1, $2, $3, $4, $5, $6, $7, $8, $9, 1, $10, $11, $12, $13,
               $14, $15, coalesce($20, '{\"mode\":\"all\",\"models\":[]}'::jsonb), $16, null, $17, null, null, now(), greatest(now(), $17)
             )",
        )
        .bind(&account.id)
        .bind(account.provider_kind)
        .bind(account.name)
        .bind(account.email)
        .bind(account.upstream_user_id)
        .bind(account.upstream_account_id)
        .bind(account.plan_type)
        .bind(account.authentication_kind)
        .bind(account.provider_credentials_json.as_value())
        .bind(account.has_refresh_token)
        .bind(account.access_token_expires_at)
        .bind(account.next_refresh_at)
        .bind(account.enabled)
        .bind(account.concurrency_limit.map(|limit| i64::from(limit.get())))
        .bind(i16::try_from(account.weight.get()).map_err(|_| invalid("invalid weight"))?)
        .bind(credential_state.as_str())
        .bind(account.credential_observed_at)
        .bind(account.outbound_proxy.as_ref().map(|proxy| proxy.expose_url()))
        .bind(proxy_id)
        .bind(account.model_access.as_ref().map(sqlx::types::Json))
        .execute(&mut *transaction)
        .await
        .map_err(|_| postgres_unavailable("insert provider account"))?;
        super::super::synchronize_account_egress_in_transaction(
            &mut transaction,
            std::slice::from_ref(&account.id),
        )
        .await?;
        transaction
            .commit()
            .await
            .map_err(|_| postgres_unavailable("commit provider account insert"))?;
        Ok(())
    }

    async fn update_provider_account(&self, account: UpdateProviderAccount) -> StoreResult<bool> {
        require_nonempty(ENTITY, "id", &account.id)?;
        require_nonempty(ENTITY, "name", &account.name)?;
        let result = sqlx::query(
            "update provider_accounts
             set name = $2, email = $3, plan_type = $4, updated_at = greatest(now(), updated_at)
             where id = $1",
        )
        .bind(account.id)
        .bind(account.name)
        .bind(account.email)
        .bind(account.plan_type)
        .execute(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("update provider account"))?;
        Ok(result.rows_affected() == 1)
    }

    async fn compare_and_swap_credentials(
        &self,
        mut update: ProviderCredentialUpdate,
    ) -> StoreResult<Revision> {
        require_nonempty(ENTITY, "account_id", &update.account_id)?;
        validate_object_size(
            "provider_credentials_json",
            &update.provider_credentials_json,
            CREDENTIALS_MAX_BYTES,
        )?;
        if !update.has_refresh_token && update.next_refresh_at.is_some() {
            return Err(invalid("next_refresh_at requires a refresh token"));
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin provider credential CAS"))?;
        update.provider_credentials_json = self
            .prepare_credential_device(
                &mut transaction,
                &update.account_id,
                update.expected_revision.get(),
                &update.provider_credentials_json,
            )
            .await?;
        let state_owner =
            state_retention::StateOwnerSnapshot::capture(&mut transaction, &update.account_id)
                .await?;
        let next = sqlx::query_scalar::<_, i64>(
            "update provider_accounts
             set provider_credentials_json = $3,
                 credential_revision = credential_revision + 1,
                 turn_state_binding_revision = credential_revision + 1,
                 has_refresh_token = $4,
                 access_token_expires_at = $5,
                 next_refresh_at = $6,
                 updated_at = greatest(now(), updated_at)
             where id = $1 and credential_revision = $2
             returning credential_revision",
        )
        .bind(&update.account_id)
        .bind(to_i64(update.expected_revision.get())?)
        .bind(update.provider_credentials_json.as_value())
        .bind(update.has_refresh_token)
        .bind(update.access_token_expires_at)
        .bind(update.next_refresh_at)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| postgres_unavailable("compare and swap provider credentials"))?
        .ok_or(StoreError::Conflict {
            entity: ENTITY,
            id: update.account_id,
            kind: ConflictKind::StaleRevision,
        })?;
        let revision = Revision::new(to_u64(next)?)?;
        self.retain_turn_state(&mut transaction, state_owner)
            .await?;
        transaction
            .commit()
            .await
            .map_err(|_| postgres_unavailable("commit provider credential CAS"))?;
        Ok(revision)
    }

    async fn apply_provider_account_state(
        &self,
        update: ProviderAccountStateUpdate,
    ) -> StoreResult<bool> {
        update.validate()?;
        let result = sqlx::query(
            "update provider_accounts
             set credential_state = case
                     when enabled and upstream_user_id is not null then $3
                     else credential_state
                 end,
                 credential_observed_at = case
                     when enabled and upstream_user_id is not null then $4
                     else credential_observed_at
                 end,
                 last_error_reason = case
                     when enabled and upstream_user_id is not null then $5
                     else last_error_reason
                 end,
                 last_error_message = case
                     when enabled and upstream_user_id is not null then $6
                     else last_error_message
                 end,
                 updated_at = case when enabled then greatest(now(), updated_at, $4) else updated_at end
             where id = $1 and credential_revision = $2
               and (credential_observed_at is null or credential_observed_at <= $4)",
        )
        .bind(update.account_id)
        .bind(to_i64(update.expected_revision.get())?)
        .bind(update.credential_state.as_str())
        .bind(update.credential_observed_at)
        .bind(update.error_reason.map(AccountErrorReason::as_str))
        .bind(update.message.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("apply provider account state"))?;
        Ok(result.rows_affected() == 1)
    }

    async fn apply_provider_account_diagnostic_state(
        &self,
        update: ProviderAccountStateUpdate,
    ) -> StoreResult<bool> {
        update.validate()?;
        let result = sqlx::query(
            "update provider_accounts
             set credential_state = $3,
                 credential_observed_at = $4,
                 last_error_reason = $5,
                 last_error_message = $6,
                 updated_at = greatest(now(), updated_at, $4)
             where id = $1 and credential_revision = $2
               and upstream_user_id is not null
               and (credential_observed_at is null or credential_observed_at <= $4)",
        )
        .bind(update.account_id)
        .bind(to_i64(update.expected_revision.get())?)
        .bind(update.credential_state.as_str())
        .bind(update.credential_observed_at)
        .bind(update.error_reason.map(AccountErrorReason::as_str))
        .bind(update.message.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("apply diagnostic provider account state"))?;
        Ok(result.rows_affected() == 1)
    }

    async fn set_provider_account_enabled(&self, id: &str, enabled: bool) -> StoreResult<bool> {
        require_nonempty(ENTITY, "id", id)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin provider account enabled state"))?;
        lock_account_egress_in_transaction(&mut transaction).await?;
        let result = sqlx::query(
            "update provider_accounts set enabled = $2, updated_at = greatest(now(), updated_at) where id = $1",
        )
        .bind(id)
        .bind(enabled)
        .execute(&mut *transaction)
        .await
        .map_err(|_| postgres_unavailable("set provider account enabled state"))?;
        transaction
            .commit()
            .await
            .map_err(|_| postgres_unavailable("commit provider account enabled state"))?;
        Ok(result.rows_affected() == 1)
    }

    async fn compare_and_swap_provider_quota(
        &self,
        account_id: &str,
        expected_revision: Revision,
        quota: JsonObject,
        observed_at: DateTime<Utc>,
        state: QuotaState,
        plan_type: Option<&str>,
    ) -> StoreResult<bool> {
        require_nonempty(ENTITY, "account_id", account_id)?;
        validate_object_size("provider_quota_json", &quota, QUOTA_MAX_BYTES)?;
        let access_observed_at = state.observed_at().map(DateTime::<Utc>::from);
        let result = sqlx::query(
            "update provider_accounts
             set provider_quota_json = $3, quota_observed_at = $4,
                 plan_type = coalesce($9, plan_type),
                 quota_access_state = case
                   when $7::timestamptz is not null
                     and (quota_access_observed_at is null or quota_access_observed_at <= $7)
                   then $5 else quota_access_state end,
                 quota_evidence = case
                   when $7::timestamptz is not null
                     and (quota_access_observed_at is null or quota_access_observed_at <= $7)
                   then $6 else quota_evidence end,
                 quota_access_observed_at = case
                   when $7::timestamptz is not null
                     and (quota_access_observed_at is null or quota_access_observed_at <= $7)
                   then $7 else quota_access_observed_at end,
                 quota_reset_at = case
                   when $7::timestamptz is not null
                     and (quota_access_observed_at is null or quota_access_observed_at <= $7)
                   then $8 else quota_reset_at end,
                 updated_at = greatest(now(), updated_at, $4, $7)
             where id = $1 and credential_revision = $2
               and (quota_observed_at is null or quota_observed_at <= $4)",
        )
        .bind(account_id)
        .bind(to_i64(expected_revision.get())?)
        .bind(quota.as_value())
        .bind(observed_at)
        .bind(state.access().as_str())
        .bind(state.evidence().map(QuotaEvidence::as_str))
        .bind(access_observed_at)
        .bind(state.reset_at().map(DateTime::<Utc>::from))
        .bind(plan_type)
        .execute(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("compare and swap provider quota"))?;
        Ok(result.rows_affected() == 1)
    }

    async fn apply_provider_quota_access(
        &self,
        account_id: &str,
        expected_revision: Revision,
        state: QuotaState,
    ) -> StoreResult<bool> {
        require_nonempty(ENTITY, "account_id", account_id)?;
        let observed_at = state
            .observed_at()
            .map(DateTime::<Utc>::from)
            .ok_or_else(|| invalid("quota access change requires observed_at"))?;
        let result = sqlx::query(
            "update provider_accounts
             set quota_access_observed_at = $3, quota_access_state = $4,
                 quota_evidence = $5, quota_reset_at = $6,
                 updated_at = greatest(now(), updated_at, $3)
             where id = $1 and credential_revision = $2
               and (quota_access_observed_at is null or quota_access_observed_at <= $3)",
        )
        .bind(account_id)
        .bind(to_i64(expected_revision.get())?)
        .bind(observed_at)
        .bind(state.access().as_str())
        .bind(state.evidence().map(QuotaEvidence::as_str))
        .bind(state.reset_at().map(DateTime::<Utc>::from))
        .execute(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("apply provider quota access"))?;
        Ok(result.rows_affected() == 1)
    }

    async fn touch_provider_quota_observation(
        &self,
        account_id: &str,
        expected_revision: Revision,
        observed_at: DateTime<Utc>,
    ) -> StoreResult<bool> {
        require_nonempty(ENTITY, "account_id", account_id)?;
        let result = sqlx::query(
            "update provider_accounts
             set quota_observed_at = $3, updated_at = greatest(now(), updated_at, $3)
             where id = $1 and credential_revision = $2
               and provider_quota_json is not null
               and (quota_observed_at is null or quota_observed_at <= $3)",
        )
        .bind(account_id)
        .bind(to_i64(expected_revision.get())?)
        .bind(observed_at)
        .execute(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("touch provider quota observation"))?;
        Ok(result.rows_affected() == 1)
    }

    async fn delete_provider_account(&self, id: &str) -> StoreResult<bool> {
        require_nonempty(ENTITY, "id", id)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin provider account deletion"))?;
        lock_account_egress_in_transaction(&mut transaction).await?;
        self.archive_deleted_devices(&mut transaction, &[id.to_owned()])
            .await?;
        let result = sqlx::query("delete from provider_accounts where id = $1 and not enabled")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| postgres_unavailable("delete disabled provider account"))?;
        if result.rows_affected() == 1 {
            super::super::synchronize_account_egress_in_transaction(
                &mut transaction,
                &[id.to_owned()],
            )
            .await?;
        }
        transaction
            .commit()
            .await
            .map_err(|_| postgres_unavailable("commit provider account deletion"))?;
        Ok(result.rows_affected() == 1)
    }
}

#[async_trait]
impl ProviderAccountAdminRepository for PgProviderAccountRepository {
    async fn export_provider_accounts(
        &self,
        scope: ProviderAccountAdminScope,
        account_ids: Vec<String>,
    ) -> StoreResult<Vec<ProviderAccountRecord>> {
        scope.validate()?;
        validate_admin_account_ids(&account_ids)?;
        let rows = sqlx::query(ACCOUNT_SELECT_BY_IDS)
            .bind(&account_ids)
            .bind(&scope.provider_kind)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| postgres_unavailable("export provider accounts"))?;
        let records = rows
            .into_iter()
            .map(account_record_from_row)
            .collect::<StoreResult<Vec<_>>>()?;
        if records.len() != account_ids.len() {
            return Err(invalid(
                "one or more exported accounts are missing or outside the Provider scope",
            ));
        }
        let by_id = records
            .into_iter()
            .map(|record| (record.summary.id.clone(), record))
            .collect::<std::collections::HashMap<_, _>>();
        account_ids
            .into_iter()
            .map(|id| {
                by_id.get(&id).cloned().ok_or_else(|| {
                    invalid("one or more exported accounts are missing after loading")
                })
            })
            .collect()
    }

    async fn import_provider_accounts(
        &self,
        command: ImportProviderAccounts,
    ) -> StoreResult<ProviderAccountAdminImport> {
        self.import_provider_accounts_with_mode(command, false)
            .await
    }

    async fn import_provider_accounts_with_mode(
        &self,
        command: ImportProviderAccounts,
        require_new: bool,
    ) -> StoreResult<ProviderAccountAdminImport> {
        command.validate()?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin provider account admin import"))?;
        let result = async {
            let revision = bump_config_revision_in_transaction(&mut transaction).await?;
            lock_account_egress_in_transaction(&mut transaction).await?;
            if let Some(binding) = &command.outbound_proxy {
                let (_, current) = super::super::proxies::resolve_proxy_selection(
                    &mut transaction,
                    &gateway_admin::model::proxies::AccountProxySelection::Saved(
                        binding.id.clone(),
                    ),
                )
                .await?;
                if current.as_ref() != Some(&binding.proxy) {
                    return Err(StoreError::Conflict {
                        entity: "outbound proxy",
                        id: binding.id.clone(),
                        kind: ConflictKind::StaleRevision,
                    });
                }
            }
            let mut account_ids = Vec::with_capacity(command.accounts.len());
            for account in &command.accounts {
                // Config revision row serializes admin imports and rotations.
                if require_new {
                    let exists = sqlx::query_scalar::<_, bool>(
                        "select exists (select 1 from provider_accounts where provider_kind=$1
                         and ((upstream_user_id=$2 and upstream_account_id=$3)
                              or lower(email)=lower($4)))",
                    )
                    .bind(&account.provider_kind)
                    .bind(&account.upstream_user_id)
                    .bind(&account.upstream_account_id)
                    .bind(&account.email)
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(|_| postgres_unavailable("check create-only import"))?;
                    if exists {
                        return Err(StoreError::Conflict {
                            entity: "provider account",
                            id: account.id.clone(),
                            kind: ConflictKind::StaleRevision,
                        });
                    }
                }
                let mut account = account.clone();
                account.provider_credentials_json = self
                    .prepare_account_device(&mut transaction, &account)
                    .await?;
                let state_owner =
                    state_retention::StateOwnerSnapshot::capture_import(&mut transaction, &account)
                        .await?;
                account_ids.push(
                    upsert_provider_account_in_transaction(&mut transaction, &account, require_new)
                        .await?,
                );
                self.retain_turn_state(&mut transaction, state_owner)
                    .await?;
            }
            if let Some(settings) = &command.settings {
                let unique_ids = account_ids
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                update_provider_accounts_scheduling_in_transaction(
                    &mut transaction,
                    &unique_ids,
                    AccountSchedulingPatch {
                        enabled: Some(settings.enabled),
                        concurrency_limit: Some(settings.concurrency_limit),
                        weight: Some(settings.weight),
                        turn_state_injection_enabled: settings.turn_state_injection_enabled,
                        responses_upstream: settings.responses_upstream,
                        excel_models: settings.excel_models.as_ref(),
                        excel_models_follow_global: settings.excel_models_follow_global,
                        model_access: settings.model_access.as_ref(),
                        outbound_proxy: None,
                    },
                )
                .await?;
                if let Some(name) = gateway_admin::model::accounts::normalize_custom_name(
                    settings.custom_name.as_deref(),
                )
                .map_err(|_| invalid("invalid custom account name"))?
                {
                    update_custom_name_in_transaction(
                        &mut transaction,
                        &unique_ids,
                        Some(name.as_str()),
                    )
                    .await?;
                }
                replace_account_group_assignments_in_transaction(
                    &mut transaction,
                    &unique_ids,
                    &settings.group_ids,
                )
                .await?;
            }
            super::super::synchronize_account_egress_in_transaction(&mut transaction, &account_ids)
                .await?;
            append_admin_audit_event_in_transaction(&mut transaction, command.audit, revision)
                .await?;
            Ok(ProviderAccountAdminImport {
                config_revision: revision,
                account_ids,
            })
        }
        .await;
        finish_admin_transaction(transaction, result, "provider account admin import").await
    }

    async fn rotate_provider_account(
        &self,
        command: RotateProviderAccount,
    ) -> StoreResult<ProviderAccountAdminRotation> {
        self.rotate_with_workspace_policy(command, false).await
    }

    async fn switch_relogin_workspace(
        &self,
        command: RotateProviderAccount,
    ) -> StoreResult<ProviderAccountAdminRotation> {
        self.rotate_with_workspace_policy(command, true).await
    }

    async fn rotate_with_workspace_policy(
        &self,
        command: RotateProviderAccount,
        switch_workspace: bool,
    ) -> StoreResult<ProviderAccountAdminRotation> {
        command.scope.validate()?;
        require_nonempty(ENTITY, "account_id", &command.profile.id)?;
        require_nonempty(ENTITY, "name", &command.profile.name)?;
        if let Some(identity) = &command.replacement_identity {
            require_nonempty(ENTITY, "upstream_user_id", identity.upstream_user_id())?;
            if let Some(account_id) = identity.upstream_account_id() {
                require_nonempty(ENTITY, "upstream_account_id", account_id)?;
            }
        }
        if command.profile.id != command.credential.account_id {
            return Err(invalid("rotated profile and credential account IDs differ"));
        }
        validate_credential_update(&command.credential)?;
        if let Some(operation_id) = &command.relogin_operation_id {
            require_nonempty(ENTITY, "relogin_operation_id", operation_id)?;
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin provider account admin rotation"))?;
        let result = async {
            let config_revision = bump_config_revision_in_transaction(&mut transaction).await?;
            if command.replacement_identity.is_some() {
                lock_account_egress_in_transaction(&mut transaction).await?;
            }
            let mut credential = command.credential.clone();
            if switch_workspace {
                // This path is separate from ordinary rotation and validated under the same CAS lock.
                let old = sqlx::query(
                    "select upstream_user_id, upstream_account_id, email, authentication_kind
                     from provider_accounts where id=$1 and provider_kind=$2 and credential_revision=$3",
                ).bind(&credential.account_id).bind(&command.scope.provider_kind)
                    .bind(to_i64(credential.expected_revision.get())?)
                    .fetch_optional(&mut *transaction).await
                    .map_err(|_| postgres_unavailable("lock workspace switch"))?
                    .ok_or_else(|| StoreError::Conflict { entity: ENTITY, id: credential.account_id.clone(), kind: ConflictKind::StaleRevision })?;
                let user: Option<String> = get(&old, "upstream_user_id")?;
                let workspace: Option<String> = get(&old, "upstream_account_id")?;
                let email: Option<String> = get(&old, "email")?;
                let authentication: String = get(&old, "authentication_kind")?;
                let identity = command.replacement_identity.as_ref()
                    .ok_or_else(|| invalid("workspace switch requires verified identity"))?;
                if command.scope.provider_kind != "openai" || authentication != "oauth"
                    || command.relogin_operation_id.is_none()
                    || user.as_deref().is_none_or(|old| old.is_empty() || old != identity.upstream_user_id())
                    || workspace.as_deref().is_none_or(str::is_empty)
                    || identity.upstream_account_id().is_none_or(str::is_empty)
                    || !email.as_deref().zip(command.profile.email.as_deref())
                        .is_some_and(|(old, new)| !old.is_empty() && old.eq_ignore_ascii_case(new))
                {
                    return Err(invalid("workspace switch principal mismatch"));
                }
            }
            credential.provider_credentials_json = self
                .prepare_rotated_device_with_workspace_policy(
                    &mut transaction,
                    (&credential.account_id, credential.expected_revision.get()),
                    command.replacement_identity.as_ref(),
                    command.profile.email.as_deref(),
                    &credential.provider_credentials_json,
                    switch_workspace,
                )
                .await?;
            let state_owner = state_retention::StateOwnerSnapshot::capture(
                &mut transaction,
                &credential.account_id,
            )
            .await?;
            if switch_workspace {
                sqlx::query(
                    "update provider_accounts set provider_quota_json=null, quota_observed_at=null,
                     quota_access_state='unknown', quota_evidence=null, quota_access_observed_at=null,
                     quota_reset_at=null where id=$1",
                ).bind(&credential.account_id).execute(&mut *transaction).await
                    .map_err(|_| postgres_unavailable("clear previous workspace quota"))?;
            }
            let credential_revision = rotate_provider_account_in_transaction(
                &mut transaction,
                &command.scope,
                &command.profile,
                command.replacement_identity.as_ref(),
                &credential,
            )
            .await?;
            self.retain_turn_state(&mut transaction, state_owner)
                .await?;
            if let Some(operation_id) = &command.relogin_operation_id {
                // The event and counter share the credential CAS transaction, not the library save.
                sqlx::query(
                    "insert into account_relogin_successes
                     (operation_id, account_id, credential_revision) values ($1, $2, $3)",
                )
                .bind(operation_id)
                .bind(&credential.account_id)
                .bind(to_i64(credential_revision.get())?)
                .execute(&mut *transaction)
                .await
                .map_err(|_| postgres_unavailable("record account relogin success"))?;
                sqlx::query(
                    "update provider_accounts
                     set relogin_count = relogin_count + 1, last_relogin_at = now()
                     where id = $1",
                )
                .bind(&credential.account_id)
                .execute(&mut *transaction)
                .await
                .map_err(|_| postgres_unavailable("increment account relogin count"))?;
            }
            if command.replacement_identity.is_some() {
                super::super::synchronize_account_egress_in_transaction(
                    &mut transaction,
                    std::slice::from_ref(&credential.account_id),
                )
                .await?;
            }
            append_admin_audit_event_in_transaction(
                &mut transaction,
                command.audit,
                config_revision,
            )
            .await?;
            Ok(ProviderAccountAdminRotation {
                config_revision,
                credential_revision,
            })
        }
        .await;
        finish_admin_transaction(transaction, result, "provider account admin rotation").await
    }

    async fn batch_update_provider_accounts_admin(
        &self,
        command: BatchUpdateProviderAccountsAdmin,
    ) -> StoreResult<Revision> {
        validate_batch_update_account_ids(&command.account_ids)?;
        if let Some(group_ids) = &command.group_ids {
            validate_batch_update_group_ids(group_ids)?;
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin provider account admin state change"))?;
        let result = async {
            let revision = bump_config_revision_in_transaction(&mut transaction).await?;
            update_provider_accounts_scheduling_in_transaction(
                &mut transaction,
                &command.account_ids,
                AccountSchedulingPatch {
                    enabled: command.enabled,
                    concurrency_limit: command.concurrency_limit,
                    weight: command.weight,
                    turn_state_injection_enabled: command.turn_state_injection_enabled,
                    responses_upstream: command.responses_upstream,
                    excel_models: command.excel_models.as_ref(),
                    excel_models_follow_global: command.excel_models_follow_global,
                    model_access: command.model_access.as_ref(),
                    outbound_proxy: command.outbound_proxy.as_ref(),
                },
            )
            .await?;
            if let Some(name) = &command.custom_name {
                update_custom_name_in_transaction(
                    &mut transaction,
                    &command.account_ids,
                    name.as_deref(),
                )
                .await?;
            }
            if let Some(group_ids) = &command.group_ids {
                replace_account_group_assignments_in_transaction(
                    &mut transaction,
                    &command.account_ids,
                    group_ids,
                )
                .await?;
            }
            append_admin_audit_event_in_transaction(&mut transaction, command.audit, revision)
                .await?;
            Ok(revision)
        }
        .await;
        finish_admin_transaction(transaction, result, "provider account admin state change").await
    }

    async fn recover_provider_account_admin(
        &self,
        command: RecoverProviderAccount,
    ) -> StoreResult<Revision> {
        validate_admin_account_ids(std::slice::from_ref(&command.account_id))?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin provider account admin recovery"))?;
        let result = async {
            let revision = bump_config_revision_in_transaction(&mut transaction).await?;
            lock_account_egress_in_transaction(&mut transaction).await?;
            let recovered = sqlx::query_scalar::<_, String>(
                "update provider_accounts
                 set enabled = true,
                     credential_state = 'ready',
                     credential_observed_at = now(),
                     access_token_expires_at = case
                         when access_token_expires_at <= now() then null
                         else access_token_expires_at
                     end,
                     provider_quota_json = null,
                     quota_observed_at = null,
                     quota_access_state = 'allowed',
                     quota_evidence = null,
                     quota_access_observed_at = now(),
                     quota_reset_at = null,
                     last_error_reason = null,
                     last_error_message = null,
                     updated_at = greatest(now(), updated_at)
                 where id = $1
                 returning id",
            )
            .bind(&command.account_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| postgres_unavailable("recover provider account state"))?
            .ok_or_else(|| StoreError::NotFound {
                entity: ENTITY,
                id: command.account_id.clone(),
            })?;
            append_admin_audit_event_in_transaction(&mut transaction, command.audit, revision)
                .await?;
            Ok((revision, recovered))
        }
        .await;
        finish_admin_transaction(transaction, result, "provider account admin recovery")
            .await
            .map(|(revision, _)| revision)
    }

    async fn delete_provider_accounts_admin(
        &self,
        command: DeleteProviderAccounts,
    ) -> StoreResult<Revision> {
        command.scope.validate()?;
        validate_admin_account_ids(&command.account_ids)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin provider account admin deletion"))?;
        let result = async {
            let revision = bump_config_revision_in_transaction(&mut transaction).await?;
            lock_account_egress_in_transaction(&mut transaction).await?;
            self.archive_deleted_devices(&mut transaction, &command.account_ids)
                .await?;
            delete_provider_accounts_in_transaction(
                &mut transaction,
                &command.scope,
                &command.account_ids,
            )
            .await?;
            append_admin_audit_event_in_transaction(&mut transaction, command.audit, revision)
                .await?;
            Ok(revision)
        }
        .await;
        finish_admin_transaction(transaction, result, "provider account admin deletion").await
    }
}

async fn replace_account_group_assignments_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    account_ids: &[String],
    group_ids: &[AccountGroupId],
) -> StoreResult<()> {
    let group_ids = group_ids
        .iter()
        .map(|group_id| group_id.as_str().to_owned())
        .collect::<Vec<_>>();
    if !group_ids.is_empty() {
        let known_group_count = sqlx::query_scalar::<_, i64>(
            "select count(*)::bigint from account_groups where id = any($1::text[])",
        )
        .bind(&group_ids)
        .fetch_one(&mut **transaction)
        .await
        .map_err(|_| postgres_unavailable("validate account group assignment"))?;
        if usize::try_from(known_group_count).ok() != Some(group_ids.len()) {
            return Err(StoreError::NotFound {
                entity: "account group",
                id: "one or more group IDs".to_owned(),
            });
        }
    }
    sqlx::query("delete from account_group_accounts where provider_account_id = any($1::text[])")
        .bind(account_ids)
        .execute(&mut **transaction)
        .await
        .map_err(|_| postgres_unavailable("clear account group assignments"))?;
    if group_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "insert into account_group_accounts
         (account_group_id, provider_account_id, created_at)
         select group_id, account_id, now()
         from unnest($1::text[]) group_id
         cross join unnest($2::text[]) account_id",
    )
    .bind(group_ids)
    .bind(account_ids)
    .execute(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("assign accounts to groups"))?;
    Ok(())
}

pub(crate) async fn upsert_provider_account_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    account: &NewProviderAccount,
    require_new: bool,
) -> StoreResult<String> {
    account.validate()?;
    let credential_state = if account.upstream_user_id.is_some() {
        account.credential_state
    } else {
        CredentialState::Unknown
    };
    let proxy_id = match account.outbound_proxy.as_ref() {
        Some(proxy) => Some(
            super::super::proxies::ensure_proxy_for_url(transaction, proxy, None)
                .await?
                .0,
        ),
        None => None,
    };
    let imported_id = sqlx::query_scalar::<_, String>(
        "insert into provider_accounts (
           outbound_proxy_url, outbound_proxy_id, id, provider_kind, name, email, upstream_user_id,
           upstream_account_id, plan_type, authentication_kind, provider_credentials_json, credential_revision,
           has_refresh_token, access_token_expires_at, next_refresh_at, enabled,
           concurrency_limit, weight, model_access_json, credential_state, provider_quota_json,
           credential_observed_at, quota_access_observed_at, quota_observed_at, created_at, updated_at
         ) values (
           $18, $19, $1, $2, $3, $4, $5, $6, $7, $8, $9, 1, $10, $11, $12, $13,
           $14, $15, coalesce($21, '{\"mode\":\"all\",\"models\":[]}'::jsonb), $16, null, $17, null, null, now(), greatest(now(), $17)
         )
         on conflict (
           provider_kind,
           upstream_user_id,
           (coalesce(upstream_account_id, ''))
         ) do update set
           name = excluded.name,
           email = excluded.email,
           plan_type = excluded.plan_type,
           authentication_kind = excluded.authentication_kind,
           provider_credentials_json = excluded.provider_credentials_json,
           outbound_proxy_url = coalesce(excluded.outbound_proxy_url, provider_accounts.outbound_proxy_url),
           outbound_proxy_id = coalesce(excluded.outbound_proxy_id, provider_accounts.outbound_proxy_id),
           credential_revision = provider_accounts.credential_revision + 1,
           turn_state_binding_revision = provider_accounts.credential_revision + 1,
           has_refresh_token = excluded.has_refresh_token,
           access_token_expires_at = excluded.access_token_expires_at,
           next_refresh_at = excluded.next_refresh_at,
           enabled = excluded.enabled,
           model_access_json = coalesce($21, provider_accounts.model_access_json),
           credential_state = excluded.credential_state,
           provider_quota_json = null,
           quota_access_state = 'unknown',
           quota_evidence = null,
           quota_access_observed_at = null,
           quota_reset_at = null,
           credential_observed_at = excluded.credential_observed_at,
           quota_observed_at = null,
           last_error_reason = null,
           last_error_message = null,
           updated_at = greatest(now(), provider_accounts.updated_at, excluded.credential_observed_at)
         where not $20
         returning id",
    )
    .bind(&account.id)
    .bind(&account.provider_kind)
    .bind(&account.name)
    .bind(&account.email)
    .bind(&account.upstream_user_id)
    .bind(&account.upstream_account_id)
    .bind(&account.plan_type)
    .bind(&account.authentication_kind)
    .bind(account.provider_credentials_json.as_value())
    .bind(account.has_refresh_token)
    .bind(account.access_token_expires_at)
    .bind(account.next_refresh_at)
    .bind(account.enabled)
    .bind(account.concurrency_limit.map(|limit| i64::from(limit.get())))
    .bind(i16::try_from(account.weight.get()).map_err(|_| invalid("invalid weight"))?)
    .bind(credential_state.as_str())
    .bind(account.credential_observed_at)
    .bind(account.outbound_proxy.as_ref().map(|proxy| proxy.expose_url()))
    .bind(proxy_id)
    .bind(require_new)
    .bind(account.model_access.as_ref().map(sqlx::types::Json))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| {
        if error
            .as_database_error()
            .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
        {
            StoreError::Conflict {
                entity: ENTITY,
                id: account.id.clone(),
                kind: ConflictKind::InvalidTransition,
            }
        } else {
            postgres_unavailable("upsert provider account in admin transaction")
        }
    })?
    .ok_or_else(|| StoreError::Conflict {
        entity: ENTITY,
        id: account.id.clone(),
        kind: ConflictKind::InvalidTransition,
    })?;
    Ok(imported_id)
}

pub(crate) async fn rotate_provider_account_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    scope: &ProviderAccountAdminScope,
    profile: &UpdateProviderAccount,
    replacement_identity: Option<&ProviderAccountIdentity>,
    update: &ProviderCredentialUpdate,
) -> StoreResult<Revision> {
    let replace_identity = replacement_identity.is_some();
    let upstream_user_id = replacement_identity.map(ProviderAccountIdentity::upstream_user_id);
    let upstream_account_id =
        replacement_identity.and_then(ProviderAccountIdentity::upstream_account_id);
    let next = sqlx::query_scalar::<_, i64>(
        "update provider_accounts
         set name = case when $14 then name else $4 end,
             email = case when $14 then email else $5 end,
             plan_type = case when $14 then plan_type else $6 end,
             provider_credentials_json = $7,
             credential_revision = credential_revision + 1,
             turn_state_binding_revision = credential_revision + 1,
             has_refresh_token = $8,
             access_token_expires_at = $9,
             next_refresh_at = $10,
             upstream_user_id = case when $11::boolean then $12::text else upstream_user_id end,
             upstream_account_id = case when $11::boolean then $13::text else upstream_account_id end,
             credential_state = case
                 when coalesce($12::text, upstream_user_id) is not null then 'ready'
                 else 'unknown'
             end,
             credential_observed_at = now(),
             last_error_reason = null,
             last_error_message = null,
             updated_at = greatest(now(), updated_at)
         where id = $1 and provider_kind = $2
           and credential_revision = $3
         returning credential_revision",
    )
    .bind(&update.account_id)
    .bind(&scope.provider_kind)
    .bind(to_i64(update.expected_revision.get())?)
    .bind(&profile.name)
    .bind(&profile.email)
    .bind(&profile.plan_type)
    .bind(update.provider_credentials_json.as_value())
    .bind(update.has_refresh_token)
    .bind(update.access_token_expires_at)
    .bind(update.next_refresh_at)
    .bind(replace_identity)
    .bind(upstream_user_id)
    .bind(upstream_account_id)
    .bind(update.preserve_profile)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| {
        if error
            .as_database_error()
            .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
        {
            StoreError::Conflict {
                entity: ENTITY,
                id: update.account_id.clone(),
                kind: ConflictKind::InvalidTransition,
            }
        } else {
            postgres_unavailable("rotate provider account in admin transaction")
        }
    })?
    .ok_or_else(|| StoreError::Conflict {
        entity: ENTITY,
        id: update.account_id.clone(),
        kind: ConflictKind::StaleRevision,
    })?;
    Revision::new(to_u64(next)?)
}

struct AccountSchedulingPatch<'a> {
    enabled: Option<bool>,
    concurrency_limit: Option<Option<AccountConcurrencyLimit>>,
    weight: Option<AccountWeight>,
    turn_state_injection_enabled: Option<bool>,
    responses_upstream: Option<gateway_core::account::ResponsesUpstream>,
    excel_models: Option<&'a gateway_core::account::ExcelModels>,
    excel_models_follow_global: Option<bool>,
    model_access: Option<&'a gateway_core::account::AccountModelAccess>,
    outbound_proxy: Option<&'a gateway_admin::model::proxies::AccountProxySelection>,
}

async fn update_provider_accounts_scheduling_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    account_ids: &[String],
    patch: AccountSchedulingPatch<'_>,
) -> StoreResult<()> {
    let AccountSchedulingPatch {
        enabled,
        concurrency_limit,
        weight,
        turn_state_injection_enabled,
        responses_upstream,
        excel_models,
        excel_models_follow_global,
        model_access,
        outbound_proxy,
    } = patch;
    lock_account_egress_in_transaction(transaction).await?;
    if let Some(upstream) = excel_models
        .map(|_| gateway_core::account::ResponsesUpstream::Excel)
        .or(excel_models_follow_global.map(|_| gateway_core::account::ResponsesUpstream::Excel))
        .or(responses_upstream)
    {
        let identities = sqlx::query(
            "select provider_kind, authentication_kind from provider_accounts
             where id = any($1::text[]) for update",
        )
        .bind(account_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| postgres_unavailable("validate account responses upstream"))?;
        for row in identities {
            let provider: String = get(&row, "provider_kind")?;
            let authentication: String = get(&row, "authentication_kind")?;
            if !upstream.supports_account(&provider, &authentication) {
                return Err(invalid("Excel upstream requires an OpenAI OAuth account"));
            }
        }
    }
    let (proxy_id, proxy) = match outbound_proxy {
        Some(selection) => {
            super::super::proxies::resolve_proxy_selection(transaction, selection).await?
        }
        None => (None, None),
    };
    let updated = sqlx::query_scalar::<_, String>(
        "update provider_accounts
         set enabled = coalesce($2, enabled),
             concurrency_limit = case when $3 then $4 else concurrency_limit end,
             weight = coalesce($5, weight),
             turn_state_injection_enabled = coalesce($6, turn_state_injection_enabled),
             updated_at = greatest(now(), updated_at),
             outbound_proxy_url = case when $7 then $8 else outbound_proxy_url end,
             outbound_proxy_id = case when $7 then $9 else outbound_proxy_id end,
             model_access_json = coalesce($10, model_access_json),
             responses_upstream = coalesce($11, responses_upstream),
             excel_models = case when $13 is true then excel_models else coalesce($12, excel_models) end,
             excel_models_follow_global = coalesce($13, case when $12::text[] is not null then false else excel_models_follow_global end)
         where id = any($1::text[])
         returning id",
    )
    .bind(account_ids)
    .bind(enabled)
    .bind(concurrency_limit.is_some())
    .bind(concurrency_limit.and_then(|limit| limit.map(|limit| i64::from(limit.get()))))
    .bind(
        weight
            .map(|weight| i16::try_from(weight.get()))
            .transpose()
            .map_err(|_| invalid("invalid weight"))?,
    )
    .bind(turn_state_injection_enabled)
    .bind(outbound_proxy.is_some())
    .bind(
        proxy
            .as_ref()
            .map(gateway_core::account::OutboundProxy::expose_url),
    )
    .bind(proxy_id)
    .bind(model_access.map(sqlx::types::Json))
    .bind(responses_upstream.map(gateway_core::account::ResponsesUpstream::as_str))
    .bind(excel_models.map(gateway_core::account::ExcelModels::as_slice))
    .bind(excel_models_follow_global)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("set provider accounts state in admin transaction"))?
    .into_iter()
    .collect::<BTreeSet<_>>();
    let expected = account_ids.iter().cloned().collect::<BTreeSet<_>>();
    if updated == expected {
        if outbound_proxy.is_some() {
            super::super::synchronize_account_egress_in_transaction(transaction, account_ids)
                .await?;
        }
        Ok(())
    } else {
        Err(StoreError::NotFound {
            entity: ENTITY,
            id: "one or more provider account IDs".to_owned(),
        })
    }
}

async fn update_custom_name_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    account_ids: &[String],
    name: Option<&str>,
) -> StoreResult<()> {
    let name = gateway_admin::model::accounts::normalize_custom_name(name)
        .map_err(|_| invalid("invalid custom account name"))?;
    sqlx::query(
        "update provider_accounts set custom_name=$2, updated_at=greatest(now(), updated_at)
         where id=any($1::text[])",
    )
    .bind(account_ids)
    .bind(name)
    .execute(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("update custom account names"))?;
    Ok(())
}

pub(crate) async fn delete_provider_accounts_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    scope: &ProviderAccountAdminScope,
    account_ids: &[String],
) -> StoreResult<()> {
    let deleted = sqlx::query_scalar::<_, String>(
        "delete from provider_accounts
         where id = any($1::text[]) and provider_kind = $2
         returning id",
    )
    .bind(account_ids)
    .bind(&scope.provider_kind)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("delete provider account in admin transaction"))?;
    let deleted = deleted.into_iter().collect::<BTreeSet<_>>();
    let expected = account_ids.iter().cloned().collect::<BTreeSet<_>>();
    if deleted == expected {
        super::super::synchronize_account_egress_in_transaction(transaction, account_ids).await?;
        Ok(())
    } else {
        Err(invalid(
            "all deleted accounts must exist and match Provider scope",
        ))
    }
}

// Take this before device/account row locks: egress account edits acquire their
// settings fence before locking accounts too. Reversing that order can deadlock
// a core create/delete with a concurrent admin egress edit.
async fn lock_account_egress_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
) -> StoreResult<()> {
    sqlx::query("select id from provider_egress_settings where id = 1 for update")
        .execute(&mut **transaction)
        .await
        .map_err(|_| postgres_unavailable("lock provider account egress"))?;
    Ok(())
}

pub(crate) fn validate_credential_update(update: &ProviderCredentialUpdate) -> StoreResult<()> {
    require_nonempty(ENTITY, "account_id", &update.account_id)?;
    validate_object_size(
        "provider_credentials_json",
        &update.provider_credentials_json,
        CREDENTIALS_MAX_BYTES,
    )?;
    if !update.has_refresh_token && update.next_refresh_at.is_some() {
        return Err(invalid("next_refresh_at requires a refresh token"));
    }
    Ok(())
}

pub(crate) fn validate_admin_account_ids(account_ids: &[String]) -> StoreResult<()> {
    if account_ids.is_empty() || account_ids.len() > MAX_ADMIN_IMPORT_BATCH {
        return Err(invalid(
            "admin account selection must contain between 1 and 200 IDs",
        ));
    }
    let mut unique = BTreeSet::new();
    for account_id in account_ids {
        require_nonempty(ENTITY, "account_id", account_id)?;
        if !unique.insert(account_id.as_str()) {
            return Err(invalid("admin account selection contains duplicate IDs"));
        }
    }
    Ok(())
}

fn validate_batch_update_account_ids(account_ids: &[String]) -> StoreResult<()> {
    const MAX_BATCH_UPDATE_ACCOUNTS: usize = 1000;
    if account_ids.is_empty() || account_ids.len() > MAX_BATCH_UPDATE_ACCOUNTS {
        return Err(invalid(
            "account batch update must contain between 1 and 1000 IDs",
        ));
    }
    let mut unique = BTreeSet::new();
    for account_id in account_ids {
        require_nonempty(ENTITY, "account_id", account_id)?;
        if !unique.insert(account_id.as_str()) {
            return Err(invalid("account batch update contains duplicate IDs"));
        }
    }
    Ok(())
}

pub(super) fn validate_batch_update_group_ids(group_ids: &[AccountGroupId]) -> StoreResult<()> {
    const MAX_BATCH_UPDATE_GROUPS: usize = 1000;
    if group_ids.len() > MAX_BATCH_UPDATE_GROUPS {
        return Err(invalid("account batch update contains too many group IDs"));
    }
    let mut unique = BTreeSet::new();
    if group_ids
        .iter()
        .any(|group_id| !unique.insert(group_id.as_str()))
    {
        return Err(invalid("account batch update contains duplicate group IDs"));
    }
    Ok(())
}

pub(crate) async fn finish_admin_transaction<T>(
    transaction: Transaction<'_, Postgres>,
    result: StoreResult<T>,
    operation: &'static str,
) -> StoreResult<T> {
    match result {
        Ok(value) => {
            transaction
                .commit()
                .await
                .map_err(|_| postgres_unavailable(operation))?;
            Ok(value)
        }
        Err(error) => {
            transaction
                .rollback()
                .await
                .map_err(|_| postgres_unavailable(operation))?;
            Err(error)
        }
    }
}
