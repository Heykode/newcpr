//! Isolated, TTL- and byte-bounded sensitive conversation cache.

use gateway_core::{
    account::OpaqueProviderData,
    provider_ports::{
        MAX_PROVIDER_REPLAY_BYTES, PROVIDER_REPLAY_TTL_SECONDS, ProviderReplayPort,
        ProviderStoreError, ProviderStoreErrorKind,
    },
};
use redis::{Script, aio::ConnectionManager};

use crate::StoreResult;

const MAX_ENTRIES: usize = 2048;
const MAX_TOTAL_BYTES: usize = 128 * 1024 * 1024;
const SCRIPT: &str = r#"
local now = redis.call('TIME')
local clock = tonumber(now[1])
local function remove(key)
  redis.call('HDEL', KEYS[1], key)
  redis.call('ZREM', KEYS[2], key)
  local size = tonumber(redis.call('HGET', KEYS[3], key) or '0')
  redis.call('HDEL', KEYS[3], key)
  redis.call('HINCRBY', KEYS[3], '_total', -size)
end
for _, key in ipairs(redis.call('ZRANGEBYSCORE', KEYS[2], '-inf', clock)) do remove(key) end
if ARGV[1] == 'read' then return redis.call('HGET', KEYS[1], ARGV[2]) end
local previous = redis.call('HGET', KEYS[1], ARGV[2])
if previous then
  if previous == ARGV[3] then return 1 else return -1 end
end
local bytes = string.len(ARGV[3])
while redis.call('ZCARD', KEYS[2]) >= tonumber(ARGV[5])
   or tonumber(redis.call('HGET', KEYS[3], '_total') or '0') + bytes > tonumber(ARGV[6]) do
  local oldest = redis.call('ZRANGE', KEYS[2], 0, 0)
  if #oldest == 0 then return -2 end
  remove(oldest[1])
end
redis.call('HSET', KEYS[1], ARGV[2], ARGV[3])
redis.call('ZADD', KEYS[2], clock + tonumber(ARGV[4]), ARGV[2])
redis.call('HSET', KEYS[3], ARGV[2], bytes)
redis.call('HINCRBY', KEYS[3], '_total', bytes)
for _, key in ipairs(KEYS) do redis.call('EXPIRE', key, ARGV[4]) end
return 1
"#;

#[derive(Clone)]
pub struct RedisProviderReplayRepository {
    connection: ConnectionManager,
    prefix: String,
}

impl RedisProviderReplayRepository {
    pub fn new(connection: ConnectionManager, namespace: &str) -> StoreResult<Self> {
        Ok(Self {
            connection,
            prefix: format!("{}:{{provider-replay-v1}}", super::namespace(namespace)?),
        })
    }

    fn validate_key(key: &str) -> Result<(), ProviderStoreError> {
        if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(error(ProviderStoreErrorKind::InvalidData));
        }
        Ok(())
    }
}

impl ProviderReplayPort for RedisProviderReplayRepository {
    fn read<'a>(
        &'a self,
        key: &'a str,
    ) -> futures::future::BoxFuture<'a, Result<Option<OpaqueProviderData>, ProviderStoreError>>
    {
        Box::pin(async move {
            Self::validate_key(key)?;
            let mut connection = self.connection.clone();
            let bytes: Option<Vec<u8>> = Script::new(SCRIPT)
                .key(format!("{}:data", self.prefix))
                .key(format!("{}:expiry", self.prefix))
                .key(format!("{}:sizes", self.prefix))
                .arg("read")
                .arg(key)
                .invoke_async(&mut connection)
                .await
                .map_err(|_| error(ProviderStoreErrorKind::Unavailable))?;
            let Some(bytes) = bytes else { return Ok(None) };
            if bytes.len() > MAX_PROVIDER_REPLAY_BYTES {
                return Err(error(ProviderStoreErrorKind::InvalidData));
            }
            let fields = serde_json::from_slice(&bytes)
                .map_err(|_| error(ProviderStoreErrorKind::InvalidData))?;
            Ok(Some(OpaqueProviderData::new(fields)))
        })
    }

    fn write<'a>(
        &'a self,
        key: &'a str,
        payload: &'a OpaqueProviderData,
    ) -> futures::future::BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            Self::validate_key(key)?;
            let bytes = serde_json::to_vec(payload.expose_to_provider())
                .map_err(|_| error(ProviderStoreErrorKind::InvalidData))?;
            if bytes.len() > MAX_PROVIDER_REPLAY_BYTES {
                return Err(error(ProviderStoreErrorKind::InvalidData));
            }
            let mut connection = self.connection.clone();
            let outcome: i64 = Script::new(SCRIPT)
                .key(format!("{}:data", self.prefix))
                .key(format!("{}:expiry", self.prefix))
                .key(format!("{}:sizes", self.prefix))
                .arg("write")
                .arg(key)
                .arg(bytes)
                .arg(PROVIDER_REPLAY_TTL_SECONDS)
                .arg(MAX_ENTRIES)
                .arg(MAX_TOTAL_BYTES)
                .invoke_async(&mut connection)
                .await
                .map_err(|_| error(ProviderStoreErrorKind::Unavailable))?;
            match outcome {
                1 => Ok(()),
                -1 => Err(error(ProviderStoreErrorKind::Conflict)),
                _ => Err(error(ProviderStoreErrorKind::InvalidData)),
            }
        })
    }
}

fn error(kind: ProviderStoreErrorKind) -> ProviderStoreError {
    ProviderStoreError::new(kind, "provider replay cache")
}
