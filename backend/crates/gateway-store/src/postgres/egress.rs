//! Durable IPv6 pool, policy overrides and identity-keyed fixed affinity.

use std::{collections::BTreeMap, net::Ipv6Addr, sync::Arc};

use async_trait::async_trait;
use futures::future::BoxFuture;
use gateway_admin::{
    model::{
        MutationContext,
        egress::{ProviderEgressMutation, ReplaceProviderEgress, SetProviderAccountEgress},
    },
    ports::store::{AdminStoreResult, ProviderEgressStore},
};
use gateway_core::{
    account::ProviderAccountId,
    provider_ports::{
        ProviderStoreError, ProviderStoreErrorKind,
        egress::{
            EgressMode, ProviderEgressAddress, ProviderEgressConfig, ProviderEgressStorePort,
        },
    },
};
use sqlx::{PgPool, Postgres, Row as _, Transaction};

use super::{append_admin_audit_event_in_transaction, bump_config_revision_in_transaction};
use crate::{
    ConflictKind, StoreError, StoreResult, admin_revision, admin_store_error, mutation_audit,
    postgres_unavailable,
};

const ENTITY: &str = "provider IPv6 egress";

#[derive(Clone)]
pub struct PgProviderEgressRepository {
    pool: PgPool,
}

impl PgProviderEgressRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn load_config(&self) -> StoreResult<ProviderEgressConfig> {
        let mut transaction = self.pool.begin().await.map_err(|_| unavailable())?;
        sqlx::query("set transaction isolation level repeatable read, read only")
            .execute(&mut *transaction)
            .await
            .map_err(|_| unavailable())?;
        let config = load_config_in_transaction(&mut transaction).await?;
        transaction.commit().await.map_err(|_| unavailable())?;
        Ok(config)
    }
}

fn unavailable() -> StoreError {
    postgres_unavailable("provider IPv6 egress")
}

fn invalid() -> StoreError {
    StoreError::InvalidData {
        entity: ENTITY,
        message: "invalid IPv6 egress configuration".to_owned(),
    }
}

fn conflict(id: &str) -> StoreError {
    StoreError::Conflict {
        entity: ENTITY,
        id: id.to_owned(),
        kind: ConflictKind::InvalidTransition,
    }
}

fn mode_text(mode: EgressMode) -> &'static str {
    mode.as_str()
}

fn parse_mode(mode: &str) -> StoreResult<EgressMode> {
    EgressMode::parse(mode).ok_or_else(invalid)
}

async fn load_config_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
) -> StoreResult<ProviderEgressConfig> {
    let (revision, default_mode): (i64, String) =
        sqlx::query_as("select revision, default_mode from provider_egress_settings where id = 1")
            .fetch_one(&mut **transaction)
            .await
            .map_err(|_| unavailable())?;
    let mut addresses = Vec::new();
    for row in sqlx::query(
        "select id, address, enabled from provider_egress_addresses order by position, id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| unavailable())?
    {
        addresses.push(ProviderEgressAddress {
            id: row.try_get("id").map_err(|_| invalid())?,
            address: row
                .try_get::<String, _>("address")
                .map_err(|_| invalid())?
                .parse()
                .map_err(|_| invalid())?,
            enabled: row.try_get("enabled").map_err(|_| invalid())?,
        });
    }
    let mut account_overrides = BTreeMap::new();
    for (id, mode) in sqlx::query_as::<_, (String, Option<String>)>(
        "select a.id, o.mode from provider_accounts a
         left join provider_egress_account_overrides o on o.provider_account_id = a.id
         where a.provider_kind = 'openai'",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| unavailable())?
    {
        account_overrides.insert(
            ProviderAccountId::new(id).map_err(|_| invalid())?,
            mode.as_deref().map(parse_mode).transpose()?,
        );
    }
    let mut fixed_bindings = BTreeMap::new();
    for (id, address) in sqlx::query_as::<_, (String, String)>(
        "select a.id, f.address from provider_accounts a
         join provider_egress_fixed_affinity f
           on f.provider_kind = a.provider_kind
          and f.upstream_account_key = coalesce(a.upstream_account_id, '')
          and f.upstream_user_key = coalesce(a.upstream_user_id, '')",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| unavailable())?
    {
        fixed_bindings.insert(
            ProviderAccountId::new(id).map_err(|_| invalid())?,
            address.parse().map_err(|_| invalid())?,
        );
    }
    Ok(ProviderEgressConfig {
        revision: u64::try_from(revision).map_err(|_| invalid())?,
        default_mode: parse_mode(&default_mode)?,
        addresses,
        account_overrides,
        fixed_bindings,
    })
}

/// Call after account scheduling/proxy writes in the same transaction.
/// A proxy must never coexist with an active local-source policy.
pub(crate) async fn validate_account_proxy_egress_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    account_ids: &[String],
) -> StoreResult<()> {
    sqlx::query("select id from provider_egress_settings where id = 1 for share")
        .execute(&mut **transaction)
        .await
        .map_err(|_| unavailable())?;
    let conflict_id: Option<String> = sqlx::query_scalar(
        "select a.id from provider_accounts a
         cross join provider_egress_settings s
         left join provider_egress_account_overrides o on o.provider_account_id = a.id
         where s.id = 1 and a.provider_kind = 'openai'
           and a.id = any($1::text[]) and a.outbound_proxy_url is not null
           and coalesce(o.mode, s.default_mode) <> 'unchanged' limit 1",
    )
    .bind(account_ids)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| unavailable())?;
    if let Some(id) = conflict_id {
        return Err(conflict(&id));
    }
    Ok(())
}

/// Account mutation hook, including deletion. Preserve historical bindings while
/// advancing the egress generation so late snapshot loads cannot restore deleted accounts.
pub(crate) async fn synchronize_account_egress_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    account_ids: &[String],
) -> StoreResult<()> {
    sqlx::query("select id from provider_egress_settings where id = 1 for update")
        .execute(&mut **transaction)
        .await
        .map_err(|_| unavailable())?;
    validate_account_proxy_egress_in_transaction(transaction, account_ids).await?;
    for id in account_ids {
        ensure_fixed_affinity_in_transaction(transaction, id).await?;
    }
    sqlx::query("update provider_egress_settings set revision = revision + 1 where id = 1")
        .execute(&mut **transaction)
        .await
        .map_err(|_| unavailable())?;
    Ok(())
}

/// Import hook: call after an account is inserted/upserted, before committing.
/// History is keyed by upstream identity, not the deletable local account ID.
/// An empty pool or incomplete upstream identity does not prevent importing.
pub(crate) async fn ensure_fixed_affinity_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: &str,
) -> StoreResult<Option<Ipv6Addr>> {
    // Serialize allocation with imports and account state changes. The legacy
    // rotation cursor remains in the schema for compatibility, but selection
    // is now based on committed occupancy counts.
    sqlx::query("select id from provider_egress_settings where id = 1 for update")
        .execute(&mut **transaction)
        .await
        .map_err(|_| unavailable())?;
    let identity: Option<(String, String, String, bool, String)> = sqlx::query_as(
        "select a.provider_kind, coalesce(a.upstream_account_id, ''),
                coalesce(a.upstream_user_id, ''), a.enabled,
                coalesce(o.mode, s.default_mode)
         from provider_accounts a
         cross join provider_egress_settings s
         left join provider_egress_account_overrides o
           on o.provider_account_id = a.id
         where a.id = $1",
    )
    .bind(account_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| unavailable())?;
    let Some((provider_kind, account_key, user_key, _enabled, mode)) = identity else {
        return Ok(None);
    };
    if provider_kind != "openai" || account_key.is_empty() || user_key.is_empty() {
        return Ok(None);
    }
    let historical: Option<String> = sqlx::query_scalar(
        "select address from provider_egress_fixed_affinity
         where provider_kind = $1 and upstream_account_key = $2 and upstream_user_key = $3",
    )
    .bind(&provider_kind)
    .bind(&account_key)
    .bind(&user_key)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| unavailable())?;
    if let Some(address) = historical {
        return address.parse().map(Some).map_err(|_| invalid());
    }
    let mode = parse_mode(&mode)?;
    let fixed_mode = matches!(
        mode,
        EgressMode::FixedIpv6Reuse | EgressMode::FixedIpv6Fresh
    );
    // Non-fixed modes still receive import-time standby bindings, but their
    // occupancy is balanced separately from active fixed-mode accounts.
    let address: Option<String> = sqlx::query_scalar(
        "with occupancy as (
             select f.address, count(*) as bound_accounts
             from provider_egress_fixed_affinity f
             join provider_accounts a
               on a.provider_kind = f.provider_kind
              and a.upstream_account_id = f.upstream_account_key
              and a.upstream_user_id = f.upstream_user_key
             cross join provider_egress_settings s
             left join provider_egress_account_overrides o
               on o.provider_account_id = a.id
             where a.provider_kind = 'openai'
               and a.enabled
               and a.id <> $1
               and (
                   coalesce(o.mode, s.default_mode)
                       in ('fixed_ipv6_reuse', 'fixed_ipv6_fresh')
               ) = $2
             group by f.address
         )
         select p.address
         from provider_egress_addresses p
         left join occupancy on occupancy.address = p.address
         where p.enabled
         order by coalesce(occupancy.bound_accounts, 0) asc,
                  p.position asc, p.id asc
         limit 1",
    )
    .bind(account_id)
    .bind(fixed_mode)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| unavailable())?;
    let Some(address) = address else {
        return Ok(None);
    };
    let address = address.parse::<Ipv6Addr>().map_err(|_| invalid())?;
    sqlx::query(
        "insert into provider_egress_fixed_affinity
             (provider_kind, upstream_account_key, upstream_user_key, address)
         values ($1, $2, $3, $4)",
    )
    .bind(provider_kind)
    .bind(account_key)
    .bind(user_key)
    .bind(address.to_string())
    .execute(&mut **transaction)
    .await
    .map_err(|_| unavailable())?;
    Ok(Some(address))
}

async fn allocate_account_affinities(
    transaction: &mut Transaction<'_, Postgres>,
) -> StoreResult<()> {
    let ids: Vec<String> = sqlx::query_scalar(
        "select id from provider_accounts where provider_kind = 'openai' order by id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| unavailable())?;
    validate_account_proxy_egress_in_transaction(transaction, &ids).await?;
    for id in ids {
        ensure_fixed_affinity_in_transaction(transaction, &id).await?;
    }
    Ok(())
}

async fn advance_revision(
    transaction: &mut Transaction<'_, Postgres>,
    expected: gateway_admin::model::Revision,
) -> StoreResult<()> {
    let result = sqlx::query(
        "update provider_egress_settings set revision = revision + 1
         where id = 1 and revision = $1",
    )
    .bind(i64::try_from(expected.get()).map_err(|_| invalid())?)
    .execute(&mut **transaction)
    .await
    .map_err(|_| unavailable())?;
    if result.rows_affected() != 1 {
        return Err(StoreError::Conflict {
            entity: ENTITY,
            id: "settings".to_owned(),
            kind: ConflictKind::StaleRevision,
        });
    }
    Ok(())
}

#[async_trait]
impl ProviderEgressStore for PgProviderEgressRepository {
    async fn load(&self) -> AdminStoreResult<ProviderEgressConfig> {
        self.load_config()
            .await
            .map_err(|error| admin_store_error(ENTITY, error))
    }

    async fn replace(
        &self,
        command: ReplaceProviderEgress,
        context: &MutationContext,
    ) -> AdminStoreResult<ProviderEgressMutation> {
        let result: StoreResult<_> = async {
            let mut transaction = self.pool.begin().await.map_err(|_| unavailable())?;
            let config_revision = bump_config_revision_in_transaction(&mut transaction).await?;
            advance_revision(&mut transaction, command.expected_revision).await?;
            sqlx::query("update provider_egress_settings set default_mode = $1 where id = 1")
                .bind(mode_text(command.default_mode))
                .execute(&mut *transaction)
                .await
                .map_err(|_| unavailable())?;
            sqlx::query("delete from provider_egress_addresses")
                .execute(&mut *transaction)
                .await
                .map_err(|_| unavailable())?;
            for (position, address) in command.addresses.iter().enumerate() {
                sqlx::query(
                    "insert into provider_egress_addresses (id, address, enabled, position)
                     values ($1, $2, $3, $4)",
                )
                .bind(&address.id)
                .bind(address.address.to_string())
                .bind(address.enabled)
                .bind(i32::try_from(position).map_err(|_| invalid())?)
                .execute(&mut *transaction)
                .await
                .map_err(|_| unavailable())?;
            }
            allocate_account_affinities(&mut transaction).await?;
            append_admin_audit_event_in_transaction(
                &mut transaction,
                mutation_audit(
                    context,
                    "update",
                    "provider_egress",
                    "openai",
                    vec!["default_mode".to_owned(), "addresses".to_owned()],
                ),
                config_revision,
            )
            .await?;
            let config = load_config_in_transaction(&mut transaction).await?;
            transaction.commit().await.map_err(|_| unavailable())?;
            Ok((config_revision, config))
        }
        .await;
        let (revision, config) = result.map_err(|error| admin_store_error(ENTITY, error))?;
        Ok(ProviderEgressMutation {
            config_revision: admin_revision(revision)?,
            config,
        })
    }

    async fn set_account(
        &self,
        command: SetProviderAccountEgress,
        context: &MutationContext,
    ) -> AdminStoreResult<ProviderEgressMutation> {
        let result: StoreResult<_> = async {
            let mut transaction = self.pool.begin().await.map_err(|_| unavailable())?;
            let config_revision = bump_config_revision_in_transaction(&mut transaction).await?;
            advance_revision(&mut transaction, command.expected_revision).await?;
            sqlx::query("select id from provider_egress_settings where id = 1 for update")
                .execute(&mut *transaction)
                .await
                .map_err(|_| unavailable())?;
            let provider: Option<String> =
                sqlx::query_scalar("select provider_kind from provider_accounts where id = $1 for update")
                    .bind(command.account_id.as_str())
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(|_| unavailable())?;
            if provider.as_deref() != Some("openai") {
                return Err(invalid());
            }
            if let Some(mode) = command.mode {
                sqlx::query(
                    "insert into provider_egress_account_overrides (provider_account_id, mode)
                     values ($1, $2) on conflict (provider_account_id) do update set mode = excluded.mode",
                )
                .bind(command.account_id.as_str())
                .bind(mode_text(mode))
                .execute(&mut *transaction)
                .await
                .map_err(|_| unavailable())?;
            } else {
                sqlx::query("delete from provider_egress_account_overrides where provider_account_id = $1")
                    .bind(command.account_id.as_str())
                    .execute(&mut *transaction)
                    .await
                    .map_err(|_| unavailable())?;
            }
            validate_account_proxy_egress_in_transaction(
                &mut transaction, &[command.account_id.to_string()],
            ).await?;
            ensure_fixed_affinity_in_transaction(&mut transaction, command.account_id.as_str()).await?;
            append_admin_audit_event_in_transaction(
                &mut transaction,
                mutation_audit(
                    context, "update", "provider_account", command.account_id.as_str(),
                    vec!["egress_mode".to_owned()],
                ),
                config_revision,
            )
            .await?;
            let config = load_config_in_transaction(&mut transaction).await?;
            transaction.commit().await.map_err(|_| unavailable())?;
            Ok((config_revision, config))
        }
        .await;
        let (revision, config) = result.map_err(|error| admin_store_error(ENTITY, error))?;
        Ok(ProviderEgressMutation {
            config_revision: admin_revision(revision)?,
            config,
        })
    }
}

impl ProviderEgressStorePort for PgProviderEgressRepository {
    fn load(&self) -> BoxFuture<'_, Result<Arc<ProviderEgressConfig>, ProviderStoreError>> {
        Box::pin(async move {
            self.load_config().await.map(Arc::new).map_err(|_| {
                ProviderStoreError::new(ProviderStoreErrorKind::Unavailable, "load IPv6 egress")
            })
        })
    }

    fn ensure_fixed_affinity(
        &self,
        account_id: &ProviderAccountId,
    ) -> BoxFuture<'_, Result<Option<Ipv6Addr>, ProviderStoreError>> {
        let account_id = account_id.to_string();
        Box::pin(async move {
            let result: StoreResult<_> = async {
                let mut transaction = self.pool.begin().await.map_err(|_| unavailable())?;
                let address =
                    ensure_fixed_affinity_in_transaction(&mut transaction, &account_id).await?;
                transaction.commit().await.map_err(|_| unavailable())?;
                Ok(address)
            }
            .await;
            result.map_err(|_| {
                ProviderStoreError::new(
                    ProviderStoreErrorKind::Unavailable,
                    "allocate IPv6 affinity",
                )
            })
        })
    }
}
