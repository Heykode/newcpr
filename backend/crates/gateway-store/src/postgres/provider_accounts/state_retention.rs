//! Rebind still-valid slots atomically with a verified credential replacement.

use super::*;

pub(super) struct StateOwnerSnapshot {
    id: String,
    provider: String,
    user: Option<String>,
    workspace: Option<String>,
    plan: Option<String>,
    binding: i64,
    credential: PlaintextCredential,
}

impl StateOwnerSnapshot {
    fn from_row(row: &sqlx::postgres::PgRow) -> StoreResult<Self> {
        Ok(Self {
            id: get(row, "id")?,
            provider: get(row, "provider_kind")?,
            user: get(row, "upstream_user_id")?,
            workspace: get(row, "upstream_account_id")?,
            plan: get(row, "plan_type")?,
            binding: get(row, "turn_state_binding_revision")?,
            credential: PlaintextCredential::new(
                get::<serde_json::Value>(row, "provider_credentials_json")?
                    .as_object()
                    .cloned()
                    .ok_or_else(|| invalid("invalid provider credential object"))?,
            ),
        })
    }

    pub(super) async fn capture(
        transaction: &mut Transaction<'_, Postgres>,
        id: &str,
    ) -> StoreResult<Option<Self>> {
        sqlx::query(
            "select id, provider_kind, upstream_user_id, upstream_account_id, plan_type,
                    turn_state_binding_revision, provider_credentials_json
             from provider_accounts where id = $1 for update",
        )
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|_| postgres_unavailable("lock State credential owner"))?
        .as_ref()
        .map(Self::from_row)
        .transpose()
    }

    pub(super) async fn capture_import(
        transaction: &mut Transaction<'_, Postgres>,
        account: &NewProviderAccount,
    ) -> StoreResult<Option<Self>> {
        sqlx::query(
            "select id, provider_kind, upstream_user_id, upstream_account_id, plan_type,
                    turn_state_binding_revision, provider_credentials_json
             from provider_accounts
             where provider_kind = $1 and upstream_user_id = $2
               and coalesce(upstream_account_id, '') = coalesce($3, '') for update",
        )
        .bind(&account.provider_kind)
        .bind(&account.upstream_user_id)
        .bind(&account.upstream_account_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|_| postgres_unavailable("lock imported State owner"))?
        .as_ref()
        .map(Self::from_row)
        .transpose()
    }
}

impl PgProviderAccountRepository {
    pub(super) async fn retain_turn_state(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        previous: Option<StateOwnerSnapshot>,
    ) -> StoreResult<()> {
        let Some(previous) = previous else {
            return Ok(());
        };
        let Some(current) = StateOwnerSnapshot::capture(transaction, &previous.id).await? else {
            return Ok(());
        };
        if current.binding == previous.binding
            || current.provider != previous.provider
            || !same_nonempty(previous.user.as_deref(), current.user.as_deref())
            || !same_nonempty(previous.workspace.as_deref(), current.workspace.as_deref())
            || previous
                .plan
                .as_deref()
                .zip(current.plan.as_deref())
                .is_none_or(|(old, new)| {
                    old.trim().is_empty() || !old.trim().eq_ignore_ascii_case(new.trim())
                })
        {
            return Ok(());
        }
        let Some(codec) = self.device_codecs.get(&current.provider)? else {
            return Ok(());
        };
        if !codec.can_retain_turn_state(&previous.credential, &current.credential) {
            return Ok(());
        }
        // Account lock precedes State locks, as on the collector path. Advancing
        // both generations fences late probes and gives new WS chains a new pool.
        // Never renew clocks or resurrect rows belonging to an earlier credential.
        sqlx::query(
            "update provider_turn_states
             set credential_revision = $3, state_version = state_version + 1,
                 active_state = case when active_expires_at > now() then active_state end,
                 active_issued_at = case when active_expires_at > now() then active_issued_at end,
                 active_captured_at = case when active_expires_at > now() then active_captured_at end,
                 active_expires_at = case when active_expires_at > now() then active_expires_at end,
                 standby_state = case when standby_expires_at > now() then standby_state end,
                 standby_issued_at = case when standby_expires_at > now() then standby_issued_at end,
                 standby_captured_at = case when standby_expires_at > now() then standby_captured_at end,
                 standby_expires_at = case when standby_expires_at > now() then standby_expires_at end,
                 refresh_status = case when active_expires_at > now() then 'ready' else 'missing' end,
                 updated_at = now()
             where provider_account_id = $1 and credential_revision = $2",
        )
        .bind(&previous.id)
        .bind(previous.binding)
        .bind(current.binding)
        .execute(&mut **transaction)
        .await
        .map_err(|_| postgres_unavailable("retain State after credential renewal"))?;
        Ok(())
    }
}

fn same_nonempty(old: Option<&str>, new: Option<&str>) -> bool {
    old.zip(new)
        .is_some_and(|(old, new)| !old.trim().is_empty() && old == new)
}
