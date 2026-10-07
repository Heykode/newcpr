use std::time::Duration;

use gateway_core::account::ProviderAccountId;
use gateway_core::provider_ports::{
    BindingToken, ProviderSessionAffinityBinding, ProviderSessionAffinityKey,
    ProviderSessionAffinityPort, ProviderSessionAlias,
};
use gateway_core::routing::ProviderKind;
use gateway_store::redis::RedisProviderSessionAffinityRepository;
use redis::aio::ConnectionManager;
use uuid::Uuid;

#[tokio::test]
async fn renewal_supports_720_hours_but_never_recreates_or_replaces_a_binding() {
    let Some((repository, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let key = ProviderSessionAffinityKey::try_new("retained-session").unwrap();
    let binding = ProviderSessionAffinityBinding::new(
        ProviderAccountId::new("acct_retained").unwrap(),
        BindingToken::generate(),
    );
    let ttl = Duration::from_secs(720 * 3600);
    assert!(
        !repository
            .renew_binding(&provider, &key, &binding, ttl)
            .await
            .unwrap()
    );
    repository
        .bind_binding(&provider, &key, &binding, Duration::from_secs(10))
        .await
        .unwrap();
    assert!(
        repository
            .renew_binding(&provider, &key, &binding, ttl)
            .await
            .unwrap()
    );
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(&mut connection)
        .await
        .unwrap();
    let before: i64 = redis::cmd("PTTL")
        .arg(&keys[0])
        .query_async(&mut connection)
        .await
        .unwrap();
    assert!((720 * 3600 * 1000 - 5000..=720 * 3600 * 1000).contains(&before));
    let stale =
        ProviderSessionAffinityBinding::new(binding.account_id().clone(), BindingToken::generate());
    assert!(
        !repository
            .renew_binding(&provider, &key, &stale, Duration::from_secs(1))
            .await
            .unwrap()
    );
    assert_eq!(
        repository.load_binding(&provider, &key).await.unwrap(),
        Some(binding.clone())
    );
    repository.clear(&provider, &key).await.unwrap();
    assert!(
        !repository
            .renew_binding(&provider, &key, &binding, ttl)
            .await
            .unwrap()
    );
    assert!(
        repository
            .load_binding(&provider, &key)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn same_account_generation_is_fenced_across_late_completion_and_legacy_calls() {
    let Some((repository, _, _)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let key = ProviderSessionAffinityKey::try_new("generation-fence").unwrap();
    let account = ProviderAccountId::new("acct_generation").unwrap();
    let old = ProviderSessionAffinityBinding::new(account.clone(), BindingToken::generate());
    let replacement =
        ProviderSessionAffinityBinding::new(account.clone(), BindingToken::generate());
    let late = ProviderSessionAffinityBinding::new(
        ProviderAccountId::new("acct_late").unwrap(),
        BindingToken::generate(),
    );
    let ttl = Duration::from_secs(60);
    assert_eq!(
        repository
            .claim_or_load_binding(&provider, &key, &old, ttl)
            .await
            .unwrap(),
        old
    );
    assert_eq!(
        repository
            .compare_and_bind_binding(&provider, &key, &old, &replacement, ttl)
            .await
            .unwrap(),
        replacement
    );
    assert_eq!(
        repository
            .compare_and_bind_binding(&provider, &key, &old, &late, ttl)
            .await
            .unwrap(),
        replacement
    );
    assert!(
        !repository
            .renew_binding(&provider, &key, &old, ttl)
            .await
            .unwrap()
    );
    assert_eq!(
        repository
            .compare_and_bind(&provider, &key, &account, late.account_id(), ttl)
            .await
            .unwrap(),
        account
    );
    assert_eq!(
        repository.load_binding(&provider, &key).await.unwrap(),
        Some(replacement)
    );
}

#[tokio::test]
async fn concurrent_claims_keep_one_complete_generation() {
    let Some((repository, _, _)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let key = ProviderSessionAffinityKey::try_new("concurrent-generation").unwrap();
    let first = ProviderSessionAffinityBinding::new(
        ProviderAccountId::new("acct_first").unwrap(),
        BindingToken::generate(),
    );
    let second = ProviderSessionAffinityBinding::new(
        ProviderAccountId::new("acct_second").unwrap(),
        BindingToken::generate(),
    );
    let ttl = Duration::from_secs(60);
    let (a, b) = tokio::join!(
        repository.claim_or_load_binding(&provider, &key, &first, ttl),
        repository.claim_or_load_binding(&provider, &key, &second, ttl),
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert!(a == first || a == second);
    assert_eq!(
        repository.load_binding(&provider, &key).await.unwrap(),
        Some(a)
    );
}

#[tokio::test]
async fn legacy_values_upgrade_atomically_and_invalid_ttls_do_not_mutate() {
    let Some((repository, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let key = ProviderSessionAffinityKey::try_new("legacy-upgrade").unwrap();
    let account = ProviderAccountId::new("acct_legacy").unwrap();
    let ttl = Duration::from_secs(60);
    repository
        .bind(&provider, &key, &account, ttl)
        .await
        .unwrap();
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(&mut connection)
        .await
        .unwrap();
    redis::cmd("SET")
        .arg(&keys[0])
        .arg(account.as_str())
        .arg("PX")
        .arg(60_000)
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    let legacy = repository
        .load_binding(&provider, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        legacy,
        ProviderSessionAffinityBinding::legacy(account.clone())
    );
    let replacement = ProviderSessionAffinityBinding::new(account, BindingToken::generate());
    assert_eq!(
        repository
            .compare_and_bind_binding(&provider, &key, &legacy, &replacement, ttl)
            .await
            .unwrap(),
        replacement
    );
    for ttl in [Duration::ZERO, Duration::from_nanos(1), Duration::MAX] {
        assert!(
            repository
                .renew_binding(&provider, &key, &replacement, ttl)
                .await
                .is_err()
        );
        assert!(
            repository
                .bind_binding(&provider, &key, &legacy, ttl)
                .await
                .is_err()
        );
        assert_eq!(
            repository.load_binding(&provider, &key).await.unwrap(),
            Some(replacement.clone())
        );
    }
    redis::cmd("SET")
        .arg(&keys[0])
        .arg("false")
        .arg("PX")
        .arg(60_000)
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    let error = repository
        .compare_and_bind_binding(&provider, &key, &replacement, &legacy, ttl)
        .await
        .unwrap_err();
    assert_eq!(
        error.kind(),
        gateway_core::provider_ports::ProviderStoreErrorKind::InvalidData
    );
    assert!(
        !repository
            .renew_binding(&provider, &key, &replacement, ttl)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn alias_refresh_retains_original_follow_only_semantics() {
    let Some((repository, _, _)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let root = ProviderSessionAffinityKey::try_new("root").unwrap();
    for follow_only in [true, false] {
        let key = ProviderSessionAffinityKey::try_new(format!("alias-{follow_only}")).unwrap();
        let original = ProviderSessionAlias::new(root.clone(), follow_only);
        assert!(
            repository
                .bind_alias(&provider, &key, &original, Duration::from_secs(60))
                .await
                .unwrap()
        );
        assert!(
            repository
                .bind_alias(
                    &provider,
                    &key,
                    &ProviderSessionAlias::new(root.clone(), !follow_only),
                    Duration::from_secs(720 * 3600)
                )
                .await
                .unwrap()
        );
        assert_eq!(
            repository.load_alias(&provider, &key).await.unwrap(),
            Some(original)
        );
    }
}

#[tokio::test]
async fn child_alias_retains_root_metadata_and_rejects_root_replacement() {
    let Some((repository, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let key = ProviderSessionAffinityKey::try_new("child-alias").unwrap();
    let child = ProviderSessionAffinityKey::try_new("child").unwrap();
    let root = ProviderSessionAffinityKey::try_new("root").unwrap();
    let original =
        ProviderSessionAlias::new(child.clone(), false).with_root_session_key(Some(root.clone()));
    assert!(
        repository
            .bind_alias(&provider, &key, &original, Duration::from_secs(1))
            .await
            .unwrap()
    );
    let same_target =
        ProviderSessionAlias::new(child.clone(), true).with_root_session_key(Some(root));
    assert!(
        repository
            .bind_alias(&provider, &key, &same_target, Duration::from_secs(60))
            .await
            .unwrap()
    );
    for root in [
        None,
        Some(ProviderSessionAffinityKey::try_new("another-root").unwrap()),
    ] {
        let conflicting =
            ProviderSessionAlias::new(child.clone(), false).with_root_session_key(root);
        assert!(
            !repository
                .bind_alias(&provider, &key, &conflicting, Duration::from_secs(1))
                .await
                .unwrap()
        );
    }
    assert_eq!(
        repository.load_alias(&provider, &key).await.unwrap(),
        Some(original)
    );
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(&mut connection)
        .await
        .unwrap();
    let ttl: i64 = redis::cmd("PTTL")
        .arg(&keys[0])
        .query_async(&mut connection)
        .await
        .unwrap();
    assert!((55_000..=60_000).contains(&ttl));
    // A pre-upgrade alias has no root metadata and must keep its follow-only flag.
    redis::cmd("SET")
        .arg(&keys[0])
        .arg(r#"{"sessionKey":"legacy-root","followOnly":true}"#)
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    let legacy = repository
        .load_alias(&provider, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(legacy.session_key().expose_to_store(), "legacy-root");
    assert!(legacy.follow_only());
    assert!(legacy.root_session_key().is_none());
}

#[tokio::test]
async fn session_affinity_should_round_trip_without_exposing_the_raw_session_key() {
    let Some((repository, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").expect("provider");
    let key = ProviderSessionAffinityKey::try_new("session-secret-value").expect("affinity key");
    let account = ProviderAccountId::new("acct_first").expect("account");

    repository
        .bind(&provider, &key, &account, Duration::from_secs(60))
        .await
        .expect("bind affinity");

    assert_eq!(
        repository
            .load(&provider, &key)
            .await
            .expect("load affinity"),
        Some(account)
    );
    let keys = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async::<Vec<String>>(&mut connection)
        .await
        .expect("list affinity keys");
    assert_eq!(keys.len(), 1);
    assert!(!keys[0].contains("session-secret-value"));
}

#[tokio::test]
async fn session_affinity_should_overwrite_the_previous_account() {
    let Some((repository, _connection, _namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").expect("provider");
    let key = ProviderSessionAffinityKey::try_new("overwrite-session").expect("affinity key");
    let first = ProviderAccountId::new("acct_first").expect("first account");
    let second = ProviderAccountId::new("acct_second").expect("second account");

    repository
        .bind(&provider, &key, &first, Duration::from_secs(60))
        .await
        .expect("bind first affinity");
    repository
        .bind(&provider, &key, &second, Duration::from_secs(60))
        .await
        .expect("overwrite affinity");

    assert_eq!(
        repository
            .load(&provider, &key)
            .await
            .expect("load affinity"),
        Some(second)
    );
}

#[tokio::test]
async fn session_affinity_claim_should_keep_the_first_account() {
    let Some((repository, _connection, _namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").expect("provider");
    let key = ProviderSessionAffinityKey::try_new("claim-session").expect("affinity key");
    let first = ProviderAccountId::new("acct_first").expect("first account");
    let second = ProviderAccountId::new("acct_second").expect("second account");

    let first_winner = repository
        .claim_or_load(&provider, &key, &first, Duration::from_secs(60))
        .await
        .expect("claim first affinity");
    let second_winner = repository
        .claim_or_load(&provider, &key, &second, Duration::from_secs(60))
        .await
        .expect("load existing affinity");

    assert_eq!((first_winner, second_winner), (first.clone(), first));
}

#[tokio::test]
async fn session_affinity_compare_should_not_replace_a_newer_winner() {
    let Some((repository, _connection, _namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").expect("provider");
    let key = ProviderSessionAffinityKey::try_new("compare-session").expect("affinity key");
    let first = ProviderAccountId::new("acct_first").expect("first account");
    let second = ProviderAccountId::new("acct_second").expect("second account");
    let stale = ProviderAccountId::new("acct_stale").expect("stale account");
    repository
        .bind(&provider, &key, &first, Duration::from_secs(60))
        .await
        .expect("seed affinity");

    let migrated = repository
        .compare_and_bind(&provider, &key, &first, &second, Duration::from_secs(60))
        .await
        .expect("migrate expected affinity");
    let stale_result = repository
        .compare_and_bind(&provider, &key, &first, &stale, Duration::from_secs(60))
        .await
        .expect("reject stale affinity migration");

    assert_eq!((migrated, stale_result), (second.clone(), second));
}

#[tokio::test]
async fn session_affinity_should_apply_ttl_and_support_explicit_clear() {
    let Some((repository, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").expect("provider");
    let key = ProviderSessionAffinityKey::try_new("ttl-session").expect("affinity key");
    let account = ProviderAccountId::new("acct_ttl").expect("account");

    repository
        .bind(&provider, &key, &account, Duration::from_secs(60))
        .await
        .expect("bind affinity");
    let redis_key = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async::<Vec<String>>(&mut connection)
        .await
        .expect("list affinity keys")
        .into_iter()
        .next()
        .expect("affinity key exists");
    let ttl = redis::cmd("PTTL")
        .arg(redis_key)
        .query_async::<i64>(&mut connection)
        .await
        .expect("read affinity TTL");
    assert!((1..=60_000).contains(&ttl));

    assert!(
        repository
            .clear(&provider, &key)
            .await
            .expect("clear affinity")
    );
    assert_eq!(
        repository
            .load(&provider, &key)
            .await
            .expect("load cleared affinity"),
        None
    );
}

#[tokio::test]
async fn session_alias_should_round_trip_with_an_independent_key_and_ttl() {
    let Some((repository, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").expect("provider");
    let alias_key = ProviderSessionAffinityKey::try_new("turn-secret").expect("alias key");
    let session_key = ProviderSessionAffinityKey::try_new("session-secret").expect("session key");
    let alias = ProviderSessionAlias::new(session_key, true);

    assert!(
        repository
            .bind_alias(&provider, &alias_key, &alias, Duration::from_secs(60))
            .await
            .expect("bind alias")
    );
    assert_eq!(
        repository
            .load_alias(&provider, &alias_key)
            .await
            .expect("load alias"),
        Some(alias)
    );

    let keys = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async::<Vec<String>>(&mut connection)
        .await
        .expect("list alias keys");
    assert_eq!(keys.len(), 1);
    assert!(keys[0].contains(":scheduler:session-alias:"));
    assert!(!keys[0].contains("turn-secret"));
    assert!(!keys[0].contains("session-secret"));

    let ttl = redis::cmd("PTTL")
        .arg(&keys[0])
        .query_async::<i64>(&mut connection)
        .await
        .expect("read alias TTL");
    assert!((1..=60_000).contains(&ttl));
}

#[tokio::test]
async fn session_alias_should_reject_conflicting_targets_but_allow_same_target_refresh() {
    let Some((repository, _connection, _namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").expect("provider");
    let alias_key = ProviderSessionAffinityKey::try_new("turn-conflict").expect("alias key");
    let first = ProviderSessionAlias::new(
        ProviderSessionAffinityKey::try_new("session-first").expect("first session"),
        true,
    );
    let second = ProviderSessionAlias::new(
        ProviderSessionAffinityKey::try_new("session-second").expect("second session"),
        true,
    );

    assert!(
        repository
            .bind_alias(&provider, &alias_key, &first, Duration::from_secs(60))
            .await
            .expect("bind first alias")
    );
    assert!(
        repository
            .bind_alias(&provider, &alias_key, &first, Duration::from_secs(60))
            .await
            .expect("refresh first alias")
    );
    assert!(
        !repository
            .bind_alias(&provider, &alias_key, &second, Duration::from_secs(60))
            .await
            .expect("reject conflicting alias")
    );
    assert_eq!(
        repository
            .load_alias(&provider, &alias_key)
            .await
            .expect("load first alias"),
        Some(first)
    );
}

async fn affinity_repository() -> Option<(
    RedisProviderSessionAffinityRepository,
    ConnectionManager,
    String,
)> {
    let redis_url = crate::support::test_env("CPR_TEST_REDIS_URL")?;
    let client = redis::Client::open(redis_url).expect("valid CPR_TEST_REDIS_URL");
    let connection = client
        .get_connection_manager()
        .await
        .expect("connect test Redis");
    let namespace = format!("gateway-store-affinity-test-{}", Uuid::new_v4());
    let repository = RedisProviderSessionAffinityRepository::new(connection.clone(), &namespace)
        .expect("valid test namespace");
    Some((repository, connection, namespace))
}
