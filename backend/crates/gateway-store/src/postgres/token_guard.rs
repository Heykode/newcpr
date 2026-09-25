//! Guard observations never contain request bodies or login material.

use async_trait::async_trait;
use gateway_admin::{
    model::{
        MutationContext,
        token_guard::{TokenGuardConfig, TokenGuardEvent},
    },
    ports::{
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
        token_guard::TokenGuardStore,
    },
};
use sqlx::PgPool;

pub struct PgTokenGuardStore {
    pool: PgPool,
}

impl PgTokenGuardStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn unavailable(_: impl std::fmt::Display) -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "token guard",
        "storage unavailable",
    )
}

#[async_trait]
impl TokenGuardStore for PgTokenGuardStore {
    async fn audit_run(&self, context: &MutationContext) -> AdminStoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        super::insert_admin_audit_event(
            &mut tx,
            crate::mutation_audit(
                context,
                "token_guard.run",
                "account_token_guard_config",
                "1",
                vec![],
            ),
        )
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)
    }

    async fn config(&self) -> AdminStoreResult<TokenGuardConfig> {
        let value: serde_json::Value =
            sqlx::query_scalar("select config from account_token_guard_config where singleton")
                .fetch_one(&self.pool)
                .await
                .map_err(unavailable)?;
        let config: TokenGuardConfig = serde_json::from_value(value).map_err(unavailable)?;
        config.validate().map_err(unavailable)?;
        Ok(config)
    }

    async fn configure(
        &self,
        config: &TokenGuardConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        config.validate().map_err(unavailable)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let groups: i64 =
            sqlx::query_scalar("select count(*) from account_groups where id=any($1)")
                .bind(&config.group_ids)
                .fetch_one(&mut *tx)
                .await
                .map_err(unavailable)?;
        if groups as usize != config.group_ids.len() {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                "token guard",
                "group not found",
            ));
        }
        sqlx::query("update account_token_guard_config set config=$1 where singleton")
            .bind(serde_json::to_value(config).map_err(unavailable)?)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        super::admin_security_audit::insert_admin_audit_event(
            &mut tx,
            crate::mutation_audit(
                context,
                "token_guard.configure",
                "account_token_guard_config",
                "1",
                vec!["config".into()],
            ),
        )
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)
    }

    async fn latest(&self) -> AdminStoreResult<Vec<TokenGuardEvent>> {
        let rows: Vec<serde_json::Value> = sqlx::query_scalar(
            "select distinct on (account_id) event from account_token_guard_events order by account_id, observed_at desc, id desc"
        ).fetch_all(&self.pool).await.map_err(unavailable)?;
        rows.into_iter()
            .map(|value| serde_json::from_value(value).map_err(unavailable))
            .collect()
    }

    async fn events(&self) -> AdminStoreResult<Vec<TokenGuardEvent>> {
        let rows: Vec<serde_json::Value> = sqlx::query_scalar(
            "select event from account_token_guard_events order by observed_at desc, id desc limit 100"
        ).fetch_all(&self.pool).await.map_err(unavailable)?;
        rows.into_iter()
            .map(|value| serde_json::from_value(value).map_err(unavailable))
            .collect()
    }

    async fn record(&self, event: &TokenGuardEvent) -> AdminStoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        // Deletion or reauthorization during a probe must not resurrect stale observations.
        sqlx::query(
            "insert into account_token_guard_events (account_id,observed_at,event)
             select id,$2,$3 from provider_accounts
             where id=$1 and credential_revision >= $4 and turn_state_binding_revision <= $4",
        )
        .bind(&event.account_id)
        .bind(event.observed_at)
        .bind(serde_json::to_value(event).map_err(unavailable)?)
        .bind(i64::try_from(event.observed_revision).map_err(unavailable)?)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        sqlx::query(
            "delete from account_token_guard_events where observed_at < now() - interval '14 days'",
        )
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)
    }
}
