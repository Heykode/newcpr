//! Dedicated login library, fenced independently from managed credentials.

use async_trait::async_trait;
use gateway_admin::{
    model::relogin::{MAX_ENTRIES, ReloginEntry, ReloginSettings},
    ports::{
        relogin::ReloginStore,
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
    },
};
use sqlx::{PgConnection, PgPool, Row};

pub struct PgReloginStore {
    pool: PgPool,
}
impl PgReloginStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn unavailable(_: impl std::fmt::Display) -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "relogin",
        "storage unavailable",
    )
}
fn conflict() -> AdminStoreError {
    AdminStoreError::new(AdminStoreErrorKind::Conflict, "relogin", "entry changed")
}

async fn save_row(
    connection: &mut PgConnection,
    entry: &ReloginEntry,
    expected: Option<u64>,
) -> AdminStoreResult<()> {
    let material = serde_json::to_value(entry).map_err(unavailable)?;
    let revision = i64::try_from(entry.revision).map_err(unavailable)?;
    let result = if let Some(expected) = expected {
        sqlx::query("update account_relogin_entries set revision=$2, material=$3 where id=$1 and revision=$4 and email=$5")
            .bind(&entry.id).bind(revision).bind(material)
            .bind(i64::try_from(expected).map_err(unavailable)?).bind(&entry.email)
            .execute(connection).await
    } else {
        sqlx::query("insert into account_relogin_entries (id,email,revision,material) values ($1,$2,$3,$4) on conflict do nothing")
            .bind(&entry.id).bind(&entry.email).bind(revision).bind(material)
            .execute(connection).await
    }.map_err(unavailable)?;
    if result.rows_affected() != 1 {
        return Err(conflict());
    }
    Ok(())
}

#[async_trait]
impl ReloginStore for PgReloginStore {
    async fn entries(&self) -> AdminStoreResult<Vec<ReloginEntry>> {
        let rows =
            sqlx::query("select material from account_relogin_entries order by email limit $1")
                .bind((MAX_ENTRIES + 1) as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(unavailable)?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_value(row.try_get("material").map_err(unavailable)?)
                    .map_err(unavailable)
            })
            .collect()
    }
    async fn save(&self, entry: &ReloginEntry, expected: Option<u64>) -> AdminStoreResult<()> {
        let mut connection = self.pool.acquire().await.map_err(unavailable)?;
        save_row(&mut connection, entry, expected).await
    }
    async fn save_batch(&self, entries: &[(ReloginEntry, Option<u64>)]) -> AdminStoreResult<()> {
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        for (entry, expected) in entries {
            save_row(&mut transaction, entry, *expected).await?;
        }
        transaction.commit().await.map_err(unavailable)
    }
    async fn delete(&self, id: &str, expected: u64) -> AdminStoreResult<()> {
        let result = sqlx::query("delete from account_relogin_entries where id=$1 and revision=$2")
            .bind(id)
            .bind(expected as i64)
            .execute(&self.pool)
            .await
            .map_err(unavailable)?;
        if result.rows_affected() != 1 {
            return Err(conflict());
        }
        Ok(())
    }
    async fn settings(&self) -> AdminStoreResult<ReloginSettings> {
        let row =
            sqlx::query("select concurrency,paused from account_relogin_settings where singleton")
                .fetch_one(&self.pool)
                .await
                .map_err(unavailable)?;
        Ok(ReloginSettings {
            concurrency: row.try_get::<i32, _>("concurrency").map_err(unavailable)? as usize,
            paused: row.try_get("paused").map_err(unavailable)?,
        })
    }
    async fn save_settings(&self, settings: &ReloginSettings) -> AdminStoreResult<()> {
        settings.validate().map_err(unavailable)?;
        sqlx::query("update account_relogin_settings set concurrency=$1,paused=$2 where singleton")
            .bind(settings.concurrency as i32)
            .bind(settings.paused)
            .execute(&self.pool)
            .await
            .map_err(unavailable)?;
        Ok(())
    }
}
