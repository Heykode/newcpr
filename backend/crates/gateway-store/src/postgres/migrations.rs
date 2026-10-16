//! Distinguish a fresh installation under the same lock that guards migrations.

use sqlx::{
    PgConnection,
    migrate::{Migrate, MigrateError, Migrator},
};

/// Run frozen migrations without resetting saved settings on existing databases.
pub async fn migrate_schema(
    connection: &mut PgConnection,
    migrator: &Migrator,
) -> Result<(), MigrateError> {
    connection.lock().await?;
    let result = async {
        sqlx::query(
            "select set_config('cpr.fresh_install',
             case when to_regclass('runtime_settings') is null then 'true' else 'false' end, false)",
        )
        .execute(&mut *connection)
        .await?;
        // PostgreSQL advisory locks are reentrant. Keep the outer lock until
        // fresh-install detection and the migrator's entire run have completed.
        migrator.run(&mut *connection).await
    }
    .await;
    let cleared = sqlx::query("select set_config('cpr.fresh_install', 'false', false)")
        .execute(&mut *connection)
        .await
        .map(|_| ())
        .map_err(MigrateError::from);
    let unlocked = connection.unlock().await;
    result.and(cleared).and(unlocked)
}
