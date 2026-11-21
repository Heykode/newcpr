//! 账号保护的原子反馈、策略栅栏与独立冻结状态

use super::{RedisCredentialCooldownRepository, resource_fingerprint};
use crate::{Revision, StoreResult, redis_unavailable};
use chrono::{DateTime, Utc};

use gateway_core::{
    account::{
        AccountErrorProtection, AccountProtectionMatch, CredentialRevision, ProviderAccountId,
    },
    provider_ports::{AccountProtectionGuard, AccountProtectionState},
    routing::ConfigRevision,
};
use redis::Script;
use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::sync::atomic::Ordering;

const PUBLISH: &str = r#"
local old = redis.call('HGET', KEYS[1], 'fingerprint')
local revision = tonumber(redis.call('HGET', KEYS[1], 'revision') or '0')
if revision > tonumber(ARGV[2]) then return math.max(tonumber(redis.call('HGET', KEYS[1], 'enabled') or '0'), redis.call('ZCARD', KEYS[2]), redis.call('ZCARD', KEYS[3])) end
if old ~= ARGV[1] then
  redis.call('HSET', KEYS[1], 'fingerprint', ARGV[1], 'generation', ARGV[3])
end
redis.call('HSET', KEYS[1], 'revision', ARGV[2], 'enabled', ARGV[4], 'probe_enabled', ARGV[5])
return math.max(tonumber(ARGV[4]), redis.call('ZCARD', KEYS[2]), redis.call('ZCARD', KEYS[3]))
"#;

// 原 key 的代次和恢复时间原样搬运，不重新开始冷却
pub(super) const MIGRATE: &str = r#"
local function migrate_protection(legacy, protection, legacy_index, protection_index, account)
  local kind = redis.call('HGET', legacy, 'kind')
  if kind == 'capacity_freeze' or kind == 'capacity_freeze_probe' then
    if redis.call('EXISTS', protection) == 0 then redis.call('RENAME', legacy, protection) else redis.call('DEL', legacy) end
    redis.call('ZREM', legacy_index, account)
    redis.call('ZADD', protection_index, redis.call('HGET', protection, 'until_ms'), account)
  end
end
"#;

static BEGIN: LazyLock<Script> = LazyLock::new(|| {
    Script::new(&[MIGRATE, r#"
migrate_protection(KEYS[5], KEYS[3], KEYS[6], KEYS[4], ARGV[5])
if redis.call('HGET', KEYS[1], 'enabled') ~= '1'
  or redis.call('HGET', KEYS[1], 'fingerprint') ~= ARGV[2]
  or tonumber(redis.call('HGET', KEYS[1], 'revision') or '0') < tonumber(ARGV[3]) then return {} end
local clock = redis.call('TIME')
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local frozen_revision = tonumber(redis.call('HGET', KEYS[3], 'revision') or '0')
if frozen_revision > 0 then
  local until_ms = tonumber(redis.call('HGET', KEYS[3], 'until_ms') or '0')
  local kind = redis.call('HGET', KEYS[3], 'kind')
  if frozen_revision >= tonumber(ARGV[1]) and (until_ms > now or kind == 'error_protection_probe' or kind == 'capacity_freeze_probe') then return {} end
  redis.call('DEL', KEYS[3])
  redis.call('ZREM', KEYS[4], ARGV[5])
end
local current = tonumber(redis.call('HGET', KEYS[2], 'revision') or '0')
if current > tonumber(ARGV[1]) then return {} end
local generation = redis.call('HGET', KEYS[1], 'generation')
if current ~= tonumber(ARGV[1]) or redis.call('HGET', KEYS[2], 'policy') ~= generation then
  redis.call('DEL', KEYS[2])
  redis.call('HSET', KEYS[2], 'revision', ARGV[1], 'policy', generation, 'reset', ARGV[4], 'version', 0)
end
redis.call('EXPIRE', KEYS[2], 691200)
return {generation, redis.call('HGET', KEYS[2], 'reset'), redis.call('HGET', KEYS[2], 'version')}
"#].concat())
});

// 反馈不创建丢失的 guard 状态；Redis 丢失状态或手动恢复后不能重放历史失败
const FAILURE: &str = r#"
if redis.call('HGET', KEYS[1], 'generation') ~= ARGV[2]
  or redis.call('HGET', KEYS[1], 'enabled') ~= '1'
  or redis.call('HGET', KEYS[2], 'revision') ~= ARGV[1]
  or redis.call('HGET', KEYS[2], 'policy') ~= ARGV[2]
  or redis.call('HGET', KEYS[2], 'reset') ~= ARGV[3]
  or redis.call('EXISTS', KEYS[3]) == 1 then return 0 end
local clock = redis.call('TIME')
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local count = tonumber(redis.call('HGET', KEYS[2], 'count') or '0')
if tonumber(redis.call('HGET', KEYS[2], 'count_until') or '0') <= now then count = 0 end
count = count + 1
redis.call('HSET', KEYS[2], 'count', count, 'count_until', now + tonumber(ARGV[4]) * 1000)
redis.call('HINCRBY', KEYS[2], 'version', 1)
if count < tonumber(ARGV[5]) then return 0 end
local until_ms = now + tonumber(ARGV[6]) * 1000
redis.call('HSET', KEYS[3], 'revision', ARGV[1], 'until_ms', until_ms, 'kind', ARGV[7], 'generation', ARGV[8], 'duration', ARGV[6], 'model', ARGV[9], 'rules', ARGV[10])
redis.call('HSET', KEYS[2], 'reset', ARGV[8], 'count', 0)
redis.call('ZADD', KEYS[4], until_ms, ARGV[11])
if ARGV[7] ~= 'error_protection_probe' then redis.call('PEXPIRE', KEYS[3], tonumber(ARGV[6]) * 1000 + 60000) end
return 1
"#;

const SUCCESS: &str = r#"
if redis.call('HGET', KEYS[1], 'generation') ~= ARGV[2]
  or redis.call('HGET', KEYS[2], 'revision') ~= ARGV[1]
  or redis.call('HGET', KEYS[2], 'policy') ~= ARGV[2]
  or redis.call('HGET', KEYS[2], 'reset') ~= ARGV[3]
  or redis.call('HGET', KEYS[2], 'version') ~= ARGV[4]
  or redis.call('EXISTS', KEYS[3]) == 1 then return 0 end
redis.call('HDEL', KEYS[2], 'count', 'count_until')
redis.call('HINCRBY', KEYS[2], 'version', 1)
return 1
"#;

static READ: LazyLock<Script> = LazyLock::new(|| {
    Script::new(&[MIGRATE, r#"
migrate_protection(KEYS[4], KEYS[2], KEYS[5], KEYS[3], ARGV[1])
local values = redis.call('HGETALL', KEYS[2])
local revision = tonumber(redis.call('HGET', KEYS[2], 'revision') or '0')
local expected = tonumber(ARGV[2])
if expected > 0 and revision ~= expected then
  if revision < expected then redis.call('DEL', KEYS[2]); redis.call('ZREM', KEYS[3], ARGV[1]) end
  return {}
end
local clock = redis.call('TIME')
local now = tonumber(clock[1])*1000 + math.floor(tonumber(clock[2])/1000)
local until_ms = tonumber(redis.call('HGET', KEYS[2], 'until_ms') or '0')
if until_ms <= now and (redis.call('HGET', KEYS[2], 'kind') ~= 'error_protection_probe' and redis.call('HGET', KEYS[2], 'kind') ~= 'capacity_freeze_probe'
  or redis.call('HGET', KEYS[1], 'enabled') ~= '1'
  or redis.call('HGET', KEYS[1], 'probe_enabled') ~= '1') then
  redis.call('DEL', KEYS[2]); redis.call('ZREM', KEYS[3], ARGV[1]); return {}
end
return values
"#].concat())
});

const CLAIM_DUE: &str = r#"
local clock = redis.call('TIME')
local now = tonumber(clock[1])*1000 + math.floor(tonumber(clock[2])/1000)
local accounts = redis.call('ZRANGEBYSCORE', KEYS[2], '-inf', now, 'LIMIT', 0, 1)
for _,account in ipairs(accounts) do
  redis.call('ZADD', KEYS[2], now + 60000, account)
end
return accounts
"#;

// 恢复与凭据换代共用同一组 key，在修改冻结前一并接管旧记录
fn protection_mutation(script: &str) -> Script {
    Script::new(
        &[
            MIGRATE,
            "migrate_protection(KEYS[4], KEYS[1], KEYS[5], KEYS[3], ARGV[3])\n",
            script,
        ]
        .concat(),
    )
}

static CLEAR: LazyLock<Script> = LazyLock::new(|| {
    protection_mutation(
        r#"
local current = math.max(tonumber(redis.call('HGET', KEYS[1], 'revision') or '0'), tonumber(redis.call('HGET', KEYS[2], 'revision') or '0'))
if current > tonumber(ARGV[1]) then return 0 end
local removed = redis.call('DEL', KEYS[1])
redis.call('HSET', KEYS[2], 'revision', ARGV[1], 'reset', ARGV[2], 'version', 0)
redis.call('HDEL', KEYS[2], 'count', 'count_until')
redis.call('EXPIRE', KEYS[2], 691200)
redis.call('ZREM', KEYS[3], ARGV[3])
return removed
"#,
    )
});

static ADVANCE_REVISION: LazyLock<Script> = LazyLock::new(|| {
    protection_mutation(
        r#"
local current = tonumber(redis.call('HGET', KEYS[2], 'revision') or '0')
local frozen = tonumber(redis.call('HGET', KEYS[1], 'revision') or '0')
if current >= tonumber(ARGV[1]) or frozen >= tonumber(ARGV[1]) then return 0 end
redis.call('DEL', KEYS[1]); redis.call('ZREM', KEYS[3], ARGV[3])
redis.call('HSET', KEYS[2], 'revision', ARGV[1], 'reset', ARGV[2], 'version', 0)
redis.call('HDEL', KEYS[2], 'count', 'count_until')
redis.call('EXPIRE', KEYS[2], 691200)
return 1
"#,
    )
});

const FINISH: &str = r#"
if redis.call('HGET', KEYS[1], 'revision') ~= ARGV[1] or redis.call('HGET', KEYS[1], 'generation') ~= ARGV[2] then return 0 end
if ARGV[3] == '' then
  redis.call('DEL', KEYS[1]); redis.call('ZREM', KEYS[3], ARGV[5])
  redis.call('HSET', KEYS[2], 'reset', ARGV[4], 'version', 0)
  redis.call('HDEL', KEYS[2], 'count', 'count_until')
  redis.call('EXPIRE', KEYS[2], 691200)
else
  local until_ms = math.max(tonumber(redis.call('HGET', KEYS[1], 'until_ms')),tonumber(ARGV[3]))
  redis.call('HSET', KEYS[1], 'until_ms', until_ms, 'generation', ARGV[4])
  redis.call('ZADD', KEYS[3], until_ms, ARGV[5])
end
return 1
"#;

impl RedisCredentialCooldownRepository {
    fn policy_key(&self) -> String {
        format!("{}:account:error-protection-policy", self.namespace)
    }
    pub(super) fn protection_key(&self, account: &str) -> StoreResult<String> {
        Ok(format!(
            "{}:account:{}:error-protection",
            self.namespace,
            resource_fingerprint("credential cooldown", account)?
        ))
    }
    pub(super) fn protection_state_key(&self, account: &str) -> StoreResult<String> {
        Ok(format!(
            "{}:account:{}:error-protection-state",
            self.namespace,
            resource_fingerprint("credential cooldown", account)?
        ))
    }
    pub(super) fn protection_index_key(&self) -> String {
        format!("{}:account:active-protections", self.namespace)
    }

    pub(super) async fn publish_policy(
        &self,
        policy: &AccountErrorProtection,
        revision: ConfigRevision,
    ) -> StoreResult<()> {
        // Publish is the startup/configuration boundary. Move legacy indexed freezes before
        // an expiry-only cooldown scan could remove their probe index entries.
        use redis::AsyncCommands as _;
        let legacy: Vec<String> = self
            .connection
            .clone()
            .zrange(self.active_index_key(), 0, -1)
            .await
            .map_err(|error| redis_unavailable("read legacy protections").with_source(error))?;
        let migrate = Script::new(
            &[
                MIGRATE,
                "migrate_protection(KEYS[1], KEYS[2], KEYS[3], KEYS[4], ARGV[1])\nreturn 1",
            ]
            .concat(),
        );
        for account in legacy {
            migrate
                .key(self.key(&account)?)
                .key(self.protection_key(&account)?)
                .key(self.active_index_key())
                .key(self.protection_index_key())
                .arg(&account)
                .invoke_async::<i64>(&mut self.connection.clone())
                .await
                .map_err(|error| {
                    redis_unavailable("migrate legacy protection").with_source(error)
                })?;
        }
        let enabled = Script::new(PUBLISH)
            .key(self.policy_key())
            .key(self.protection_index_key())
            .key(self.active_index_key())
            .arg(policy.fingerprint())
            .arg(revision.get())
            .arg(uuid::Uuid::new_v4().to_string())
            .arg(u8::from(policy.enabled))
            .arg(u8::from(policy.probe_enabled))
            .invoke_async::<i64>(&mut self.connection.clone())
            .await
            .map_err(|e| redis_unavailable("publish account protection").with_source(e))?;
        self.protection_enabled
            .store(enabled > 0, Ordering::Release);
        Ok(())
    }
    pub(super) async fn begin_account_protection(
        &self,
        account: &ProviderAccountId,
        revision: CredentialRevision,
        fingerprint: &str,
        snapshot_revision: ConfigRevision,
    ) -> StoreResult<Option<AccountProtectionGuard>> {
        let result: Vec<String> = BEGIN
            .key(self.policy_key())
            .key(self.protection_state_key(account.as_str())?)
            .key(self.protection_key(account.as_str())?)
            .key(self.protection_index_key())
            .key(self.key(account.as_str())?)
            .key(self.active_index_key())
            .arg(revision.get())
            .arg(fingerprint)
            .arg(snapshot_revision.get())
            .arg(uuid::Uuid::new_v4().to_string())
            .arg(account.as_str())
            .invoke_async(&mut self.connection.clone())
            .await
            .map_err(|e| redis_unavailable("begin account protection").with_source(e))?;
        let [generation, reset, version] = result.as_slice() else {
            return Ok(None);
        };
        Ok(Some(AccountProtectionGuard {
            account_id: account.clone(),
            credential_revision: revision,
            policy_generation: generation.clone(),
            reset_generation: reset.clone(),
            evidence_version: version
                .parse()
                .map_err(|_| super::credential_cooldown::invalid("protection version"))?,
        }))
    }
    pub(super) async fn protection_failure(
        &self,
        guard: &AccountProtectionGuard,
        policy: &AccountErrorProtection,
        matched: &[AccountProtectionMatch],
    ) -> StoreResult<bool> {
        if !policy.enabled || matched.is_empty() {
            return Ok(false);
        }
        let rules = serde_json::to_string(matched)
            .map_err(|_| super::credential_cooldown::invalid("protection rules"))?;
        let changed: i64 = Script::new(FAILURE)
            .key(self.policy_key())
            .key(self.protection_state_key(guard.account_id.as_str())?)
            .key(self.protection_key(guard.account_id.as_str())?)
            .key(self.protection_index_key())
            .arg(guard.credential_revision.get())
            .arg(&guard.policy_generation)
            .arg(&guard.reset_generation)
            .arg(policy.window_seconds)
            .arg(policy.threshold)
            .arg(policy.duration_seconds)
            .arg(if policy.probe_enabled {
                "error_protection_probe"
            } else {
                "error_protection"
            })
            .arg(uuid::Uuid::new_v4().to_string())
            .arg(policy.probe_model.as_deref().unwrap_or_default())
            .arg(rules)
            .arg(guard.account_id.as_str())
            .invoke_async(&mut self.connection.clone())
            .await
            .map_err(|e| redis_unavailable("record protection failure").with_source(e))?;
        Ok(changed == 1)
    }
    pub(super) async fn protection_success(
        &self,
        guard: &AccountProtectionGuard,
    ) -> StoreResult<bool> {
        let changed: i64 = Script::new(SUCCESS)
            .key(self.policy_key())
            .key(self.protection_state_key(guard.account_id.as_str())?)
            .key(self.protection_key(guard.account_id.as_str())?)
            .arg(guard.credential_revision.get())
            .arg(&guard.policy_generation)
            .arg(&guard.reset_generation)
            .arg(guard.evidence_version)
            .invoke_async(&mut self.connection.clone())
            .await
            .map_err(|e| redis_unavailable("record protection success").with_source(e))?;
        Ok(changed == 1)
    }
    pub(crate) async fn read_account_protection(
        &self,
        account: &str,
        revision: Option<CredentialRevision>,
    ) -> StoreResult<Option<AccountProtectionState>> {
        if !self.protection_enabled.load(Ordering::Acquire) {
            return Ok(None);
        }
        let values: BTreeMap<String, String> = READ
            .clone()
            .key(self.policy_key())
            .key(self.protection_key(account)?)
            .key(self.protection_index_key())
            .key(self.key(account)?)
            .key(self.active_index_key())
            .arg(account)
            .arg(revision.map_or(0, CredentialRevision::get))
            .invoke_async(&mut self.connection.clone())
            .await
            .map_err(|error| redis_unavailable("read account protection").with_source(error))?;
        parse_protection(account, values)
    }

    pub(super) async fn read_account_protections(
        &self,
        accounts: &[(ProviderAccountId, CredentialRevision)],
    ) -> Vec<StoreResult<Option<AccountProtectionState>>> {
        if !self.protection_enabled.load(Ordering::Acquire) {
            return vec![Ok(None); accounts.len()];
        }
        let script = READ.clone();
        let mut results = Vec::with_capacity(accounts.len());
        for batch in accounts.chunks(128) {
            let commands = batch
                .iter()
                .map(|(account, revision)| {
                    let mut command = redis::cmd("EVALSHA");
                    command
                        .arg(script.get_hash())
                        .arg(5)
                        .arg(self.policy_key())
                        .arg(self.protection_key(account.as_str())?)
                        .arg(self.protection_index_key())
                        .arg(self.key(account.as_str())?)
                        .arg(self.active_index_key())
                        .arg(account.as_str())
                        .arg(revision.get());
                    Ok(command)
                })
                .collect();
            let values = self.read_script_batch(&script, commands).await;
            results.extend(batch.iter().zip(values).map(|((account, _), value)| {
                let values = value.and_then(|value| {
                    redis::from_redis_value(value).map_err(|source| {
                        redis_unavailable("decode protection batch").with_source(source)
                    })
                })?;
                parse_protection(account.as_str(), values)
            }));
        }
        results
    }

    pub(crate) async fn active_protections(
        &self,
    ) -> StoreResult<BTreeMap<String, AccountProtectionState>> {
        use redis::AsyncCommands as _;
        if !self.protection_enabled.load(Ordering::Acquire) {
            return Ok(BTreeMap::new());
        }
        let accounts: Vec<String> = self
            .connection
            .clone()
            .zrange(self.protection_index_key(), 0, -1)
            .await
            .map_err(|e| redis_unavailable("active protections").with_source(e))?;
        let mut result = BTreeMap::new();
        for chunk in accounts.chunks(super::ACCOUNT_STATE_READ_CONCURRENCY) {
            for read in futures::future::join_all(
                chunk
                    .iter()
                    .map(|id| self.read_account_protection(id, None)),
            )
            .await
            {
                if let Some(state) = read? {
                    result.insert(state.account_id.to_string(), state);
                }
            }
        }
        Ok(result)
    }

    pub(crate) async fn due_account_protections(&self) -> StoreResult<Vec<AccountProtectionState>> {
        if !self.protection_enabled.load(Ordering::Acquire) {
            return Ok(Vec::new());
        }
        // Expired/deleted index entries must not delay a live recovery behind them.
        for _ in 0..32 {
            let accounts: Vec<String> = Script::new(CLAIM_DUE)
                .key(self.policy_key())
                .key(self.protection_index_key())
                .invoke_async(&mut self.connection.clone())
                .await
                .map_err(|error| {
                    redis_unavailable("claim account protections").with_source(error)
                })?;
            if accounts.is_empty() {
                break;
            }
            for account in accounts {
                if let Some(state) = self.read_account_protection(&account, None).await?
                    && state.requires_probe
                    && state.until <= std::time::SystemTime::now()
                {
                    return Ok(vec![state]);
                }
            }
        }
        Ok(Vec::new())
    }

    pub(crate) async fn clear_protection(
        &self,
        account: &str,
        revision: Revision,
    ) -> StoreResult<bool> {
        let changed: i64 = CLEAR
            .key(self.protection_key(account)?)
            .key(self.protection_state_key(account)?)
            .key(self.protection_index_key())
            .key(self.key(account)?)
            .key(self.active_index_key())
            .arg(revision.get())
            .arg(uuid::Uuid::new_v4().to_string())
            .arg(account)
            .invoke_async(&mut self.connection.clone())
            .await
            .map_err(|e| redis_unavailable("clear account protection").with_source(e))?;
        Ok(changed == 1)
    }
    pub(super) async fn advance_account_protection_revision(
        &self,
        account: &ProviderAccountId,
        revision: CredentialRevision,
    ) -> StoreResult<bool> {
        if !self.protection_enabled.load(Ordering::Acquire) {
            return Ok(false);
        }
        let changed: i64 = ADVANCE_REVISION
            .key(self.protection_key(account.as_str())?)
            .key(self.protection_state_key(account.as_str())?)
            .key(self.protection_index_key())
            .key(self.key(account.as_str())?)
            .key(self.active_index_key())
            .arg(revision.get())
            .arg(uuid::Uuid::new_v4().to_string())
            .arg(account.as_str())
            .invoke_async(&mut self.connection.clone())
            .await
            .map_err(|e| {
                redis_unavailable("advance protection credential revision").with_source(e)
            })?;
        Ok(changed == 1)
    }

    pub(crate) async fn finish_protection(
        &self,
        account: &str,
        expected: &AccountProtectionState,
        postpone: Option<DateTime<Utc>>,
    ) -> StoreResult<bool> {
        let changed: i64 = Script::new(FINISH)
            .key(self.protection_key(account)?)
            .key(self.protection_state_key(account)?)
            .key(self.protection_index_key())
            .arg(expected.credential_revision.get())
            .arg(&expected.generation)
            .arg(
                postpone
                    .map(|v| v.timestamp_millis().to_string())
                    .unwrap_or_default(),
            )
            .arg(uuid::Uuid::new_v4().to_string())
            .arg(account)
            .invoke_async(&mut self.connection.clone())
            .await
            .map_err(|e| redis_unavailable("finish account protection").with_source(e))?;
        Ok(changed == 1)
    }
}

fn parse_protection(
    account: &str,
    values: BTreeMap<String, String>,
) -> StoreResult<Option<AccountProtectionState>> {
    if values.is_empty() {
        return Ok(None);
    }
    let invalid = || super::credential_cooldown::invalid("invalid protection snapshot");
    let revision = values
        .get("revision")
        .ok_or_else(invalid)?
        .parse::<u64>()
        .map_err(|_| invalid())?;
    let until_ms = values
        .get("until_ms")
        .ok_or_else(invalid)?
        .parse::<i64>()
        .map_err(|_| invalid())?;
    Ok(Some(AccountProtectionState {
        account_id: ProviderAccountId::new(account.to_owned()).map_err(|_| invalid())?,
        credential_revision: CredentialRevision::new(revision).map_err(|_| invalid())?,
        until: DateTime::from_timestamp_millis(until_ms)
            .ok_or_else(invalid)?
            .into(),
        generation: values.get("generation").cloned().ok_or_else(invalid)?,
        requires_probe: values.get("kind").is_some_and(|kind| {
            kind == "error_protection_probe" || kind == "capacity_freeze_probe"
        }),
        duration_seconds: values
            .get("duration")
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?,
        probe_model: values
            .get("model")
            .filter(|value| !value.is_empty())
            .cloned(),
        rules: values
            .get("rules")
            .map(|value| serde_json::from_str(value))
            .transpose()
            .map_err(|_| invalid())?
            .unwrap_or_default(),
    }))
}
