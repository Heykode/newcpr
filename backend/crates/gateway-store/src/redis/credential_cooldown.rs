//! 请求调度用的可丢失 Provider 级 cooldown Redis 存储。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gateway_admin::model::accounts::AccountRuntimeSnapshot;
use gateway_core::{
    account::{CredentialRevision, ProviderAccountId},
    provider_ports::{
        ProviderCooldown, ProviderCooldownPort, ProviderCooldownScope, ProviderScopedCooldown,
        ProviderStoreError, ProviderStoreErrorKind,
    },
};
use redis::{Script, aio::ConnectionManager};
use uuid::Uuid;

use crate::{Revision, StoreError, StoreResult, redis_unavailable, require_nonempty};

use super::{namespace, resource_fingerprint};

const WRITE_SCRIPT: &str = r#"
local current = tonumber(redis.call('HGET', KEYS[1], 'revision') or '0')
local incoming = tonumber(ARGV[1])
local incoming_until = tonumber(ARGV[2])
if current > incoming then return 0 end
local current_until = tonumber(redis.call('HGET', KEYS[1], 'until_ms') or '0')
local clock = redis.call('TIME')
local now_ms = (tonumber(clock[1]) * 1000) + math.floor(tonumber(clock[2]) / 1000)
if current == incoming and current_until >= incoming_until then
  if incoming_until > now_ms and #KEYS > 1 then
    redis.call('HSET', KEYS[1], 'observation_token', ARGV[4])
  end
  return 0
end
if incoming_until <= now_ms then
  redis.call('DEL', KEYS[1])
  if #KEYS > 1 then redis.call('ZREM', KEYS[2], ARGV[3]) end
  return 0
end
redis.call('HSET', KEYS[1], 'revision', ARGV[1], 'until_ms', ARGV[2])
if #KEYS > 1 then redis.call('HSET', KEYS[1], 'observation_token', ARGV[4]) end
local ttl = incoming_until - now_ms + 60000
redis.call('PEXPIRE', KEYS[1], ttl)
if #KEYS > 1 then redis.call('ZADD', KEYS[2], incoming_until, ARGV[3]) end
return 1
"#;

const READ_SCRIPT: &str = r#"
local revision = redis.call('HGET', KEYS[1], 'revision')
local until_ms = redis.call('HGET', KEYS[1], 'until_ms')
if revision == false or until_ms == false then
  redis.call('DEL', KEYS[1])
  if #KEYS > 1 then redis.call('ZREM', KEYS[2], ARGV[1]) end
  return {0, '0', '0', ''}
end
local clock = redis.call('TIME')
local now_ms = (tonumber(clock[1]) * 1000) + math.floor(tonumber(clock[2]) / 1000)
if tonumber(until_ms) <= now_ms then
  redis.call('DEL', KEYS[1])
  if #KEYS > 1 then redis.call('ZREM', KEYS[2], ARGV[1]) end
  return {0, '0', '0', ''}
end
return {1, revision, until_ms, redis.call('HGET', KEYS[1], 'observation_token') or ''}
"#;

const INVALIDATE_SCRIPT: &str = r#"
local current = tonumber(redis.call('HGET', KEYS[1], 'revision') or '0')
if current > tonumber(ARGV[1]) then return 0 end
redis.call('DEL', KEYS[1])
if #KEYS > 1 then redis.call('ZREM', KEYS[2], ARGV[2]) end
return 1
"#;

const RECOVER_OBSERVED_SCRIPT: &str = r#"
if redis.call('HGET', KEYS[1], 'revision') ~= ARGV[1]
   or redis.call('HGET', KEYS[1], 'until_ms') ~= ARGV[2]
   or ARGV[4] == ''
   or redis.call('HGET', KEYS[1], 'observation_token') ~= ARGV[4] then return 0 end
redis.call('DEL', KEYS[1])
redis.call('ZREM', KEYS[2], ARGV[3])
return 1
"#;

const ACTIVE_COOLDOWNS_SCRIPT: &str = r#"
local clock = redis.call('TIME')
local now_ms = (tonumber(clock[1]) * 1000) + math.floor(tonumber(clock[2]) / 1000)
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now_ms)
return redis.call('ZRANGEBYSCORE', KEYS[1], '(' .. now_ms, '+inf', 'WITHSCORES')
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialCooldown {
    pub provider_account_id: String,
    pub credential_revision: Revision,
    pub cooldown_until: DateTime<Utc>,
}

#[async_trait]
pub trait CredentialCooldownRepository: Send + Sync {
    async fn cache_credential_cooldown(&self, cooldown: &CredentialCooldown) -> StoreResult<bool>;
    async fn read_credential_cooldown(
        &self,
        provider_account_id: &str,
    ) -> StoreResult<Option<CredentialCooldown>>;
    async fn invalidate_credential_cooldown(
        &self,
        provider_account_id: &str,
        through_revision: Revision,
    ) -> StoreResult<bool>;
    /// 删除账号时清除该账号全部 account/model scope cooldown key。
    async fn delete_account_cooldowns(&self, provider_account_id: &str) -> StoreResult<bool>;
}

#[derive(Clone)]
pub struct RedisCredentialCooldownRepository {
    pub(super) connection: ConnectionManager,
    pub(super) namespace: String,
    pub(super) protection_enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

struct StoredCooldown {
    revision: Revision,
    until: DateTime<Utc>,
    observation_token: Option<String>,
}

impl RedisCredentialCooldownRepository {
    pub fn new(connection: ConnectionManager, key_namespace: &str) -> StoreResult<Self> {
        Ok(Self {
            connection,
            namespace: namespace(key_namespace)?,
            protection_enabled: Default::default(),
        })
    }

    fn key(&self, provider_account_id: &str) -> StoreResult<String> {
        let fingerprint = resource_fingerprint("credential cooldown", provider_account_id)?;
        Ok(format!("{}:account:{fingerprint}:cooldown", self.namespace))
    }

    fn active_index_key(&self) -> String {
        format!("{}:account:active-cooldowns", self.namespace)
    }

    fn scoped_key(
        &self,
        provider_account_id: &str,
        scope: &ProviderCooldownScope,
    ) -> StoreResult<String> {
        let account_fingerprint = resource_fingerprint("credential cooldown", provider_account_id)?;
        let scope_fingerprint = resource_fingerprint("credential cooldown scope", scope.value())?;
        Ok(format!(
            "{}:account:{account_fingerprint}:cooldown:{}:{scope_fingerprint}",
            self.namespace,
            scope.kind(),
        ))
    }

    async fn cache_at_key(
        &self,
        key: String,
        credential_revision: Revision,
        cooldown_until: DateTime<Utc>,
        index_member: Option<&str>,
    ) -> StoreResult<bool> {
        let until_ms = cooldown_until.timestamp_millis();
        if until_ms <= 0 {
            return Err(invalid("cooldown expiry must be positive"));
        }
        let mut connection = self.connection.clone();
        let written = if let Some(index_member) = index_member {
            Script::new(WRITE_SCRIPT)
                .key(key)
                .key(self.active_index_key())
                .arg(credential_revision.get())
                .arg(until_ms)
                .arg(index_member)
                .arg(Uuid::new_v4().to_string())
                .invoke_async::<i64>(&mut connection)
                .await
        } else {
            Script::new(WRITE_SCRIPT)
                .key(key)
                .arg(credential_revision.get())
                .arg(until_ms)
                .arg("")
                .invoke_async::<i64>(&mut connection)
                .await
        }
        .map_err(|error| redis_unavailable("cache credential cooldown").with_source(error))?;
        Ok(written == 1)
    }

    async fn read_at_key(
        &self,
        key: String,
        index_member: Option<&str>,
    ) -> StoreResult<Option<StoredCooldown>> {
        let mut connection = self.connection.clone();
        let result = if let Some(index_member) = index_member {
            Script::new(READ_SCRIPT)
                .key(key)
                .key(self.active_index_key())
                .arg(index_member)
                .invoke_async(&mut connection)
                .await
        } else {
            Script::new(READ_SCRIPT)
                .key(key)
                .arg("")
                .invoke_async(&mut connection)
                .await
        };
        parse_stored_cooldown(
            result.map_err(|error| {
                redis_unavailable("read credential cooldown").with_source(error)
            })?,
        )
    }

    /// 保留每条命令的错误，脚本失效时只补发尚未执行的 NOSCRIPT 项
    pub(super) async fn read_script_batch(
        &self,
        script: &Script,
        commands: Vec<StoreResult<redis::Cmd>>,
    ) -> Vec<StoreResult<redis::Value>> {
        let mut pipeline = redis::pipe();
        pipeline.ignore_errors();
        for command in commands.iter().filter_map(|command| command.as_ref().ok()) {
            pipeline.add_command(command.clone());
        }
        let mut connection = self.connection.clone();
        let values = pipeline
            .query_async::<Vec<redis::RedisResult<redis::Value>>>(&mut connection)
            .await;
        let values = match values {
            Ok(values) => {
                let mut retry = redis::pipe();
                retry.ignore_errors();
                let mut retry_indices = Vec::new();
                for (index, (command, value)) in pipeline.cmd_iter().zip(&values).enumerate() {
                    if value.as_ref().is_err_and(|error| {
                        error.kind() == redis::ErrorKind::Server(redis::ServerErrorKind::NoScript)
                    }) {
                        retry.add_command(command.clone());
                        retry_indices.push(index);
                    }
                }
                let mut values = values
                    .into_iter()
                    .map(|value| {
                        value.map_err(|source| {
                            redis_unavailable("read cooldown batch").with_source(source)
                        })
                    })
                    .collect::<Vec<_>>();
                if !retry_indices.is_empty() {
                    let retried = async {
                        script.load_async(&mut connection).await?;
                        retry
                            .query_async::<Vec<redis::RedisResult<redis::Value>>>(&mut connection)
                            .await
                    }
                    .await;
                    match retried {
                        Ok(retried) => {
                            for (index, value) in retry_indices.into_iter().zip(retried) {
                                values[index] = value.map_err(|source| {
                                    redis_unavailable("retry cooldown batch").with_source(source)
                                });
                            }
                        }
                        Err(source) => {
                            let error =
                                redis_unavailable("retry cooldown batch").with_source(source);
                            for index in retry_indices {
                                values[index] = Err(error.clone());
                            }
                        }
                    }
                }
                values
            }
            Err(source) => {
                vec![
                    Err(redis_unavailable("read cooldown batch").with_source(source));
                    pipeline.len()
                ]
            }
        };
        let mut values = values.into_iter();
        commands
            .into_iter()
            .map(|command| {
                command.and_then(|_| {
                    values
                        .next()
                        .unwrap_or_else(|| Err(invalid("missing cooldown batch response")))
                })
            })
            .collect()
    }

    async fn invalidate_at_key(
        &self,
        key: String,
        through_revision: Revision,
        index_member: Option<&str>,
    ) -> StoreResult<bool> {
        let mut connection = self.connection.clone();
        let removed = if let Some(index_member) = index_member {
            Script::new(INVALIDATE_SCRIPT)
                .key(key)
                .key(self.active_index_key())
                .arg(through_revision.get())
                .arg(index_member)
                .invoke_async::<i64>(&mut connection)
                .await
        } else {
            Script::new(INVALIDATE_SCRIPT)
                .key(key)
                .arg(through_revision.get())
                .arg("")
                .invoke_async::<i64>(&mut connection)
                .await
        }
        .map_err(|error| redis_unavailable("invalidate credential cooldown").with_source(error))?;
        Ok(removed == 1)
    }

    pub(crate) async fn active_cooldowns(&self) -> StoreResult<AccountRuntimeSnapshot> {
        let mut connection = self.connection.clone();
        let values = Script::new(ACTIVE_COOLDOWNS_SCRIPT)
            .key(self.active_index_key())
            .invoke_async::<Vec<String>>(&mut connection)
            .await
            .map_err(|error| {
                redis_unavailable("list active credential cooldowns").with_source(error)
            })?;
        if values.len() % 2 != 0 {
            return Err(invalid("active cooldown index is invalid"));
        }
        let mut rate_limited_until = std::collections::BTreeMap::new();
        for pair in values.as_chunks::<2>().0 {
            let until_ms = pair[1]
                .parse::<i64>()
                .map_err(|_| invalid("active cooldown expiry is invalid"))?;
            let until = DateTime::from_timestamp_millis(until_ms)
                .ok_or_else(|| invalid("active cooldown expiry is invalid"))?;
            rate_limited_until.insert(pair[0].clone(), until);
        }
        Ok(AccountRuntimeSnapshot {
            error_protections: Default::default(),
            rate_limited_until,
            in_flight: None,
        })
    }
}

#[async_trait]
impl CredentialCooldownRepository for RedisCredentialCooldownRepository {
    async fn cache_credential_cooldown(&self, cooldown: &CredentialCooldown) -> StoreResult<bool> {
        require_nonempty(
            "credential cooldown",
            "provider_account_id",
            &cooldown.provider_account_id,
        )?;
        self.cache_at_key(
            self.key(&cooldown.provider_account_id)?,
            cooldown.credential_revision,
            cooldown.cooldown_until,
            Some(&cooldown.provider_account_id),
        )
        .await
    }

    async fn read_credential_cooldown(
        &self,
        provider_account_id: &str,
    ) -> StoreResult<Option<CredentialCooldown>> {
        require_nonempty(
            "credential cooldown",
            "provider_account_id",
            provider_account_id,
        )?;
        self.read_at_key(self.key(provider_account_id)?, Some(provider_account_id))
            .await
            .map(|value| {
                value.map(|record| CredentialCooldown {
                    provider_account_id: provider_account_id.to_owned(),
                    credential_revision: record.revision,
                    cooldown_until: record.until,
                })
            })
    }

    async fn invalidate_credential_cooldown(
        &self,
        provider_account_id: &str,
        through_revision: Revision,
    ) -> StoreResult<bool> {
        require_nonempty(
            "credential cooldown",
            "provider_account_id",
            provider_account_id,
        )?;
        self.invalidate_at_key(
            self.key(provider_account_id)?,
            through_revision,
            Some(provider_account_id),
        )
        .await
    }

    async fn delete_account_cooldowns(&self, provider_account_id: &str) -> StoreResult<bool> {
        require_nonempty(
            "credential cooldown",
            "provider_account_id",
            provider_account_id,
        )?;
        // 账号删除：清除该账号的 account key 与全部 model-scoped key。
        // 用 SCAN 精确匹配命名空间内该账号前缀，避免 KEYS 阻塞。
        let mut connection = self.connection.clone();
        let account_key = self.key(provider_account_id)?;
        let mut keys = vec![
            account_key.clone(),
            self.protection_key(provider_account_id)?,
            self.protection_state_key(provider_account_id)?,
        ];
        let pattern = format!(
            "{}:account:{}:cooldown:*",
            self.namespace,
            resource_fingerprint("credential cooldown", provider_account_id)?
        );
        let mut cursor = 0_i64;
        loop {
            let (next, found): (i64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut connection)
                .await
                .map_err(|error| {
                    redis_unavailable("scan account cooldown keys").with_source(error)
                })?;
            keys.extend(found);
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
        if keys.is_empty() {
            return Ok(false);
        }
        let removed: i64 = redis::cmd("DEL")
            .arg(keys)
            .query_async(&mut connection)
            .await
            .map_err(|error| {
                redis_unavailable("delete account cooldown keys").with_source(error)
            })?;
        let index_removed: i64 = redis::cmd("ZREM")
            .arg(self.active_index_key())
            .arg(provider_account_id)
            .query_async(&mut connection)
            .await
            .map_err(|error| {
                redis_unavailable("remove active account cooldown").with_source(error)
            })?;
        let _: i64 = redis::cmd("ZREM")
            .arg(self.protection_index_key())
            .arg(provider_account_id)
            .query_async(&mut connection)
            .await
            .map_err(|error| redis_unavailable("remove protection index").with_source(error))?;
        Ok(removed > 0 || index_removed > 0)
    }
}

impl ProviderCooldownPort for RedisCredentialCooldownRepository {
    fn publish_protection_policy<'a>(
        &'a self,
        policy: &'a gateway_core::account::AccountErrorProtection,
        revision: gateway_core::routing::ConfigRevision,
    ) -> futures::future::BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            self.publish_policy(policy, revision)
                .await
                .map_err(|error| provider_unavailable("publish protection").with_source(error))
        })
    }
    fn begin_protection<'a>(
        &'a self,
        account: &'a ProviderAccountId,
        revision: CredentialRevision,
        fingerprint: &'a str,
        snapshot_revision: gateway_core::routing::ConfigRevision,
    ) -> futures::future::BoxFuture<
        'a,
        Result<Option<gateway_core::provider_ports::AccountProtectionGuard>, ProviderStoreError>,
    > {
        Box::pin(async move {
            self.begin_account_protection(account, revision, fingerprint, snapshot_revision)
                .await
                .map_err(|error| provider_unavailable("begin protection").with_source(error))
        })
    }
    fn advance_protection_revision<'a>(
        &'a self,
        account: &'a ProviderAccountId,
        revision: CredentialRevision,
    ) -> futures::future::BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            self.advance_account_protection_revision(account, revision)
                .await
                .map_err(|error| provider_unavailable("advance protection").with_source(error))
        })
    }
    fn record_protection_failure<'a>(
        &'a self,
        guard: &'a gateway_core::provider_ports::AccountProtectionGuard,
        policy: &'a gateway_core::account::AccountErrorProtection,
        matched: &'a [gateway_core::account::AccountProtectionMatch],
    ) -> futures::future::BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            self.protection_failure(guard, policy, matched)
                .await
                .map_err(|error| {
                    provider_unavailable("record protection failure").with_source(error)
                })
        })
    }
    fn record_protection_success<'a>(
        &'a self,
        guard: &'a gateway_core::provider_ports::AccountProtectionGuard,
    ) -> futures::future::BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            self.protection_success(guard).await.map_err(|error| {
                provider_unavailable("record protection success").with_source(error)
            })
        })
    }
    fn read_protection<'a>(
        &'a self,
        account: &'a ProviderAccountId,
        revision: CredentialRevision,
    ) -> futures::future::BoxFuture<
        'a,
        Result<Option<gateway_core::provider_ports::AccountProtectionState>, ProviderStoreError>,
    > {
        Box::pin(async move {
            self.read_account_protection(account.as_str(), Some(revision))
                .await
                .map_err(|error| provider_unavailable("read protection").with_source(error))
        })
    }

    fn read_many<'a>(
        &'a self,
        account_ids: &'a [ProviderAccountId],
    ) -> futures::future::BoxFuture<'a, Vec<Result<Option<ProviderCooldown>, ProviderStoreError>>>
    {
        Box::pin(async move {
            let script = Script::new(READ_SCRIPT);
            let mut results = Vec::with_capacity(account_ids.len());
            for batch in account_ids.chunks(128) {
                let commands = batch
                    .iter()
                    .map(|account| {
                        let mut command = redis::cmd("EVALSHA");
                        command
                            .arg(script.get_hash())
                            .arg(2)
                            .arg(self.key(account.as_str())?)
                            .arg(self.active_index_key())
                            .arg(account.as_str());
                        Ok(command)
                    })
                    .collect();
                let values = self.read_script_batch(&script, commands).await;
                results.extend(batch.iter().zip(values).map(|(account, value)| {
                    let value = value
                        .and_then(|value| {
                            redis::from_redis_value(value).map_err(|source| {
                                redis_unavailable("decode cooldown").with_source(source)
                            })
                        })
                        .and_then(parse_stored_cooldown)
                        .map_err(|source| {
                            provider_unavailable("read credential cooldown").with_source(source)
                        })?;
                    value
                        .map(|record| {
                            let revision = CredentialRevision::new(record.revision.get())
                                .map_err(|_| provider_invalid("decode credential cooldown"))?;
                            Ok(ProviderCooldown::new(
                                account.clone(),
                                revision,
                                record.until.into(),
                            )
                            .with_observation_token(record.observation_token))
                        })
                        .transpose()
                }));
            }
            results
        })
    }

    fn read_protection_many<'a>(
        &'a self,
        accounts: &'a [(ProviderAccountId, CredentialRevision)],
    ) -> futures::future::BoxFuture<
        'a,
        Vec<
            Result<
                Option<gateway_core::provider_ports::AccountProtectionState>,
                ProviderStoreError,
            >,
        >,
    > {
        Box::pin(async move {
            self.read_account_protections(accounts)
                .await
                .into_iter()
                .map(|result| {
                    result.map_err(|error| {
                        provider_unavailable("read protection batch").with_source(error)
                    })
                })
                .collect()
        })
    }

    fn clear_if_observed<'a>(
        &'a self,
        observed: &'a ProviderCooldown,
    ) -> futures::future::BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            let Some(observation_token) = observed.observation_token() else {
                return Ok(false);
            };
            let key = self
                .key(observed.account_id().as_str())
                .map_err(|_| provider_invalid("encode cooldown key"))?;
            let until: DateTime<Utc> = observed.until().into();
            let result: i64 = Script::new(RECOVER_OBSERVED_SCRIPT)
                .key(key)
                .key(self.active_index_key())
                .arg(observed.credential_revision().get())
                .arg(until.timestamp_millis())
                .arg(observed.account_id().as_str())
                .arg(observation_token)
                .invoke_async(&mut self.connection.clone())
                .await
                .map_err(|_| provider_unavailable("recover observed cooldown"))?;
            Ok(result == 1)
        })
    }

    fn put_if_later(
        &self,
        cooldown: ProviderCooldown,
    ) -> futures::future::BoxFuture<'_, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            let record = CredentialCooldown {
                provider_account_id: cooldown.account_id().as_str().to_owned(),
                credential_revision: Revision::new(cooldown.credential_revision().get())
                    .map_err(|_| provider_invalid("encode credential cooldown"))?,
                cooldown_until: cooldown.until().into(),
            };
            CredentialCooldownRepository::cache_credential_cooldown(self, &record)
                .await
                .map_err(|error| {
                    provider_unavailable("cache credential cooldown").with_source(error)
                })
        })
    }

    fn read<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
    ) -> futures::future::BoxFuture<'a, Result<Option<ProviderCooldown>, ProviderStoreError>> {
        Box::pin(async move {
            self.read_at_key(
                self.key(account_id.as_str())
                    .map_err(|_| provider_invalid("encode cooldown key"))?,
                Some(account_id.as_str()),
            )
            .await
            .map_err(|error| provider_unavailable("read credential cooldown").with_source(error))?
            .map(|record| {
                let revision = CredentialRevision::new(record.revision.get())
                    .map_err(|_| provider_invalid("decode credential cooldown"))?;
                Ok(
                    ProviderCooldown::new(account_id.clone(), revision, record.until.into())
                        .with_observation_token(record.observation_token),
                )
            })
            .transpose()
        })
    }

    fn clear<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
        through_revision: CredentialRevision,
    ) -> futures::future::BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            let revision = Revision::new(through_revision.get())
                .map_err(|_| provider_invalid("encode credential cooldown revision"))?;
            CredentialCooldownRepository::invalidate_credential_cooldown(
                self,
                account_id.as_str(),
                revision,
            )
            .await
            .map_err(|error| provider_unavailable("clear credential cooldown").with_source(error))
        })
    }

    fn put_scoped_if_later(
        &self,
        cooldown: ProviderScopedCooldown,
    ) -> futures::future::BoxFuture<'_, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            let revision = Revision::new(cooldown.credential_revision().get())
                .map_err(|_| provider_invalid("encode scoped credential cooldown"))?;
            self.cache_at_key(
                self.scoped_key(cooldown.account_id().as_str(), cooldown.scope())
                    .map_err(|_| provider_invalid("encode scoped credential cooldown"))?,
                revision,
                cooldown.until().into(),
                None,
            )
            .await
            .map_err(|error| {
                provider_unavailable("cache scoped credential cooldown").with_source(error)
            })
        })
    }

    fn read_scoped<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
        scope: &'a ProviderCooldownScope,
    ) -> futures::future::BoxFuture<'a, Result<Option<ProviderScopedCooldown>, ProviderStoreError>>
    {
        Box::pin(async move {
            self.read_at_key(
                self.scoped_key(account_id.as_str(), scope)
                    .map_err(|_| provider_invalid("encode scoped credential cooldown"))?,
                None,
            )
            .await
            .map_err(|error| {
                provider_unavailable("read scoped credential cooldown").with_source(error)
            })?
            .map(|record| {
                Ok(ProviderScopedCooldown::new(
                    account_id.clone(),
                    CredentialRevision::new(record.revision.get())
                        .map_err(|_| provider_invalid("decode scoped credential cooldown"))?,
                    scope.clone(),
                    record.until.into(),
                ))
            })
            .transpose()
        })
    }

    fn read_scoped_many<'a>(
        &'a self,
        account_ids: &'a [ProviderAccountId],
        scope: &'a ProviderCooldownScope,
    ) -> futures::future::BoxFuture<
        'a,
        Vec<Result<Option<ProviderScopedCooldown>, ProviderStoreError>>,
    > {
        Box::pin(async move {
            let script = Script::new(READ_SCRIPT);
            let mut results = Vec::with_capacity(account_ids.len());
            for batch in account_ids.chunks(128) {
                let commands = batch
                    .iter()
                    .map(|account| {
                        let mut command = redis::cmd("EVALSHA");
                        command
                            .arg(script.get_hash())
                            .arg(1)
                            .arg(self.scoped_key(account.as_str(), scope)?)
                            .arg("");
                        Ok(command)
                    })
                    .collect();
                let values = self.read_script_batch(&script, commands).await;
                results.extend(batch.iter().zip(values).map(|(account, value)| {
                    let value = value
                        .and_then(|value| {
                            redis::from_redis_value(value).map_err(|source| {
                                redis_unavailable("decode scoped cooldown").with_source(source)
                            })
                        })
                        .and_then(parse_stored_cooldown)
                        .map_err(|source| {
                            provider_unavailable("read scoped credential cooldown")
                                .with_source(source)
                        })?;
                    value
                        .map(|record| {
                            Ok(ProviderScopedCooldown::new(
                                account.clone(),
                                CredentialRevision::new(record.revision.get()).map_err(|_| {
                                    provider_invalid("decode scoped credential cooldown")
                                })?,
                                scope.clone(),
                                record.until.into(),
                            ))
                        })
                        .transpose()
                }));
            }
            results
        })
    }

    fn clear_scoped<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
        scope: &'a ProviderCooldownScope,
        through_revision: CredentialRevision,
    ) -> futures::future::BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            let revision = Revision::new(through_revision.get())
                .map_err(|_| provider_invalid("encode scoped cooldown revision"))?;
            self.invalidate_at_key(
                self.scoped_key(account_id.as_str(), scope)
                    .map_err(|_| provider_invalid("encode scoped credential cooldown"))?,
                revision,
                None,
            )
            .await
            .map_err(|error| {
                provider_unavailable("clear scoped credential cooldown").with_source(error)
            })
        })
    }

    fn clear_all<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
    ) -> futures::future::BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            self.delete_account_cooldowns(account_id.as_str())
                .await
                .map_err(|error| {
                    provider_unavailable("clear all credential cooldowns").with_source(error)
                })
        })
    }
}

pub(super) fn invalid(message: &str) -> StoreError {
    StoreError::InvalidData {
        entity: "credential cooldown",
        message: message.to_owned(),
    }
}

fn provider_unavailable(operation: &'static str) -> ProviderStoreError {
    ProviderStoreError::new(ProviderStoreErrorKind::Unavailable, operation)
}

fn provider_invalid(operation: &'static str) -> ProviderStoreError {
    ProviderStoreError::new(ProviderStoreErrorKind::InvalidData, operation)
}

fn parse_stored_cooldown(
    (present, revision, until_ms, observation_token): (i64, String, String, String),
) -> StoreResult<Option<StoredCooldown>> {
    if present == 0 {
        return Ok(None);
    }
    let revision = revision
        .parse::<u64>()
        .map_err(|_| invalid("cached cooldown revision is invalid"))?;
    let until_ms = until_ms
        .parse::<i64>()
        .map_err(|_| invalid("cached cooldown expiry is invalid"))?;
    let cooldown_until = DateTime::from_timestamp_millis(until_ms)
        .ok_or_else(|| invalid("cached cooldown expiry is invalid"))?;
    Ok(Some(StoredCooldown {
        revision: Revision::new(revision)?,
        until: cooldown_until,
        observation_token: (!observation_token.is_empty()).then_some(observation_token),
    }))
}
