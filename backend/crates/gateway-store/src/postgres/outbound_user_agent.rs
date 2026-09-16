//! Targeted outbound user-agent persistence without replacing runtime settings.

use gateway_core::provider_ports::ProviderUserAgentOverride;
use gateway_core::routing::ProviderKind;
use sqlx::PgPool;

use super::{
    AdminAuditEvent, PgControlPlaneRepository, append_admin_audit_event_in_transaction,
    bump_config_revision_in_transaction,
};
use crate::{Revision, StoreError, StoreResult, postgres_unavailable};

pub(crate) async fn load_user_agent_override(
    pool: &PgPool,
    provider_kind: &ProviderKind,
) -> StoreResult<ProviderUserAgentOverride> {
    let row = sqlx::query_as::<_, (String, Option<String>)>(
        "select mode, custom_user_agent from provider_outbound_user_agents
         where provider_kind = $1",
    )
    .bind(provider_kind.as_str())
    .fetch_optional(pool)
    .await
    .map_err(|_| postgres_unavailable("load outbound user-agent"))?;
    match row {
        None => Ok(ProviderUserAgentOverride::Default),
        Some((mode, None)) if mode == "default" => Ok(ProviderUserAgentOverride::Default),
        Some((mode, Some(user_agent))) if mode == "custom" => {
            validate_custom(Some(&user_agent))?;
            Ok(ProviderUserAgentOverride::Custom { user_agent })
        }
        Some(_) => Err(invalid_selection()),
    }
}

impl PgControlPlaneRepository {
    pub async fn load_user_agent_override(
        &self,
        provider_kind: &ProviderKind,
    ) -> StoreResult<ProviderUserAgentOverride> {
        load_user_agent_override(&self.pool, provider_kind).await
    }

    pub async fn replace_user_agent_override(
        &self,
        provider_kind: &ProviderKind,
        selection: ProviderUserAgentOverride,
        audit: AdminAuditEvent,
    ) -> StoreResult<Revision> {
        let (mode, custom) = match &selection {
            ProviderUserAgentOverride::Default => ("default", None),
            ProviderUserAgentOverride::Custom { user_agent } => {
                ("custom", Some(user_agent.as_str()))
            }
        };
        validate_custom(custom)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin outbound user-agent update"))?;
        let revision = bump_config_revision_in_transaction(&mut transaction).await?;
        sqlx::query(
            "insert into provider_outbound_user_agents
                 (provider_kind, mode, custom_user_agent)
             values ($1, $2, $3)
             on conflict (provider_kind) do update
             set mode = excluded.mode, custom_user_agent = excluded.custom_user_agent,
                 updated_at = now()",
        )
        .bind(provider_kind.as_str())
        .bind(mode)
        .bind(custom)
        .execute(&mut *transaction)
        .await
        .map_err(|_| postgres_unavailable("save outbound user-agent"))?;
        append_admin_audit_event_in_transaction(&mut transaction, audit, revision).await?;
        transaction
            .commit()
            .await
            .map_err(|_| postgres_unavailable("commit outbound user-agent update"))?;
        Ok(revision)
    }
}

fn validate_custom(value: Option<&str>) -> StoreResult<()> {
    if value.is_some_and(|value| {
        value.trim().is_empty()
            || value.len() > 512
            || !value.is_ascii()
            || value.bytes().any(|byte| byte.is_ascii_control())
    }) {
        return Err(invalid_selection());
    }
    Ok(())
}

fn invalid_selection() -> StoreError {
    StoreError::InvalidData {
        entity: "provider outbound user-agent",
        message: "outbound user-agent selection is invalid".to_owned(),
    }
}
