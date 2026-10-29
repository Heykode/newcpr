use std::time::{Duration as StdDuration, SystemTime};

use chrono::{DateTime, Duration, Utc};
use gateway_core::{
    account::{CredentialRevision, ProviderAccountId},
    provider_ports::{ProviderCooldownPort, ProviderCooldownScope, ProviderScopedCooldown},
};
use gateway_store::{
    Revision,
    redis::{CredentialCooldown, CredentialCooldownRepository, RedisCredentialCooldownRepository},
};
use redis::aio::ConnectionManager;
use uuid::Uuid;

#[test]
fn credential_cooldown_is_revision_fenced() {
    let cooldown = CredentialCooldown {
        provider_account_id: "account-1".to_owned(),
        credential_revision: Revision::new(2).expect("positive revision"),
        cooldown_until: Utc::now() + Duration::seconds(30),
    };
    assert_eq!(cooldown.credential_revision.get(), 2);
}

#[tokio::test]
async fn excel_quota_recovery_cannot_clear_newer_cooldown_or_model_scope() {
    let Some((repository, _, _)) = repository().await else {
        return;
    };
    let id = ProviderAccountId::new("acct_excel_recovery").unwrap();
    let current = cooldown(id.as_str(), 2, 30);
    repository
        .cache_credential_cooldown(&current)
        .await
        .unwrap();
    let observed = ProviderCooldownPort::read(&repository, &id)
        .await
        .unwrap()
        .unwrap();
    let newer = cooldown(id.as_str(), 2, 60);
    repository.cache_credential_cooldown(&newer).await.unwrap();
    assert!(!repository.clear_if_observed(&observed).await.unwrap());
    let updated = ProviderCooldownPort::read(&repository, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(DateTime::<Utc>::from(updated.until()), newer.cooldown_until);
    let scope = ProviderCooldownScope::UpstreamModel(
        gateway_core::routing::UpstreamModelId::new("fixture-model").unwrap(),
    );
    repository
        .put_scoped_if_later(ProviderScopedCooldown::new(
            id.clone(),
            CredentialRevision::new(2).unwrap(),
            scope.clone(),
            SystemTime::now() + StdDuration::from_secs(60),
        ))
        .await
        .unwrap();
    assert!(repository.clear_if_observed(&updated).await.unwrap());
    assert!(
        ProviderCooldownPort::read(&repository, &id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(repository.read_scoped(&id, &scope).await.unwrap().is_some());
}

#[tokio::test]
async fn excel_quota_recovery_fences_equal_shorter_and_recreated_cooldowns() {
    let Some((repository, _, _)) = repository().await else {
        return;
    };
    let id = ProviderAccountId::new("acct_excel_recovery_generation").unwrap();
    let initial = cooldown(id.as_str(), 2, 120);
    repository
        .cache_credential_cooldown(&initial)
        .await
        .unwrap();
    for seconds in [0, -30] {
        let observed = ProviderCooldownPort::read(&repository, &id)
            .await
            .unwrap()
            .unwrap();
        let repeated = CredentialCooldown {
            cooldown_until: initial.cooldown_until + Duration::seconds(seconds),
            ..initial.clone()
        };
        assert!(
            !repository
                .cache_credential_cooldown(&repeated)
                .await
                .unwrap()
        );
        assert!(!repository.clear_if_observed(&observed).await.unwrap());
        let updated = ProviderCooldownPort::read(&repository, &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.until(), observed.until());
        assert_ne!(updated.observation_token(), observed.observation_token());
    }

    let observed = ProviderCooldownPort::read(&repository, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(repository.clear_if_observed(&observed).await.unwrap());
    repository
        .cache_credential_cooldown(&initial)
        .await
        .unwrap();
    assert!(!repository.clear_if_observed(&observed).await.unwrap());
    let recreated = ProviderCooldownPort::read(&repository, &id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(recreated.observation_token(), observed.observation_token());

    let stale = cooldown(id.as_str(), 1, 240);
    assert!(!repository.cache_credential_cooldown(&stale).await.unwrap());
    let expired = cooldown(id.as_str(), 2, -1);
    assert!(
        !repository
            .cache_credential_cooldown(&expired)
            .await
            .unwrap()
    );
    assert!(repository.clear_if_observed(&recreated).await.unwrap());
}

#[tokio::test]
async fn excel_quota_recovery_does_not_clear_legacy_cooldown_without_generation() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let id = ProviderAccountId::new("acct_excel_recovery_legacy").unwrap();
    repository
        .cache_credential_cooldown(&cooldown(id.as_str(), 2, 120))
        .await
        .unwrap();
    let key = namespace_keys(&mut connection, &namespace)
        .await
        .into_iter()
        .find(|key| key.ends_with(":cooldown"))
        .unwrap();
    redis::cmd("HDEL")
        .arg(key)
        .arg("observation_token")
        .query_async::<i64>(&mut connection)
        .await
        .unwrap();
    let observed = ProviderCooldownPort::read(&repository, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(observed.observation_token().is_none());
    assert!(!repository.clear_if_observed(&observed).await.unwrap());
    assert!(
        ProviderCooldownPort::read(&repository, &id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn credential_cooldown_round_trips_and_indexes_active_account_without_raw_id_in_keys() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let cooldown = cooldown("acct_cooldown_round_trip", 1, 30);

    assert!(
        repository
            .cache_credential_cooldown(&cooldown)
            .await
            .expect("cache cooldown")
    );
    assert_eq!(
        repository
            .read_credential_cooldown(&cooldown.provider_account_id)
            .await
            .expect("read cooldown"),
        Some(cooldown.clone())
    );
    let mut keys = namespace_keys(&mut connection, &namespace).await;
    keys.sort();
    assert_eq!(keys.len(), 2);
    assert!(
        keys.iter()
            .all(|key| !key.contains(&cooldown.provider_account_id))
    );

    let index_key = keys
        .iter()
        .find(|key| key.ends_with(":account:active-cooldowns"))
        .expect("active cooldown index key");
    let indexed_accounts: Vec<String> = redis::cmd("ZRANGE")
        .arg(index_key)
        .arg(0)
        .arg(-1)
        .query_async(&mut connection)
        .await
        .expect("read active cooldown index");
    assert_eq!(indexed_accounts, [cooldown.provider_account_id]);
}

#[tokio::test]
async fn credential_cooldown_rejects_older_revision_and_fences_invalidation() {
    let Some((repository, _connection, _namespace)) = repository().await else {
        return;
    };
    let current = cooldown("acct_cooldown_revision", 2, 30);
    let stale = cooldown("acct_cooldown_revision", 1, 60);
    repository
        .cache_credential_cooldown(&current)
        .await
        .expect("cache current cooldown");

    assert!(
        !repository
            .cache_credential_cooldown(&stale)
            .await
            .expect("reject stale cooldown")
    );
    assert!(
        !repository
            .invalidate_credential_cooldown(
                &current.provider_account_id,
                Revision::new(1).expect("positive revision"),
            )
            .await
            .expect("fence stale invalidation")
    );
    assert!(
        repository
            .invalidate_credential_cooldown(
                &current.provider_account_id,
                current.credential_revision,
            )
            .await
            .expect("invalidate current cooldown")
    );
    assert_eq!(
        repository
            .read_credential_cooldown(&current.provider_account_id)
            .await
            .expect("read invalidated cooldown"),
        None
    );
}

#[tokio::test]
async fn credential_cooldown_same_revision_only_extends_deadline() {
    let Some((repository, _connection, _namespace)) = repository().await else {
        return;
    };
    let initial = cooldown("acct_cooldown_extend", 3, 30);
    let shorter = CredentialCooldown {
        cooldown_until: initial.cooldown_until - Duration::seconds(5),
        ..initial.clone()
    };
    let longer = CredentialCooldown {
        cooldown_until: initial.cooldown_until + Duration::seconds(5),
        ..initial.clone()
    };
    repository
        .cache_credential_cooldown(&initial)
        .await
        .expect("cache initial cooldown");

    assert!(
        !repository
            .cache_credential_cooldown(&shorter)
            .await
            .expect("reject shorter cooldown")
    );
    assert!(
        repository
            .cache_credential_cooldown(&longer)
            .await
            .expect("extend cooldown")
    );
    assert_eq!(
        repository
            .read_credential_cooldown(&initial.provider_account_id)
            .await
            .expect("read extended cooldown"),
        Some(longer)
    );
}

#[tokio::test]
async fn credential_cooldown_read_removes_expired_grace_key() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let cooldown_until = Utc::now() + Duration::milliseconds(40);
    let cooldown = CredentialCooldown {
        provider_account_id: "acct_cooldown_expiry".to_owned(),
        credential_revision: Revision::new(1).expect("positive revision"),
        cooldown_until: millisecond_precision(cooldown_until),
    };
    repository
        .cache_credential_cooldown(&cooldown)
        .await
        .expect("cache short cooldown");
    tokio::time::sleep(StdDuration::from_millis(80)).await;

    assert_eq!(
        repository
            .read_credential_cooldown(&cooldown.provider_account_id)
            .await
            .expect("read expired cooldown"),
        None
    );
    assert!(namespace_keys(&mut connection, &namespace).await.is_empty());
}

#[tokio::test]
async fn scoped_cooldown_isolated_by_model_and_revision_fenced() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let account_id = ProviderAccountId::new("acct_scoped_cooldown").expect("valid account ID");
    let revision = CredentialRevision::new(2).expect("positive revision");
    let model_a = ProviderCooldownScope::upstream_model(
        gateway_core::routing::UpstreamModelId::new("grok-4.5").expect("model"),
    );
    let model_b = ProviderCooldownScope::upstream_model(
        gateway_core::routing::UpstreamModelId::new("grok-4.6").expect("model"),
    );
    let until_a: SystemTime = millisecond_precision(Utc::now() + Duration::seconds(30)).into();
    let until_b: SystemTime = millisecond_precision(Utc::now() + Duration::seconds(60)).into();

    assert!(
        repository
            .put_scoped_if_later(ProviderScopedCooldown::new(
                account_id.clone(),
                revision,
                model_a.clone(),
                until_a,
            ))
            .await
            .expect("cache model A cooldown")
    );
    assert!(
        !repository
            .put_scoped_if_later(ProviderScopedCooldown::new(
                account_id.clone(),
                CredentialRevision::new(1).expect("stale revision"),
                model_a.clone(),
                until_b,
            ))
            .await
            .expect("reject stale model A cooldown")
    );
    assert!(
        repository
            .put_scoped_if_later(ProviderScopedCooldown::new(
                account_id.clone(),
                revision,
                model_b.clone(),
                until_b,
            ))
            .await
            .expect("cache model B cooldown")
    );
    assert_eq!(
        repository
            .read_scoped(&account_id, &model_a)
            .await
            .expect("read model A cooldown")
            .expect("model A cooldown")
            .until(),
        until_a
    );
    assert_eq!(
        repository
            .read_scoped(&account_id, &model_b)
            .await
            .expect("read model B cooldown")
            .expect("model B cooldown")
            .until(),
        until_b
    );
    assert!(
        !repository
            .clear_scoped(
                &account_id,
                &model_a,
                CredentialRevision::new(1).expect("stale revision"),
            )
            .await
            .expect("fence stale model A invalidation")
    );
    assert!(
        repository
            .clear_scoped(&account_id, &model_a, revision)
            .await
            .expect("clear model A cooldown")
    );
    assert!(
        repository
            .read_scoped(&account_id, &model_a)
            .await
            .expect("read cleared model A cooldown")
            .is_none()
    );
    assert!(
        repository
            .read_scoped(&account_id, &model_b)
            .await
            .expect("read retained model B cooldown")
            .is_some()
    );
    let keys = namespace_keys(&mut connection, &namespace).await;
    assert_eq!(keys.len(), 1);
    assert!(!keys[0].contains(account_id.as_str()));
    assert!(!keys[0].contains(model_b.value()));
}

fn cooldown(account_id: &str, revision: u64, seconds: i64) -> CredentialCooldown {
    CredentialCooldown {
        provider_account_id: account_id.to_owned(),
        credential_revision: Revision::new(revision).expect("positive revision"),
        cooldown_until: millisecond_precision(Utc::now() + Duration::seconds(seconds)),
    }
}

fn millisecond_precision(value: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(value.timestamp_millis()).expect("valid timestamp")
}

async fn repository() -> Option<(RedisCredentialCooldownRepository, ConnectionManager, String)> {
    let redis_url = crate::support::test_env("CPR_TEST_REDIS_URL")?;
    let client = redis::Client::open(redis_url).expect("valid CPR_TEST_REDIS_URL");
    let connection = client
        .get_connection_manager()
        .await
        .expect("connect test Redis");
    let namespace = format!("gateway-store-cooldown-test-{}", Uuid::new_v4());
    let repository = RedisCredentialCooldownRepository::new(connection.clone(), &namespace)
        .expect("valid cooldown namespace");
    Some((repository, connection, namespace))
}

async fn namespace_keys(connection: &mut ConnectionManager, namespace: &str) -> Vec<String> {
    redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(connection)
        .await
        .expect("list isolated cooldown keys")
}

fn protection_policy() -> gateway_core::account::AccountErrorProtection {
    use gateway_core::account::*;
    AccountErrorProtection {
        enabled: true,
        threshold: 2,
        duration_seconds: 300,
        rules: vec![AccountErrorRule {
            id: "busy".into(),
            name: "Busy upstream".into(),
            conditions: vec![AccountErrorCondition {
                field: AccountErrorField::Code,
                operator: AccountErrorOperator::Equals,
                values: vec![Some("busy".into())],
                ignore_case: false,
            }],
        }],
        ..Default::default()
    }
}

async fn protection_guard(
    repository: &RedisCredentialCooldownRepository,
    account: &ProviderAccountId,
    revision: u64,
    policy: &gateway_core::account::AccountErrorProtection,
    config: u64,
) -> gateway_core::provider_ports::AccountProtectionGuard {
    repository
        .begin_protection(
            account,
            CredentialRevision::new(revision).unwrap(),
            &policy.fingerprint(),
            gateway_core::routing::ConfigRevision::new(config).unwrap(),
        )
        .await
        .unwrap()
        .unwrap()
}

fn protection_runtime(
    repository: RedisCredentialCooldownRepository,
    connection: ConnectionManager,
    namespace: &str,
) -> gateway_store::redis::RedisAdminAccountRuntimeStore {
    use gateway_store::redis::*;
    RedisAdminAccountRuntimeStore::new(
        repository,
        RedisCredentialLeaseRepository::new(connection.clone(), namespace).unwrap(),
        RedisClientAdmissionRepository::new(connection, namespace).unwrap(),
    )
}

fn protection_matches() -> Vec<gateway_core::account::AccountProtectionMatch> {
    vec![gateway_core::account::AccountProtectionMatch {
        id: "busy".into(),
        name: "Busy upstream".into(),
    }]
}

#[tokio::test]
async fn protection_survives_unrelated_policy_publish_and_recovery_preserves_quota_cooldown() {
    use gateway_admin::ports::store::AccountRuntimeStore;
    let Some((repository, connection, namespace)) = repository().await else {
        return;
    };
    let runtime = protection_runtime(repository.clone(), connection, &namespace);
    let account = ProviderAccountId::new("acct_protected_account").unwrap();
    let revision = CredentialRevision::new(1).unwrap();
    let policy = protection_policy();
    let config = gateway_core::routing::ConfigRevision::new(5).unwrap();
    repository
        .publish_protection_policy(&policy, config)
        .await
        .unwrap();
    let guard = protection_guard(&repository, &account, 1, &policy, 5).await;
    assert!(
        !repository
            .record_protection_failure(&guard, &policy, &protection_matches())
            .await
            .unwrap()
    );
    repository
        .publish_protection_policy(
            &policy,
            gateway_core::routing::ConfigRevision::new(6).unwrap(),
        )
        .await
        .unwrap();
    let next = protection_guard(&repository, &account, 1, &policy, 5).await;
    assert_eq!(guard.policy_generation, next.policy_generation);
    assert!(
        repository
            .record_protection_failure(&next, &policy, &protection_matches())
            .await
            .unwrap()
    );
    let frozen = repository
        .read_protection(&account, revision)
        .await
        .unwrap()
        .unwrap();
    assert!(frozen.requires_probe);
    assert!(!repository.record_protection_success(&guard).await.unwrap());
    repository
        .cache_credential_cooldown(&cooldown(account.as_str(), 1, 3600))
        .await
        .unwrap();
    assert!(runtime.finish_protection(&frozen, None).await.unwrap());
    assert!(
        repository
            .read_protection(&account, revision)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        ProviderCooldownPort::read(&repository, &account)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        !repository
            .record_protection_failure(&guard, &policy, &protection_matches())
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn protection_credential_change_and_manual_clear_reject_late_feedback() {
    use gateway_admin::ports::store::AccountRuntimeStore;
    let Some((repository, connection, namespace)) = repository().await else {
        return;
    };
    let runtime = protection_runtime(repository.clone(), connection, &namespace);
    let policy = protection_policy();
    let account = ProviderAccountId::new("acct_protected_relogin").unwrap();
    repository
        .publish_protection_policy(
            &policy,
            gateway_core::routing::ConfigRevision::new(1).unwrap(),
        )
        .await
        .unwrap();
    let old = protection_guard(&repository, &account, 1, &policy, 1).await;
    assert!(
        repository
            .advance_protection_revision(&account, CredentialRevision::new(2).unwrap())
            .await
            .unwrap()
    );
    assert!(
        !repository
            .record_protection_failure(&old, &policy, &protection_matches())
            .await
            .unwrap()
    );
    assert!(
        repository
            .begin_protection(
                &account,
                CredentialRevision::new(1).unwrap(),
                &policy.fingerprint(),
                gateway_core::routing::ConfigRevision::new(1).unwrap()
            )
            .await
            .unwrap()
            .is_none()
    );
    let current = protection_guard(&repository, &account, 2, &policy, 1).await;
    runtime
        .clear_protection(
            account.as_str(),
            gateway_admin::model::Revision::new(2).unwrap(),
        )
        .await
        .unwrap();
    assert!(
        !repository
            .record_protection_failure(&current, &policy, &protection_matches())
            .await
            .unwrap()
    );
    assert!(
        repository
            .read_protection(&account, CredentialRevision::new(2).unwrap())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn protection_due_probe_remains_blocked_and_old_recovery_cannot_clear_postponed_generation() {
    use gateway_admin::ports::store::AccountRuntimeStore;
    let Some((repository, connection, namespace)) = repository().await else {
        return;
    };
    let runtime = protection_runtime(repository.clone(), connection.clone(), &namespace);
    let account = ProviderAccountId::new("acct_protected_probe").unwrap();
    let policy = protection_policy();
    repository
        .publish_protection_policy(
            &policy,
            gateway_core::routing::ConfigRevision::new(1).unwrap(),
        )
        .await
        .unwrap();
    let guard = protection_guard(&repository, &account, 1, &policy, 1).await;
    for _ in 0..2 {
        repository
            .record_protection_failure(&guard, &policy, &protection_matches())
            .await
            .unwrap();
    }
    let mut conn = connection;
    let key = namespace_keys(&mut conn, &namespace)
        .await
        .into_iter()
        .find(|key| key.ends_with(":error-protection"))
        .unwrap();
    let _: () = redis::cmd("HSET")
        .arg(&key)
        .arg("until_ms")
        .arg(Utc::now().timestamp_millis() - 1000)
        .query_async(&mut conn)
        .await
        .unwrap();
    let _: () = redis::cmd("ZADD")
        .arg(format!("{namespace}:account:active-protections"))
        .arg(0)
        .arg(account.as_str())
        .query_async(&mut conn)
        .await
        .unwrap();
    let state = repository
        .read_protection(&account, CredentialRevision::new(1).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(state.scheduling_until(SystemTime::now()) > SystemTime::now());
    let _: () = redis::cmd("ZADD")
        .arg(format!("{namespace}:account:active-protections"))
        .arg(-1)
        .arg("acct_removed")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(runtime.due_protections().await.unwrap().len(), 1);
    assert!(runtime.due_protections().await.unwrap().is_empty());
    assert!(
        runtime
            .finish_protection(&state, Some(Utc::now() + Duration::minutes(5)))
            .await
            .unwrap()
    );
    assert!(!runtime.finish_protection(&state, None).await.unwrap());
    assert!(
        repository
            .read_protection(&account, CredentialRevision::new(1).unwrap())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn protection_stale_disabled_publish_cannot_disable_a_newer_policy() {
    let Some((repository, _, _)) = repository().await else {
        return;
    };
    let policy = protection_policy();
    let account = ProviderAccountId::new("acct_protected_policy").unwrap();
    repository
        .publish_protection_policy(
            &policy,
            gateway_core::routing::ConfigRevision::new(3).unwrap(),
        )
        .await
        .unwrap();
    repository
        .publish_protection_policy(
            &Default::default(),
            gateway_core::routing::ConfigRevision::new(2).unwrap(),
        )
        .await
        .unwrap();
    let guard = protection_guard(&repository, &account, 1, &policy, 3).await;
    for _ in 0..2 {
        repository
            .record_protection_failure(&guard, &policy, &protection_matches())
            .await
            .unwrap();
    }
    assert!(
        repository
            .read_protection(&account, CredentialRevision::new(1).unwrap())
            .await
            .unwrap()
            .is_some()
    );
}
