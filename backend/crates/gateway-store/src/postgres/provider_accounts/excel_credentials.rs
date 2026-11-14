//! Excel material has an independent revision, refresh fence and health owner.

use gateway_core::account::{
    ExcelCredentialCasUpdate, ExcelCredentialMetadata, LoadedExcelCredential,
};
use serde::Deserialize;

use super::*;

#[derive(Deserialize)]
struct MetadataRow {
    credential_revision: u64,
    upstream_user_id: String,
    upstream_account_id: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    credential_state: String,
    credential_observed_at: DateTime<Utc>,
    access_token_expires_at: Option<DateTime<Utc>>,
    has_refresh_token: bool,
    next_refresh_at: Option<DateTime<Utc>>,
    last_error_reason: Option<String>,
    last_error_message: Option<String>,
    suspended: bool,
    refresh_attempt_revision: Option<u64>,
    quota_access_state: String,
    quota_evidence: Option<String>,
    quota_access_observed_at: Option<DateTime<Utc>>,
    quota_reset_at: Option<DateTime<Utc>>,
}

pub(super) fn metadata_from_account_row(
    row: &sqlx::postgres::PgRow,
) -> StoreResult<Option<ExcelCredentialMetadata>> {
    let value: Option<serde_json::Value> = get(row, "excel_credential_json")?;
    value.map(parse_metadata).transpose()
}

fn parse_metadata(value: serde_json::Value) -> StoreResult<ExcelCredentialMetadata> {
    let row: MetadataRow =
        serde_json::from_value(value).map_err(|_| invalid("invalid Excel metadata"))?;
    let quota = QuotaState::from_persisted(
        parse_quota_access_state(&row.quota_access_state)?,
        parse_quota_evidence(row.quota_evidence)?,
        row.quota_access_observed_at.map(Into::into),
        row.quota_reset_at.map(Into::into),
    )
    .ok_or_else(|| invalid("invalid Excel quota state"))?;
    Ok(ExcelCredentialMetadata {
        revision: CoreCredentialRevision::new(row.credential_revision)
            .map_err(|_| invalid("invalid Excel revision"))?,
        state: parse_credential_state(&row.credential_state)?,
        upstream_user_id: row.upstream_user_id,
        upstream_account_id: row.upstream_account_id,
        email: row.email,
        plan_type: row.plan_type,
        access_token_expires_at: row.access_token_expires_at.map(Into::into),
        has_refresh_token: row.has_refresh_token,
        next_refresh_at: row.next_refresh_at.map(Into::into),
        observed_at: row.credential_observed_at.into(),
        last_error_reason: parse_error_reason(row.last_error_reason)?,
        last_error_message: row.last_error_message,
        suspended: row.suspended,
        refresh_attempted: row.refresh_attempt_revision.is_some(),
        quota,
    })
}

fn unavailable(error: sqlx::Error) -> CoreStoreError {
    CoreStoreError::caused_by(CoreStoreErrorKind::Unavailable, error)
}

impl PgProviderAccountRepository {
    pub(super) async fn import_excel(
        &self,
        prepared: PreparedCredentialCreate,
        target: Option<gateway_admin::model::accounts::ExcelCredentialImportTarget>,
        settings: Option<AccountImportSettings>,
        context: &MutationContext,
    ) -> AdminStoreResult<gateway_admin::model::accounts::ExcelCredentialImportResult> {
        let incoming = prepared_account(prepared).map_err(excel_admin_store_error)?;
        incoming.validate().map_err(excel_admin_store_error)?;
        let user = incoming
            .upstream_user_id
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| excel_admin_invalid("Excel identity is missing"))?;
        if incoming.provider_kind != "openai"
            || incoming.authentication_kind != "excel_oauth"
            || incoming
                .upstream_account_id
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return Err(excel_admin_invalid(
                "Excel requires a resolved OpenAI user and workspace",
            ));
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| excel_admin_unavailable())?;
        sqlx::query("select config_revision from runtime_settings where id=1 for update")
            .execute(&mut *tx)
            .await
            .map_err(|_| excel_admin_unavailable())?;
        sqlx::query("select id from provider_egress_settings where id=1 for update")
            .execute(&mut *tx)
            .await
            .map_err(|_| excel_admin_unavailable())?;
        let existing = sqlx::query(
            "select id,upstream_user_id,upstream_account_id,provider_kind from provider_accounts
            where ($1::text is not null and id=$1) or ($1::text is null and provider_kind='openai'
              and upstream_user_id=$2 and upstream_account_id is not distinct from $3) for update",
        )
        .bind(target.as_ref().map(|t| t.account_id.as_str()))
        .bind(user)
        .bind(&incoming.upstream_account_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| excel_admin_unavailable())?;
        let created = existing.is_none();
        let id = if let Some(row) = existing {
            let stored_user: Option<String> =
                get(&row, "upstream_user_id").map_err(excel_admin_store_error)?;
            let stored_workspace: Option<String> =
                get(&row, "upstream_account_id").map_err(excel_admin_store_error)?;
            let provider: String = get(&row, "provider_kind").map_err(excel_admin_store_error)?;
            if stored_user.as_deref() != Some(user)
                || stored_workspace != incoming.upstream_account_id
                || provider != "openai"
            {
                return Err(excel_admin_invalid(
                    "Excel identity does not match the account workspace",
                ));
            }
            get::<String>(&row, "id").map_err(excel_admin_store_error)?
        } else {
            if target.is_some() {
                return Err(AdminStoreError::new(
                    AdminStoreErrorKind::NotFound,
                    "Excel credential",
                    "account not found",
                ));
            }
            // A shell has no Codex credential and never enters Codex device initialization.
            let proxy_id = match incoming.outbound_proxy.as_ref() {
                Some(proxy) => Some(
                    super::super::proxies::ensure_proxy_for_url(&mut tx, proxy, None)
                        .await
                        .map_err(excel_admin_store_error)?
                        .0,
                ),
                None => None,
            };
            sqlx::query("insert into provider_accounts
                (id,provider_kind,name,email,upstream_user_id,upstream_account_id,plan_type,
                 authentication_kind,provider_credentials_json,credential_revision,credential_state,
                 credential_observed_at,has_refresh_token,enabled,responses_upstream,model_access_json,
                 outbound_proxy_url,outbound_proxy_id,created_at,updated_at)
                values($1,'openai',$2,$3,$4,$5,$6,'none','{}',1,'unknown',$7,false,$8,'codex',
                 coalesce($9,'{\"mode\":\"all\",\"models\":[]}'::jsonb),$10,$11,now(),greatest(now(),$7))")
                .bind(&incoming.id).bind(&incoming.name).bind(&incoming.email).bind(user)
                .bind(&incoming.upstream_account_id).bind(&incoming.plan_type).bind(incoming.credential_observed_at)
                .bind(incoming.enabled).bind(incoming.model_access.as_ref().map(sqlx::types::Json))
                .bind(incoming.outbound_proxy.as_ref().map(|p| p.expose_url())).bind(proxy_id)
                .execute(&mut *tx).await.map_err(|_| excel_admin_unavailable())?;
            super::super::synchronize_account_egress_in_transaction(
                &mut tx,
                std::slice::from_ref(&incoming.id),
            )
            .await
            .map_err(excel_admin_store_error)?;
            incoming.id.clone()
        };
        let current: Option<i64> = sqlx::query_scalar("select credential_revision from openai_excel_credentials where account_id=$1 for update")
            .bind(&id).fetch_optional(&mut *tx).await.map_err(|_| excel_admin_unavailable())?;
        let expected = target
            .as_ref()
            .and_then(|t| t.expected_revision)
            .map(|r| r.get());
        if current.and_then(|v| u64::try_from(v).ok()) != expected {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Conflict,
                "Excel credential",
                "Excel credential changed",
            ));
        }
        let revision = current
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| excel_admin_invalid("Excel revision overflow"))?;
        sqlx::query("insert into openai_excel_credentials
            (account_id,credential_revision,upstream_user_id,upstream_account_id,email,plan_type,
             credential_json,credential_state,credential_observed_at,access_token_expires_at,
             has_refresh_token,next_refresh_at,created_at,updated_at)
            values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,now(),greatest(now(),$9))
            on conflict(account_id) do update set credential_revision=excluded.credential_revision,
             email=excluded.email,plan_type=excluded.plan_type,credential_json=excluded.credential_json,
             credential_state=excluded.credential_state,credential_observed_at=excluded.credential_observed_at,
             access_token_expires_at=excluded.access_token_expires_at,has_refresh_token=excluded.has_refresh_token,
             next_refresh_at=excluded.next_refresh_at,last_error_reason=null,last_error_message=null,suspended=false,quality_pause_owner=null,
             refresh_attempt_revision=null,refresh_attempt_started_at=null,
             quota_json=null,quota_access_state='unknown',quota_evidence=null,quota_access_observed_at=null,
             quota_reset_at=null,quota_observed_at=null,
             updated_at=greatest(now(),openai_excel_credentials.updated_at,excluded.credential_observed_at)")
            .bind(&id).bind(revision).bind(user).bind(&incoming.upstream_account_id)
            .bind(&incoming.email).bind(&incoming.plan_type).bind(incoming.provider_credentials_json.as_value())
            .bind(incoming.credential_state.as_str()).bind(incoming.credential_observed_at)
            .bind(incoming.access_token_expires_at).bind(incoming.has_refresh_token).bind(incoming.next_refresh_at)
            .execute(&mut *tx).await.map_err(|_| excel_admin_unavailable())?;
        let config_revision = bump_config_revision_in_transaction(&mut tx)
            .await
            .map_err(excel_admin_store_error)?;
        if created && let Some(settings) = settings {
            self.apply_excel_shell_settings(&mut tx, &id, &settings)
                .await
                .map_err(excel_admin_store_error)?;
        }
        append_admin_audit_event_in_transaction(
            &mut tx,
            mutation_audit(
                context,
                "import_excel_credential",
                "provider_account",
                &id,
                vec!["excel_credential".to_owned()],
            ),
            config_revision,
        )
        .await
        .map_err(excel_admin_store_error)?;
        let row = sqlx::query(ACCOUNT_SELECT)
            .bind(&id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| excel_admin_unavailable())?;
        let result =
            admin_account_record(account_summary_from_row(row).map_err(excel_admin_store_error)?)?;
        tx.commit().await.map_err(|_| excel_admin_unavailable())?;
        Ok(
            gateway_admin::model::accounts::ExcelCredentialImportResult {
                account: result,
                config_revision: admin_revision(config_revision)?,
            },
        )
    }

    pub(super) async fn load_excel(
        &self,
        id: &CoreProviderAccountId,
    ) -> Result<LoadedExcelCredential, CoreStoreError> {
        // Lock parent before sidecar, matching imports and account updates.
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("select id from provider_accounts where id=$1 for share")
            .bind(id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| CoreStoreError::new(CoreStoreErrorKind::InvalidState))?;
        let row = sqlx::query("select e.credential_json, to_jsonb(e) - 'credential_json' - 'quota_json' as excel_credential_json
            from openai_excel_credentials e join provider_accounts a on a.id=e.account_id
            where e.account_id=$1 and a.provider_kind='openai'
              and a.upstream_user_id=e.upstream_user_id
              and a.upstream_account_id is not distinct from e.upstream_account_id for share of e")
            .bind(id.as_str()).fetch_optional(&mut *tx).await.map_err(unavailable)?
            .ok_or_else(|| CoreStoreError::new(CoreStoreErrorKind::InvalidState))?;
        let metadata = metadata_from_account_row(&row)
            .map_err(core_store_error)?
            .ok_or_else(|| CoreStoreError::new(CoreStoreErrorKind::InvalidData))?;
        let value: serde_json::Value = get(&row, "credential_json").map_err(core_store_error)?;
        let credential = PlaintextCredential::new(
            value
                .as_object()
                .cloned()
                .ok_or_else(|| CoreStoreError::new(CoreStoreErrorKind::InvalidData))?,
        );
        let account_row = sqlx::query(ACCOUNT_SELECT)
            .bind(id.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
        let account = core_account_from_summary(
            account_summary_from_row(account_row).map_err(core_store_error)?,
        )?;
        tx.commit().await.map_err(unavailable)?;
        Ok(LoadedExcelCredential {
            account,
            metadata,
            credential,
        })
    }

    pub(super) async fn claim_excel_refresh_once(
        &self,
        id: &CoreProviderAccountId,
        revision: CoreCredentialRevision,
    ) -> Result<bool, CoreStoreError> {
        let result = sqlx::query("update openai_excel_credentials e set refresh_attempt_revision=credential_revision,
            refresh_attempt_started_at=now(),updated_at=greatest(now(),updated_at)
            where account_id=$1 and credential_revision=$2 and refresh_attempt_revision is null
              and has_refresh_token and exists(select 1 from provider_accounts a where a.id=e.account_id
                and a.upstream_user_id=e.upstream_user_id and a.upstream_account_id is not distinct from e.upstream_account_id)")
            .bind(id.as_str()).bind(to_i64(revision.get()).map_err(core_store_error)?)
            .execute(&self.pool).await.map_err(unavailable)?;
        Ok(result.rows_affected() == 1)
    }

    pub(super) async fn cas_excel(
        &self,
        update: ExcelCredentialCasUpdate,
    ) -> Result<CredentialCasOutcome, CoreStoreError> {
        if !update.has_refresh_token && update.next_refresh_at.is_some() {
            return Err(CoreStoreError::new(CoreStoreErrorKind::InvalidData));
        }
        let material = JsonObject::try_from_value(
            "Excel credential",
            serde_json::Value::Object(update.credential.into_inner()),
            CREDENTIALS_MAX_BYTES,
        )
        .map_err(core_store_error)?;
        let next = update
            .expected_revision
            .next()
            .map_err(|_| CoreStoreError::new(CoreStoreErrorKind::InvalidData))?;
        let result = sqlx::query("update openai_excel_credentials e
            set credential_json=$3,credential_revision=$4,email=$5,plan_type=$6,
                has_refresh_token=$7,access_token_expires_at=$8,next_refresh_at=$9,
                credential_state='ready',credential_observed_at=$10,last_error_reason=null,last_error_message=null,
                refresh_attempt_revision=null,refresh_attempt_started_at=null,
                updated_at=greatest(now(),updated_at,$10)
            where account_id=$1 and credential_revision=$2 and credential_observed_at<=$10
              and upstream_user_id=$11 and upstream_account_id is not distinct from $12
              and exists(select 1 from provider_accounts a where a.id=e.account_id and a.provider_kind='openai'
                and a.upstream_user_id=$11 and a.upstream_account_id is not distinct from $12)")
            .bind(update.account_id.as_str()).bind(to_i64(update.expected_revision.get()).map_err(core_store_error)?)
            .bind(material.as_value()).bind(to_i64(next.get()).map_err(core_store_error)?)
            .bind(update.email).bind(update.plan_type).bind(update.has_refresh_token)
            .bind(update.access_token_expires_at.map(DateTime::<Utc>::from))
            .bind(update.next_refresh_at.map(DateTime::<Utc>::from)).bind(DateTime::<Utc>::from(update.observed_at))
            .bind(update.identity.upstream_user_id()).bind(update.identity.upstream_account_id())
            .execute(&self.pool).await.map_err(unavailable)?;
        Ok(if result.rows_affected() == 1 {
            CredentialCasOutcome::Updated(next)
        } else {
            CredentialCasOutcome::Conflict
        })
    }

    pub(super) async fn write_excel_state(
        &self,
        change: AccountStateChange,
        diagnostic: bool,
    ) -> Result<(), CoreStoreError> {
        let result = sqlx::query("update openai_excel_credentials e set credential_state=$3,credential_observed_at=$4,
            last_error_reason=$5,last_error_message=$6,updated_at=greatest(now(),updated_at,$4)
            where account_id=$1 and credential_revision=$2 and credential_observed_at<=$4
              and exists(select 1 from provider_accounts a where a.id=e.account_id and ($7 or a.enabled)
                and a.upstream_user_id=e.upstream_user_id and a.upstream_account_id is not distinct from e.upstream_account_id)")
            .bind(change.account_id.as_str()).bind(to_i64(change.expected_revision.get()).map_err(core_store_error)?)
            .bind(change.credential_state.as_str()).bind(DateTime::<Utc>::from(change.observed_at))
            .bind(change.error_reason.map(AccountErrorReason::as_str)).bind(change.message).bind(diagnostic)
            .execute(&self.pool).await.map_err(unavailable)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(CoreStoreError::new(CoreStoreErrorKind::Conflict))
        }
    }

    pub(super) async fn write_excel_quota(
        &self,
        observation: QuotaObservation,
    ) -> Result<QuotaWriteOutcome, CoreStoreError> {
        let material = JsonObject::try_from_value(
            "Excel quota",
            serde_json::Value::Object(observation.quota.into_inner()),
            QUOTA_MAX_BYTES,
        )
        .map_err(core_store_error)?;
        let result = sqlx::query("update openai_excel_credentials set quota_json=$3,quota_observed_at=$4,
            quota_access_state=$5,quota_evidence=$6,quota_access_observed_at=$7,quota_reset_at=$8,
            updated_at=greatest(now(),updated_at,$4,$7)
            where account_id=$1 and credential_revision=$2 and (quota_observed_at is null or quota_observed_at<=$4)
              and (quota_access_observed_at is null or quota_access_observed_at<=$7)")
            .bind(observation.account_id.as_str()).bind(to_i64(observation.expected_revision.get()).map_err(core_store_error)?)
            .bind(material.as_value()).bind(DateTime::<Utc>::from(observation.observed_at))
            .bind(observation.state.access().as_str()).bind(observation.state.evidence().map(QuotaEvidence::as_str))
            .bind(observation.state.observed_at().map(DateTime::<Utc>::from)).bind(observation.state.reset_at().map(DateTime::<Utc>::from))
            .execute(&self.pool).await.map_err(unavailable)?;
        Ok(if result.rows_affected() == 1 {
            QuotaWriteOutcome::Updated
        } else {
            QuotaWriteOutcome::Conflict
        })
    }
}

fn excel_admin_invalid(message: &'static str) -> AdminStoreError {
    AdminStoreError::new(AdminStoreErrorKind::Invalid, "Excel credential", message)
}

fn excel_admin_store_error(error: StoreError) -> AdminStoreError {
    admin_store_error("Excel credential", error)
}

fn excel_admin_unavailable() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "Excel credential",
        "Excel storage unavailable",
    )
}
