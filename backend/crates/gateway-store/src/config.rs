//! Store 启动配置、环境变量解析与校验。

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use super::*;

pub(crate) const DATABASE_URL_ENV: &str = "CPR_DATABASE_URL";
pub(crate) const REDIS_URL_ENV: &str = "CPR_REDIS_URL";
pub(crate) const DATABASE_PASSWORD_ENV: &str = "CPR_DATABASE_PASSWORD";
pub(crate) const REDIS_PASSWORD_ENV: &str = "CPR_REDIS_PASSWORD";
pub(crate) const POSTGRES_STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const POSTGRES_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const POSTGRES_IDLE_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const POSTGRES_HEALTH_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(450);
pub(crate) const POSTGRES_HEALTH_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Store 自己拥有并校验的启动配置。
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreConfig {
    pub(crate) database: StoreConnectionConfig,
    pub(crate) redis: StoreConnectionConfig,
    #[serde(default)]
    pub(crate) pool: StorePoolConfig,
    #[serde(skip)]
    backup_staging_dir: PathBuf,
    #[serde(skip)]
    pub(crate) log_files: Option<Arc<dyn gateway_admin::ports::log_cleanup::LogFileMaintenance>>,
}

/// PostgreSQL 连接池预算；acquire 超时决定池耗尽时快速失败而非排队积压。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StorePoolConfig {
    pub max_connections: u32,
    pub acquire_timeout_seconds: u64,
    pub observability_query_timeout_seconds: u64,
}

impl Default for StorePoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 20,
            acquire_timeout_seconds: 5,
            observability_query_timeout_seconds: 120,
        }
    }
}

impl StorePoolConfig {
    pub(crate) fn validate(&self) -> StoreResult<()> {
        if self.max_connections < 2 || self.acquire_timeout_seconds == 0 {
            return Err(StoreError::InvalidData {
                entity: "store config",
                message:
                    "pool.max_connections must be at least 2 and acquire timeout must be positive"
                        .to_owned(),
            });
        }
        if self.observability_query_timeout_seconds == 0
            || self.observability_query_timeout_seconds > i32::MAX as u64 / 1000
        {
            return Err(StoreError::InvalidData {
                entity: "store config",
                message:
                    "observability query timeout must be positive and within PostgreSQL limits"
                        .to_owned(),
            });
        }
        Ok(())
    }

    /// 管理观测查询可并发占用的连接数；始终为数据面保留约 20% 的池容量。
    #[must_use]
    pub const fn observability_max_connections(self) -> u32 {
        self.max_connections - self.max_connections.div_ceil(5)
    }

    pub(crate) const fn acquire_timeout(self) -> Duration {
        Duration::from_secs(self.acquire_timeout_seconds)
    }
}

impl StoreConfig {
    #[must_use]
    pub fn with_log_files(
        mut self,
        files: Arc<dyn gateway_admin::ports::log_cleanup::LogFileMaintenance>,
    ) -> Self {
        self.log_files = Some(files);
        self
    }
    pub fn resolve_and_validate(&mut self, runtime_data_dir: &Path) -> StoreResult<()> {
        if runtime_data_dir.as_os_str().is_empty() {
            return Err(StoreError::InvalidData {
                entity: "store config",
                message: "runtime_data_dir must not be empty".to_owned(),
            });
        }
        self.backup_staging_dir = runtime_data_dir.join("backup-staging");
        self.validate_resolved()
    }

    pub(crate) fn validate_resolved(&mut self) -> StoreResult<()> {
        if let Some(url) = optional_environment_value(DATABASE_URL_ENV)? {
            self.database.url = url;
        }
        if let Some(url) = optional_environment_value(REDIS_URL_ENV)? {
            self.redis.url = url;
        }
        if let Some(password) = optional_environment_value(DATABASE_PASSWORD_ENV)? {
            self.database.password = password;
        }
        if let Some(password) = optional_environment_value(REDIS_PASSWORD_ENV)? {
            self.redis.password = password;
        }
        self.database.validate("database")?;
        self.redis.validate("redis")?;
        self.pool.validate()?;
        if self.backup_staging_dir.as_os_str().is_empty() {
            return Err(StoreError::InvalidData {
                entity: "store config",
                message: "runtime_data_dir was not resolved".to_owned(),
            });
        }
        Ok(())
    }

    pub(crate) fn database_url(&self) -> StoreResult<String> {
        self.database.connection_url("database")
    }

    pub(crate) fn redis_url(&self) -> StoreResult<String> {
        self.redis.connection_url("redis")
    }

    /// 返回由统一运行数据根目录派生的备份暂存目录。
    #[must_use]
    pub fn backup_staging_dir(&self) -> &Path {
        &self.backup_staging_dir
    }
}

pub(crate) fn optional_environment_value(name: &'static str) -> StoreResult<Option<String>> {
    match std::env::var(name) {
        Ok(value) if value.trim().is_empty() => Err(StoreError::InvalidData {
            entity: "store config",
            message: format!("environment variable {name} is empty"),
        }),
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(StoreError::InvalidData {
            entity: "store config",
            message: format!("environment variable {name} is not Unicode"),
        }),
    }
}

impl fmt::Debug for StoreConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoreConfig")
            .field("database", &"[REDACTED]")
            .field("redis", &"[REDACTED]")
            .field("pool", &self.pool)
            .finish()
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoreConnectionConfig {
    pub(crate) url: String,
    pub(crate) password: String,
}

impl StoreConnectionConfig {
    fn validate(&self, field: &'static str) -> StoreResult<()> {
        require_nonempty("store config", field, &self.url)?;
        if !(16..=128).contains(&self.password.len())
            || self.password.chars().any(char::is_control)
            || self.password.trim().is_empty()
        {
            return Err(StoreError::InvalidData {
                entity: "store config",
                message: format!(
                    "{field}.password must be 16 to 128 bytes without control characters and must not be blank"
                ),
            });
        }
        self.connection_url(field).map(|_| ())
    }

    fn connection_url(&self, field: &'static str) -> StoreResult<String> {
        let mut url = url::Url::parse(&self.url).map_err(|_| StoreError::InvalidData {
            entity: "store config",
            message: format!("{field}.url is invalid"),
        })?;
        if url.password().is_some() {
            return Err(StoreError::InvalidData {
                entity: "store config",
                message: format!("{field}.url must not contain a password"),
            });
        }
        url.set_password(Some(&self.password.replace('%', "%25")))
            .map_err(|()| StoreError::InvalidData {
                entity: "store config",
                message: format!("{field}.url cannot carry credentials"),
            })?;
        Ok(url.to_string())
    }
}

#[cfg(test)]
mod connection_password_tests {
    use super::*;
    use ::redis as redis_client;

    #[test]
    fn passwords_preserve_reserved_and_literal_percent_sequences() {
        use std::str::FromStr;
        for password in ["example-password!@:/?#[]%41%25", "a".repeat(128).as_str()] {
            let database = StoreConnectionConfig {
                url: "postgres://example@localhost/example".to_owned(),
                password: password.to_owned(),
            };
            database.validate("database").unwrap();
            let _parsed = sqlx::postgres::PgConnectOptions::from_str(
                &database.connection_url("database").unwrap(),
            )
            .unwrap();
            let redis = StoreConnectionConfig {
                url: "redis://localhost/".to_owned(),
                password: password.to_owned(),
            };
            redis.validate("redis").unwrap();
            redis_client::Client::open(redis.connection_url("redis").unwrap()).unwrap();
        }
    }

    #[test]
    fn password_validation_rejects_short_long_blank_and_control_characters() {
        for password in [
            "a".repeat(15),
            "a".repeat(129),
            " ".repeat(16),
            "example-password\n".to_owned(),
            "example-password\0".to_owned(),
        ] {
            let config = StoreConnectionConfig {
                url: "postgres://example@localhost/example".to_owned(),
                password,
            };
            assert!(config.validate("database").is_err());
        }
    }

    #[tokio::test]
    async fn literal_percent_passwords_authenticate_against_real_stores() {
        let (Ok(database_url), Ok(redis_url)) = (
            std::env::var("CPR_TEST_DATABASE_URL"),
            std::env::var("CPR_TEST_REDIS_URL"),
        ) else {
            assert!(
                std::env::var("CI").is_err(),
                "isolated stores required in CI"
            );
            return;
        };
        let admin = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .unwrap();
        let username = format!("cpr_password_{}", uuid::Uuid::new_v4().simple());
        let password = "Example%41%25%FFPassword9!";
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE ROLE {username} LOGIN PASSWORD '{password}'"
        )))
        .execute(&admin)
        .await
        .unwrap();
        let mut url = url::Url::parse(&database_url).unwrap();
        url.set_username(&username).unwrap();
        url.set_password(None).unwrap();
        let config = StoreConnectionConfig {
            url: url.to_string(),
            password: password.to_owned(),
        };
        let result = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&config.connection_url("database").unwrap())
            .await;
        if let Ok(pool) = &result {
            pool.close().await;
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP ROLE {username}")))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        assert!(
            result.is_ok(),
            "PostgreSQL must preserve literal percent sequences"
        );

        let mut redis_admin = redis_client::Client::open(redis_url.as_str())
            .unwrap()
            .get_connection_manager()
            .await
            .unwrap();
        redis_client::cmd("ACL")
            .arg("SETUSER")
            .arg(&username)
            .arg("on")
            .arg(format!(">{password}"))
            .arg("~*")
            .arg("+ping")
            .query_async::<()>(&mut redis_admin)
            .await
            .unwrap();
        let mut url = url::Url::parse(&redis_url).unwrap();
        url.set_username(&username).unwrap();
        url.set_password(None).unwrap();
        let config = StoreConnectionConfig {
            url: url.to_string(),
            password: password.to_owned(),
        };
        let result = async {
            let mut connection =
                redis_client::Client::open(config.connection_url("redis").unwrap())?
                    .get_multiplexed_async_connection()
                    .await?;
            redis_client::cmd("PING")
                .query_async::<String>(&mut connection)
                .await
        }
        .await;
        redis_client::cmd("ACL")
            .arg("DELUSER")
            .arg(&username)
            .query_async::<i64>(&mut redis_admin)
            .await
            .unwrap();
        assert_eq!(result.unwrap(), "PONG");
    }
}
