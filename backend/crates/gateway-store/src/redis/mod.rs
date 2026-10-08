//! 可丢失、可从 PostgreSQL 或 Provider 重建的 Redis 协调状态。

use chrono::{DateTime, SecondsFormat, Utc};
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use async_trait::async_trait;

mod admin_account_runtime;
mod artifact_profile;
mod capacity_wait;
mod client_admission;
mod coordination_buffer;
mod credential_cooldown;
mod credential_leases;
mod credential_state;
mod native_continuation;
mod oauth_pending;
mod provider_circuit;
mod provider_replay;
mod provider_session_affinity;
mod provider_session_exclusion;
mod runtime_change;
pub(crate) mod worker_lease;

pub use admin_account_runtime::*;
pub use artifact_profile::*;
pub use capacity_wait::CapacityWaitCleanupWriter;
pub use client_admission::*;
pub use coordination_buffer::*;
pub use credential_cooldown::*;
pub use credential_leases::*;
pub use credential_state::*;
pub use native_continuation::*;
pub use oauth_pending::*;
pub use provider_circuit::*;
pub use provider_replay::*;
pub use provider_session_affinity::*;
pub use provider_session_exclusion::*;
pub use runtime_change::*;

use crate::{StoreError, StoreResult, redis_unavailable, require_nonempty};

pub(crate) const MAX_REDIS_EXACT_INTEGER: u64 = (1_u64 << 53) - 1;

/// Redis 中可丢失的管理员会话事实；认证秘密不属于该结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminSessionRecord {
    pub admin_user_id: String,
    pub expires_at: DateTime<Utc>,
    pub credential_fingerprint: String,
    pub absolute_expires_at: Option<DateTime<Utc>>,
}

impl AdminSessionRecord {
    fn validate(&self) -> StoreResult<u64> {
        require_nonempty("admin session", "admin_user_id", &self.admin_user_id)?;
        if self
            .absolute_expires_at
            .is_some_and(|limit| self.expires_at > limit)
        {
            return Err(admin_auth_invalid(
                "session expiry exceeds its absolute lifetime",
            ));
        }
        let expires_at_millis = u64::try_from(self.expires_at.timestamp_millis())
            .map_err(|_| admin_auth_invalid("session expiry must be after the Unix epoch"))?;
        if expires_at_millis > MAX_REDIS_EXACT_INTEGER {
            return Err(admin_auth_invalid(
                "session expiry is outside the supported range",
            ));
        }
        let now_millis = u64::try_from(Utc::now().timestamp_millis())
            .map_err(|_| admin_auth_invalid("current time is outside the supported range"))?;
        if expires_at_millis <= now_millis {
            return Err(admin_auth_invalid("session expiry must be in the future"));
        }
        Ok(expires_at_millis)
    }
}

/// 管理员会话的 Redis 基础设施端口。
#[async_trait]
pub trait AdminAuthStateRepository: Send + Sync {
    async fn renew_admin_session(
        &self,
        session_id: &str,
        expected: &AdminSessionRecord,
        expires_at: DateTime<Utc>,
    ) -> StoreResult<Option<AdminSessionRecord>>;

    async fn consume_password_change_attempt(
        &self,
        admin_user_id: &str,
        limit: u32,
        window_seconds: u64,
    ) -> StoreResult<bool>;

    async fn load_admin_session(&self, session_id: &str)
    -> StoreResult<Option<AdminSessionRecord>>;
    async fn store_admin_session(
        &self,
        session_id: &str,
        session: &AdminSessionRecord,
    ) -> StoreResult<()>;
    async fn delete_admin_session(
        &self,
        session_id: &str,
    ) -> StoreResult<Option<AdminSessionRecord>>;
}

/// Redis 管理员会话 adapter。
#[derive(Clone)]
pub struct RedisAdminAuthStateRepository {
    connection: ConnectionManager,
    namespace: String,
}

impl RedisAdminAuthStateRepository {
    pub fn new(connection: ConnectionManager, key_namespace: &str) -> StoreResult<Self> {
        Ok(Self {
            connection,
            namespace: format!("{}:admin-auth:v1", namespace(key_namespace)?),
        })
    }

    fn session_key(&self, session_id: &str) -> StoreResult<String> {
        let fingerprint = resource_fingerprint("admin session", session_id)?;
        Ok(format!("{}:session:{{{fingerprint}}}", self.namespace))
    }
}

#[async_trait]
impl AdminAuthStateRepository for RedisAdminAuthStateRepository {
    async fn renew_admin_session(
        &self,
        session_id: &str,
        expected: &AdminSessionRecord,
        expires_at: DateTime<Utc>,
    ) -> StoreResult<Option<AdminSessionRecord>> {
        if expected.expires_at <= Utc::now()
            || expected.credential_fingerprint.is_empty()
            || expires_at < expected.expires_at
            || expected
                .absolute_expires_at
                .is_none_or(|limit| expires_at > limit)
        {
            return Err(admin_auth_invalid(
                "session renewal is outside its lifetime",
            ));
        }
        let renewed = AdminSessionRecord {
            expires_at,
            ..expected.clone()
        };
        let expiry = renewed.validate()?;
        let payload: Option<String> = redis::Script::new(
            r#"
local current = redis.call('GET', KEYS[1])
if current == ARGV[1] then
    redis.call('SET', KEYS[1], ARGV[2], 'PXAT', ARGV[3], 'XX')
    return ARGV[2]
end
return current
"#,
        )
        .key(self.session_key(session_id)?)
        .arg(encode_admin_session(expected)?)
        .arg(encode_admin_session(&renewed)?)
        .arg(expiry)
        .invoke_async(&mut self.connection.clone())
        .await
        .map_err(|error| redis_unavailable("renew admin session").with_source(error))?;
        payload
            .map(|value| decode_admin_session(&value))
            .transpose()
    }

    async fn consume_password_change_attempt(
        &self,
        admin_user_id: &str,
        limit: u32,
        window_seconds: u64,
    ) -> StoreResult<bool> {
        if limit == 0 || window_seconds == 0 || window_seconds > 86_400 {
            return Err(admin_auth_invalid("invalid password change limit"));
        }
        let fingerprint = resource_fingerprint("admin user", admin_user_id)?;
        let key = format!("{}:password-change:{{{fingerprint}}}", self.namespace);
        let mut connection = self.connection.clone();
        let allowed: bool = redis::Script::new(
            "local count = tonumber(redis.call('GET', KEYS[1]) or '0')
             if count >= tonumber(ARGV[1]) then return 0 end
             count = redis.call('INCR', KEYS[1])
             if count == 1 then redis.call('EXPIRE', KEYS[1], ARGV[2]) end
             return 1",
        )
        .key(key)
        .arg(limit)
        .arg(window_seconds)
        .invoke_async(&mut connection)
        .await
        .map_err(|error| redis_unavailable("consume password change attempt").with_source(error))?;
        Ok(allowed)
    }

    async fn load_admin_session(
        &self,
        session_id: &str,
    ) -> StoreResult<Option<AdminSessionRecord>> {
        let key = self.session_key(session_id)?;
        let mut connection = self.connection.clone();
        let payload = redis::cmd("GET")
            .arg(key)
            .query_async::<Option<String>>(&mut connection)
            .await
            .map_err(|error| redis_unavailable("load admin session").with_source(error))?;
        payload
            .map(|value| decode_admin_session(&value))
            .transpose()
    }

    async fn store_admin_session(
        &self,
        session_id: &str,
        session: &AdminSessionRecord,
    ) -> StoreResult<()> {
        let key = self.session_key(session_id)?;
        let expires_at_millis = session.validate()?;
        let payload = encode_admin_session(session)?;
        let mut connection = self.connection.clone();
        redis::cmd("SET")
            .arg(key)
            .arg(payload)
            .arg("PXAT")
            .arg(expires_at_millis)
            .query_async::<String>(&mut connection)
            .await
            .map_err(|error| redis_unavailable("store admin session").with_source(error))?;
        Ok(())
    }

    async fn delete_admin_session(
        &self,
        session_id: &str,
    ) -> StoreResult<Option<AdminSessionRecord>> {
        let key = self.session_key(session_id)?;
        let mut connection = self.connection.clone();
        let payload = redis::cmd("GETDEL")
            .arg(key)
            .query_async::<Option<String>>(&mut connection)
            .await
            .map_err(|error| redis_unavailable("delete admin session").with_source(error))?;
        payload
            .map(|value| decode_admin_session(&value))
            .transpose()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminSessionWire {
    admin_user_id: String,
    expires_at: String,
    #[serde(default)]
    credential_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    absolute_expires_at: Option<String>,
}

fn encode_admin_session(session: &AdminSessionRecord) -> StoreResult<String> {
    serde_json::to_string(&AdminSessionWire {
        admin_user_id: session.admin_user_id.clone(),
        credential_fingerprint: session.credential_fingerprint.clone(),
        absolute_expires_at: session
            .absolute_expires_at
            .map(|value| value.to_rfc3339_opts(SecondsFormat::Nanos, true)),
        expires_at: session
            .expires_at
            .to_rfc3339_opts(SecondsFormat::Nanos, true),
    })
    .map_err(|_| admin_auth_invalid("session value cannot be encoded"))
}

fn decode_admin_session(value: &str) -> StoreResult<AdminSessionRecord> {
    let wire: AdminSessionWire = serde_json::from_str(value)
        .map_err(|_| admin_auth_invalid("Redis returned an invalid session value"))?;
    require_nonempty("admin session", "admin_user_id", &wire.admin_user_id)?;
    let expires_at = DateTime::parse_from_rfc3339(&wire.expires_at)
        .map_err(|_| admin_auth_invalid("Redis returned an invalid session expiry"))?
        .with_timezone(&Utc);
    Ok(AdminSessionRecord {
        admin_user_id: wire.admin_user_id,
        expires_at,
        credential_fingerprint: wire.credential_fingerprint,
        absolute_expires_at: wire
            .absolute_expires_at
            .as_deref()
            .map(|value| DateTime::parse_from_rfc3339(value).map(|value| value.with_timezone(&Utc)))
            .transpose()
            .map_err(|_| admin_auth_invalid("Redis returned an invalid session lifetime"))?,
    })
}

fn admin_auth_invalid(message: &str) -> StoreError {
    StoreError::InvalidData {
        entity: "admin authentication state",
        message: message.to_owned(),
    }
}

pub(crate) fn resource_fingerprint(entity: &'static str, value: &str) -> StoreResult<String> {
    require_nonempty(entity, "resource ID", value)?;
    Ok(hex::encode(Sha256::digest(value.as_bytes())))
}

pub(crate) fn namespace(value: &str) -> StoreResult<String> {
    require_nonempty("Redis namespace", "namespace", value)?;
    if value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(StoreError::InvalidData {
            entity: "Redis namespace",
            message: "namespace contains unsupported characters".to_owned(),
        });
    }
    Ok(value.to_owned())
}
/// Bound account-state read fan-out, independently of model execution capacity.
pub(crate) const ACCOUNT_STATE_READ_CONCURRENCY: usize = 128;
