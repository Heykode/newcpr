//! Provider 会话亲和性的可丢失 Redis 映射。

use std::time::Duration;

use gateway_core::account::ProviderAccountId;
use gateway_core::provider_ports::{
    BindingToken, ProviderSessionAffinityBinding, ProviderSessionAffinityKey,
    ProviderSessionAffinityPort, ProviderSessionAlias, ProviderStoreError, ProviderStoreErrorKind,
};
use gateway_core::routing::ProviderKind;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};

use crate::StoreResult;

use super::{namespace, resource_fingerprint};

const CLAIM_OR_LOAD_SCRIPT: &str = r#"
local current = redis.call('GET', KEYS[1])
if current then
  return current
end
redis.call('PSETEX', KEYS[1], tonumber(ARGV[2]), ARGV[1])
return ARGV[1]
"#;

const COMPARE_AND_BIND_SCRIPT: &str = r#"
local current = redis.call('GET', KEYS[1])
if not current then
  redis.call('PSETEX', KEYS[1], tonumber(ARGV[4]), ARGV[3])
  return ARGV[3]
end
local ok, decoded = pcall(cjson.decode, current)
if (ok and type(decoded) == 'table' and decoded['accountId'] == ARGV[1] and decoded['token'] == ARGV[2])
   or (not ok and current == ARGV[1] and ARGV[2] == 'legacy') then
  redis.call('PSETEX', KEYS[1], tonumber(ARGV[4]), ARGV[3])
  return ARGV[3]
end
return current
"#;

const RENEW_BINDING_SCRIPT: &str = r#"
local current = redis.call('GET', KEYS[1])
if not current then return 0 end
local ok, decoded = pcall(cjson.decode, current)
if (ok and type(decoded) == 'table' and decoded['accountId'] == ARGV[1] and decoded['token'] == ARGV[2])
   or (not ok and current == ARGV[1] and ARGV[2] == 'legacy') then
  return redis.call('PEXPIRE', KEYS[1], ARGV[3])
end
return 0
"#;

const BIND_ALIAS_SCRIPT: &str = r#"
local current = redis.call('GET', KEYS[1])
if not current then
  redis.call('PSETEX', KEYS[1], tonumber(ARGV[3]), ARGV[2])
  return 1
end
local ok, stored = pcall(cjson.decode, current)
local candidate_ok, candidate = pcall(cjson.decode, ARGV[2])
if ok and candidate_ok and type(stored) == 'table' and type(candidate) == 'table'
   and stored['sessionKey'] == candidate['sessionKey']
   and (stored['rootSessionKey'] or cjson.null) == (candidate['rootSessionKey'] or cjson.null) then
  redis.call('PEXPIRE', KEYS[1], tonumber(ARGV[3]))
  return 1
end
return 0
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredBinding {
    #[serde(rename = "accountId")]
    account_id: String,
    token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredAlias {
    #[serde(rename = "sessionKey")]
    session_key: String,
    #[serde(rename = "followOnly")]
    follow_only: bool,
    #[serde(
        default,
        rename = "rootSessionKey",
        skip_serializing_if = "Option::is_none"
    )]
    root_session_key: Option<String>,
}

#[derive(Clone)]
pub struct RedisProviderSessionAffinityRepository {
    connection: ConnectionManager,
    namespace: String,
}

impl RedisProviderSessionAffinityRepository {
    pub fn new(connection: ConnectionManager, key_namespace: &str) -> StoreResult<Self> {
        Ok(Self {
            connection,
            namespace: namespace(key_namespace)?,
        })
    }

    fn key(
        &self,
        provider_kind: &ProviderKind,
        affinity_key: &ProviderSessionAffinityKey,
    ) -> Result<String, ProviderStoreError> {
        let scope = format!(
            "{}\0{}",
            provider_kind.as_str(),
            affinity_key.expose_to_store()
        );
        let fingerprint = resource_fingerprint("provider session affinity", &scope)
            .map_err(|_| provider_invalid("encode provider session affinity key"))?;
        Ok(format!(
            "{}:scheduler:affinity:{{{fingerprint}}}",
            self.namespace
        ))
    }

    fn alias_key(
        &self,
        provider_kind: &ProviderKind,
        alias: &ProviderSessionAffinityKey,
    ) -> Result<String, ProviderStoreError> {
        let scope = format!("{}\0{}", provider_kind.as_str(), alias.expose_to_store());
        let fingerprint = resource_fingerprint("provider session alias", &scope)
            .map_err(|_| provider_invalid("encode provider session alias key"))?;
        Ok(format!(
            "{}:scheduler:session-alias:{{{fingerprint}}}",
            self.namespace
        ))
    }
}

impl ProviderSessionAffinityPort for RedisProviderSessionAffinityRepository {
    fn load<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
    ) -> futures::future::BoxFuture<'a, Result<Option<ProviderAccountId>, ProviderStoreError>> {
        Box::pin(async move {
            let mut connection = self.connection.clone();
            let value = redis::cmd("GET")
                .arg(self.key(provider_kind, key)?)
                .query_async::<Option<String>>(&mut connection)
                .await
                .map_err(|error| {
                    provider_unavailable("load provider session affinity").with_source(error)
                })?;
            value
                .map(decode_binding)
                .transpose()
                .map(|binding| binding.map(|binding| binding.account_id().clone()))
        })
    }

    fn bind<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        account_id: &'a ProviderAccountId,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            let ttl_millis = session_affinity_ttl_millis(ttl)?;
            let mut connection = self.connection.clone();
            let value =
                encode_binding(&ProviderSessionAffinityBinding::legacy(account_id.clone()))?;
            redis::cmd("PSETEX")
                .arg(self.key(provider_kind, key)?)
                .arg(ttl_millis)
                .arg(value)
                .query_async::<()>(&mut connection)
                .await
                .map_err(|error| {
                    provider_unavailable("bind provider session affinity").with_source(error)
                })
        })
    }

    fn claim_or_load<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        candidate_account_id: &'a ProviderAccountId,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<ProviderAccountId, ProviderStoreError>> {
        Box::pin(async move {
            let ttl_millis = session_affinity_ttl_millis(ttl)?;
            let mut connection = self.connection.clone();
            let candidate = ProviderSessionAffinityBinding::legacy(candidate_account_id.clone());
            let candidate_value = encode_binding(&candidate)?;
            let effective_binding = redis::Script::new(CLAIM_OR_LOAD_SCRIPT)
                .key(self.key(provider_kind, key)?)
                .arg(candidate_value)
                .arg(ttl_millis)
                .invoke_async::<String>(&mut connection)
                .await
                .map_err(|error| {
                    provider_unavailable("claim provider session affinity").with_source(error)
                })?;
            decode_binding(effective_binding).map(|binding| binding.account_id().clone())
        })
    }

    fn compare_and_bind<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        expected_account_id: &'a ProviderAccountId,
        replacement_account_id: &'a ProviderAccountId,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<ProviderAccountId, ProviderStoreError>> {
        Box::pin(async move {
            let ttl_millis = session_affinity_ttl_millis(ttl)?;
            let mut connection = self.connection.clone();
            let expected = ProviderSessionAffinityBinding::legacy(expected_account_id.clone());
            let replacement =
                ProviderSessionAffinityBinding::legacy(replacement_account_id.clone());
            let effective_binding = redis::Script::new(COMPARE_AND_BIND_SCRIPT)
                .key(self.key(provider_kind, key)?)
                .arg(expected_account_id.as_str())
                .arg(expected.token().expose_to_store())
                .arg(encode_binding(&replacement)?)
                .arg(ttl_millis)
                .invoke_async::<String>(&mut connection)
                .await
                .map_err(|error| {
                    provider_unavailable("compare provider session affinity").with_source(error)
                })?;
            decode_binding(effective_binding).map(|binding| binding.account_id().clone())
        })
    }

    fn load_binding<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
    ) -> futures::future::BoxFuture<
        'a,
        Result<Option<ProviderSessionAffinityBinding>, ProviderStoreError>,
    > {
        Box::pin(async move {
            let mut connection = self.connection.clone();
            let value = redis::cmd("GET")
                .arg(self.key(provider_kind, key)?)
                .query_async::<Option<String>>(&mut connection)
                .await
                .map_err(|error| {
                    provider_unavailable("load provider session affinity binding")
                        .with_source(error)
                })?;
            value.map(decode_binding).transpose()
        })
    }

    fn load_alias<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        alias: &'a ProviderSessionAffinityKey,
    ) -> futures::future::BoxFuture<'a, Result<Option<ProviderSessionAlias>, ProviderStoreError>>
    {
        Box::pin(async move {
            let mut connection = self.connection.clone();
            let value = redis::cmd("GET")
                .arg(self.alias_key(provider_kind, alias)?)
                .query_async::<Option<String>>(&mut connection)
                .await
                .map_err(|error| {
                    provider_unavailable("load provider session alias").with_source(error)
                })?;
            value.map(decode_alias).transpose()
        })
    }

    fn bind_alias<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        alias: &'a ProviderSessionAffinityKey,
        session: &'a ProviderSessionAlias,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            let ttl_millis = session_affinity_ttl_millis(ttl)?;
            let value = encode_alias(session)?;
            let mut connection = self.connection.clone();
            redis::Script::new(BIND_ALIAS_SCRIPT)
                .key(self.alias_key(provider_kind, alias)?)
                .arg(value.clone())
                .arg(value)
                .arg(ttl_millis)
                .invoke_async::<i32>(&mut connection)
                .await
                .map(|result| result == 1)
                .map_err(|error| {
                    provider_unavailable("bind provider session alias").with_source(error)
                })
        })
    }

    fn bind_binding<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        binding: &'a ProviderSessionAffinityBinding,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            let ttl_millis = session_affinity_ttl_millis(ttl)?;
            let value = encode_binding(binding)?;
            let mut connection = self.connection.clone();
            redis::cmd("PSETEX")
                .arg(self.key(provider_kind, key)?)
                .arg(ttl_millis)
                .arg(value)
                .query_async::<()>(&mut connection)
                .await
                .map_err(|error| {
                    provider_unavailable("bind provider session affinity binding")
                        .with_source(error)
                })
        })
    }

    fn claim_or_load_binding<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        candidate: &'a ProviderSessionAffinityBinding,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<ProviderSessionAffinityBinding, ProviderStoreError>>
    {
        Box::pin(async move {
            let ttl_millis = session_affinity_ttl_millis(ttl)?;
            let candidate_value = encode_binding(candidate)?;
            let mut connection = self.connection.clone();
            let effective = redis::Script::new(CLAIM_OR_LOAD_SCRIPT)
                .key(self.key(provider_kind, key)?)
                .arg(candidate_value)
                .arg(ttl_millis)
                .invoke_async::<String>(&mut connection)
                .await
                .map_err(|error| {
                    provider_unavailable("claim provider session affinity binding")
                        .with_source(error)
                })?;
            decode_binding(effective)
        })
    }

    fn renew_binding<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        expected: &'a ProviderSessionAffinityBinding,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            redis::Script::new(RENEW_BINDING_SCRIPT)
                .key(self.key(provider_kind, key)?)
                .arg(expected.account_id().as_str())
                .arg(expected.token().expose_to_store())
                .arg(session_affinity_ttl_millis(ttl)?)
                .invoke_async::<bool>(&mut self.connection.clone())
                .await
                .map_err(|error| {
                    provider_unavailable("renew provider session affinity binding")
                        .with_source(error)
                })
        })
    }

    fn compare_and_bind_binding<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        expected: &'a ProviderSessionAffinityBinding,
        replacement: &'a ProviderSessionAffinityBinding,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<ProviderSessionAffinityBinding, ProviderStoreError>>
    {
        Box::pin(async move {
            let ttl_millis = session_affinity_ttl_millis(ttl)?;
            let mut connection = self.connection.clone();
            let effective = redis::Script::new(COMPARE_AND_BIND_SCRIPT)
                .key(self.key(provider_kind, key)?)
                .arg(expected.account_id().as_str())
                .arg(expected.token().expose_to_store())
                .arg(encode_binding(replacement)?)
                .arg(ttl_millis)
                .invoke_async::<String>(&mut connection)
                .await
                .map_err(|error| {
                    provider_unavailable("compare provider session affinity binding")
                        .with_source(error)
                })?;
            decode_binding(effective)
        })
    }

    fn clear<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
    ) -> futures::future::BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            let mut connection = self.connection.clone();
            redis::cmd("DEL")
                .arg(self.key(provider_kind, key)?)
                .query_async::<u64>(&mut connection)
                .await
                .map(|removed| removed > 0)
                .map_err(|error| {
                    provider_unavailable("clear provider session affinity").with_source(error)
                })
        })
    }
}

fn session_affinity_ttl_millis(ttl: Duration) -> Result<i64, ProviderStoreError> {
    let millis = i64::try_from(ttl.as_millis())
        .map_err(|_| provider_invalid("validate provider session affinity TTL"))?;
    if millis == 0 {
        return Err(provider_invalid("validate provider session affinity TTL"));
    }
    Ok(millis)
}

fn encode_binding(binding: &ProviderSessionAffinityBinding) -> Result<String, ProviderStoreError> {
    serde_json::to_string(&StoredBinding {
        account_id: binding.account_id().as_str().to_owned(),
        token: binding.token().expose_to_store().to_owned(),
    })
    .map_err(|_| provider_invalid("encode provider session affinity binding"))
}

fn decode_binding(value: String) -> Result<ProviderSessionAffinityBinding, ProviderStoreError> {
    if let Ok(stored) = serde_json::from_str::<StoredBinding>(&value) {
        let account_id = ProviderAccountId::new(stored.account_id)
            .map_err(|_| provider_invalid("decode provider session affinity account"))?;
        let token = BindingToken::new(stored.token)?;
        return Ok(ProviderSessionAffinityBinding::new(account_id, token));
    }
    ProviderAccountId::new(value)
        .map(ProviderSessionAffinityBinding::legacy)
        .map_err(|_| provider_invalid("decode provider session affinity"))
}

fn encode_alias(alias: &ProviderSessionAlias) -> Result<String, ProviderStoreError> {
    serde_json::to_string(&StoredAlias {
        session_key: alias.session_key().expose_to_store().to_owned(),
        follow_only: alias.follow_only(),
        root_session_key: alias
            .root_session_key()
            .map(|key| key.expose_to_store().to_owned()),
    })
    .map_err(|_| provider_invalid("encode provider session alias"))
}

fn decode_alias(value: String) -> Result<ProviderSessionAlias, ProviderStoreError> {
    let stored = serde_json::from_str::<StoredAlias>(&value)
        .map_err(|_| provider_invalid("decode provider session alias"))?;
    let session_key = ProviderSessionAffinityKey::try_new(stored.session_key)?;
    Ok(
        ProviderSessionAlias::new(session_key, stored.follow_only).with_root_session_key(
            stored
                .root_session_key
                .map(ProviderSessionAffinityKey::try_new)
                .transpose()?,
        ),
    )
}

fn provider_unavailable(operation: &'static str) -> ProviderStoreError {
    ProviderStoreError::new(ProviderStoreErrorKind::Unavailable, operation)
}

fn provider_invalid(operation: &'static str) -> ProviderStoreError {
    ProviderStoreError::new(ProviderStoreErrorKind::InvalidData, operation)
}
