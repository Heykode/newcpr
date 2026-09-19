//! Durable installation facts; credential decoding remains provider-owned.

use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use gateway_core::account::{ProviderDeviceCodec, ProviderDeviceCodecError};

use super::*;

#[derive(Clone, Default)]
pub(crate) struct DeviceCodecs(Arc<RwLock<BTreeMap<String, Arc<dyn ProviderDeviceCodec>>>>);

impl DeviceCodecs {
    fn is_empty(&self) -> StoreResult<bool> {
        self.0
            .read()
            .map(|codecs| codecs.is_empty())
            .map_err(|_| invalid("provider device codecs are unavailable"))
    }

    pub(super) fn get(&self, provider: &str) -> StoreResult<Option<Arc<dyn ProviderDeviceCodec>>> {
        self.0
            .read()
            .map(|codecs| codecs.get(provider).cloned())
            .map_err(|_| invalid("provider device codecs are unavailable"))
    }
}

impl PgProviderAccountRepository {
    pub(crate) async fn initialize_devices(
        &self,
        codec: Arc<dyn ProviderDeviceCodec>,
    ) -> StoreResult<()> {
        require_nonempty("provider device", "provider_kind", codec.provider_kind())?;
        // Publish the codec before awaiting the backfill lock. Concurrent
        // account mutations must already participate in the registry protocol,
        // rather than observe an empty codec map and skip archival.
        self.device_codecs
            .0
            .write()
            .map_err(|_| invalid("provider device codecs are unavailable"))?
            .insert(codec.provider_kind().to_owned(), Arc::clone(&codec));
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin provider device backfill"))?;
        lock_device_mutation(&mut transaction).await?;
        let rows = sqlx::query(
            "select upstream_user_id, upstream_account_id, provider_credentials_json
             from provider_accounts
             where provider_kind = $1 and upstream_user_id is not null
             order by id for update",
        )
        .bind(codec.provider_kind())
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| postgres_unavailable("load existing provider devices"))?;
        for row in rows {
            let user: String = get(&row, "upstream_user_id")?;
            let account: Option<String> = get(&row, "upstream_account_id")?;
            let Some(account) = account.as_deref().filter(|value| !value.is_empty()) else {
                continue;
            };
            let current = row_credential(&row)?;
            retain_existing(
                &mut transaction,
                codec.as_ref(),
                &user,
                Some(account),
                &current,
            )
            .await?;
        }
        transaction
            .commit()
            .await
            .map_err(|_| postgres_unavailable("commit provider device backfill"))?;
        Ok(())
    }

    pub(crate) async fn prepare_account_device(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        incoming: &NewProviderAccount,
    ) -> StoreResult<JsonObject> {
        let Some(codec) = self.device_codecs.get(&incoming.provider_kind)? else {
            return Ok(incoming.provider_credentials_json.clone());
        };
        let (Some(user), Some(account)) = (
            incoming
                .upstream_user_id
                .as_deref()
                .filter(|value| !value.is_empty()),
            incoming
                .upstream_account_id
                .as_deref()
                .filter(|value| !value.is_empty()),
        ) else {
            // An unresolved principal must never be matched by email or row ID.
            return Ok(incoming.provider_credentials_json.clone());
        };
        lock_device_mutation(transaction).await?;
        let row = sqlx::query(
            "select provider_credentials_json from provider_accounts
             where provider_kind = $1 and upstream_user_id = $2
               and coalesce(upstream_account_id, '') = $3 for update",
        )
        .bind(&incoming.provider_kind)
        .bind(user)
        .bind(account)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|_| postgres_unavailable("lock existing provider device"))?;
        let current = row.as_ref().map(row_credential).transpose()?;
        bind_material(
            transaction,
            codec.as_ref(),
            user,
            Some(account),
            &incoming.provider_credentials_json,
            current.as_ref(),
        )
        .await
    }

    pub(crate) async fn prepare_rotated_device(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        account_id: &str,
        expected_revision: u64,
        replacement_identity: Option<&ProviderAccountIdentity>,
        replacement_email: Option<&str>,
        incoming: &JsonObject,
    ) -> StoreResult<JsonObject> {
        if self.device_codecs.is_empty()? {
            return Ok(incoming.clone());
        }
        lock_device_mutation(transaction).await?;
        let row = sqlx::query(
            "select provider_kind, upstream_user_id, upstream_account_id, email,
                    provider_credentials_json
             from provider_accounts where id = $1 and credential_revision = $2 for update",
        )
        .bind(account_id)
        .bind(to_i64(expected_revision)?)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|_| postgres_unavailable("lock provider device for rotation"))?
        .ok_or_else(|| StoreError::Conflict {
            entity: ENTITY,
            id: account_id.to_owned(),
            kind: ConflictKind::StaleRevision,
        })?;
        let provider: String = get(&row, "provider_kind")?;
        let Some(codec) = self.device_codecs.get(&provider)? else {
            return Ok(incoming.clone());
        };
        let old_user: Option<String> = get(&row, "upstream_user_id")?;
        let old_account: Option<String> = get(&row, "upstream_account_id")?;
        let old_email: Option<String> = get(&row, "email")?;
        let user = replacement_identity
            .map(ProviderAccountIdentity::upstream_user_id)
            .or(old_user.as_deref());
        let account = match replacement_identity {
            Some(identity) => identity.upstream_account_id(),
            None => old_account.as_deref(),
        };
        if replacement_identity.is_some()
            && old_account
                .as_deref()
                .is_some_and(|known| Some(known) != account)
        {
            return Err(device_conflict(
                "reauthorization cannot move a device to another upstream workspace",
            ));
        }
        let (Some(user), Some(account)) = (
            user.filter(|value| !value.is_empty()),
            account.filter(|value| !value.is_empty()),
        ) else {
            return Ok(incoming.clone());
        };
        let current = row_credential(&row)?;
        let old_user = old_user.as_deref().filter(|value| !value.is_empty());
        let old_account = old_account.as_deref().filter(|value| !value.is_empty());
        if let (Some(old_user), Some(old_account), Some(new_user), Some(new_account)) =
            (old_user, old_account, Some(user), Some(account))
            && old_user != new_user
        {
            let emails_match = old_email
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .zip(
                    replacement_email
                        .map(str::trim)
                        .filter(|value| !value.is_empty()),
                )
                .is_some_and(|(old, new)| old.eq_ignore_ascii_case(new));
            if !emails_match {
                return Err(device_conflict(
                    "reauthorization cannot rebind a device without a matching email",
                ));
            }
            let installation_id = codec
                .installation_id(&current)
                .map_err(device_codec_error)?;
            rebind_identity(
                transaction,
                &provider,
                old_user,
                old_account,
                new_user,
                new_account,
                &installation_id,
            )
            .await?;
        }
        bind_material(
            transaction,
            codec.as_ref(),
            user,
            Some(account),
            incoming,
            Some(&current),
        )
        .await
    }

    pub(crate) async fn archive_deleted_devices(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        account_ids: &[String],
    ) -> StoreResult<()> {
        if self.device_codecs.is_empty()? {
            return Ok(());
        }
        lock_device_mutation(transaction).await?;
        let rows = sqlx::query(
            "select provider_kind, upstream_user_id, upstream_account_id,
                    provider_credentials_json
             from provider_accounts where id = any($1::text[]) order by id for update",
        )
        .bind(account_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| postgres_unavailable("lock provider devices for deletion"))?;
        for row in rows {
            let provider: String = get(&row, "provider_kind")?;
            let Some(codec) = self.device_codecs.get(&provider)? else {
                continue;
            };
            let Some(user) = get::<Option<String>>(&row, "upstream_user_id")? else {
                continue;
            };
            let account: Option<String> = get(&row, "upstream_account_id")?;
            let Some(account) = account.as_deref().filter(|value| !value.is_empty()) else {
                continue;
            };
            retain_existing(
                transaction,
                codec.as_ref(),
                &user,
                Some(account),
                &row_credential(&row)?,
            )
            .await?;
        }
        Ok(())
    }
}

async fn rebind_identity(
    transaction: &mut Transaction<'_, Postgres>,
    provider: &str,
    old_user: &str,
    old_account: &str,
    new_user: &str,
    new_account: &str,
    installation_id: &str,
) -> StoreResult<()> {
    let rebound = sqlx::query_scalar::<_, String>(
        "update provider_device_identities
         set upstream_user_id = $4, upstream_account_id = $5, updated_at = now()
         where provider_kind = $1 and upstream_user_id = $2 and upstream_account_id = $3
           and installation_id = $6
         returning installation_id",
    )
    .bind(provider)
    .bind(old_user)
    .bind(old_account)
    .bind(new_user)
    .bind(new_account)
    .bind(installation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| {
        if error
            .as_database_error()
            .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
        {
            device_conflict("device is already bound to another upstream principal")
        } else {
            postgres_unavailable("rebind provider device identity")
        }
    })?;
    if rebound.as_deref() == Some(installation_id) {
        return Ok(());
    }
    claim_identity(
        transaction,
        provider,
        new_user,
        Some(new_account),
        installation_id,
    )
    .await
    .map(|_| ())
}

// Account mutation only, never request dispatch. A single transaction-scoped
// lock avoids missing-row import/delete races and reversed registry/row lock
// order. Admin paths take config_revision first, then this lock.
async fn lock_device_mutation(transaction: &mut Transaction<'_, Postgres>) -> StoreResult<()> {
    sqlx::query(
        "select pg_advisory_xact_lock(hashtextextended(current_schema() || ':cpr.provider-devices', 0))",
    )
        .execute(&mut **transaction)
        .await
        .map_err(|_| postgres_unavailable("lock provider device mutation"))?;
    Ok(())
}

async fn bind_material(
    transaction: &mut Transaction<'_, Postgres>,
    codec: &dyn ProviderDeviceCodec,
    user: &str,
    account: Option<&str>,
    incoming: &JsonObject,
    existing: Option<&PlaintextCredential>,
) -> StoreResult<JsonObject> {
    let material = PlaintextCredential::new(incoming.fields().clone());
    let candidate = codec
        .installation_id(existing.unwrap_or(&material))
        .map_err(device_codec_error)?;
    let retained = claim_identity(
        transaction,
        codec.provider_kind(),
        user,
        account,
        &candidate,
    )
    .await?;
    if existing.is_some() && retained != candidate {
        return Err(device_conflict(
            "existing account conflicts with its durable device",
        ));
    }
    let updated = codec
        .with_installation_id(&material, &retained)
        .map_err(device_codec_error)?;
    JsonObject::try_from_value(
        "provider_credentials_json",
        serde_json::Value::Object(updated.into_inner()),
        CREDENTIALS_MAX_BYTES,
    )
}

async fn retain_existing(
    transaction: &mut Transaction<'_, Postgres>,
    codec: &dyn ProviderDeviceCodec,
    user: &str,
    account: Option<&str>,
    current: &PlaintextCredential,
) -> StoreResult<()> {
    let candidate = codec.installation_id(current).map_err(device_codec_error)?;
    let retained = claim_identity(
        transaction,
        codec.provider_kind(),
        user,
        account,
        &candidate,
    )
    .await?;
    if retained != candidate {
        return Err(device_conflict(
            "existing account conflicts with its durable device",
        ));
    }
    Ok(())
}

async fn claim_identity(
    transaction: &mut Transaction<'_, Postgres>,
    provider: &str,
    user: &str,
    account: Option<&str>,
    installation_id: &str,
) -> StoreResult<String> {
    require_nonempty("provider device", "upstream_user_id", user)?;
    require_nonempty("provider device", "installation_id", installation_id)?;
    sqlx::query_scalar(
        "insert into provider_device_identities
             (provider_kind, upstream_user_id, upstream_account_id, installation_id)
         values ($1, $2, $3, $4)
         on conflict (provider_kind, upstream_user_id, upstream_account_id)
         do update set installation_id = provider_device_identities.installation_id
         returning installation_id",
    )
    .bind(provider)
    .bind(user)
    .bind(account.unwrap_or_default())
    .bind(installation_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| {
        if error
            .as_database_error()
            .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
        {
            device_conflict("device is already bound to another upstream principal")
        } else {
            postgres_unavailable("retain provider device identity")
        }
    })
}

fn row_credential(row: &sqlx::postgres::PgRow) -> StoreResult<PlaintextCredential> {
    get::<serde_json::Value>(row, "provider_credentials_json")?
        .as_object()
        .cloned()
        .map(PlaintextCredential::new)
        .ok_or_else(|| invalid("invalid provider credential object"))
}

fn device_codec_error(error: ProviderDeviceCodecError) -> StoreError {
    match error {
        ProviderDeviceCodecError::Conflict => device_conflict("invalid provider device identity"),
        ProviderDeviceCodecError::Unavailable => {
            postgres_unavailable("project provider device identity")
        }
        ProviderDeviceCodecError::Invalid => invalid("provider device identity is invalid"),
    }
}

fn device_conflict(message: &str) -> StoreError {
    StoreError::Conflict {
        entity: "provider device identity",
        id: message.to_owned(),
        kind: ConflictKind::InvalidTransition,
    }
}
