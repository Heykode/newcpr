use gateway_core::{account::OpaqueProviderData, provider_ports::ProviderReplayPort};
use gateway_store::redis::RedisProviderReplayRepository;
use serde_json::json;

#[tokio::test]
async fn excel_assets_are_isolated_persistent_bounded_and_conditionally_invalidated() {
    let Some(url) = crate::support::test_env("CPR_TEST_REDIS_URL") else {
        return;
    };
    let mut connection = redis::Client::open(url)
        .unwrap()
        .get_connection_manager()
        .await
        .unwrap();
    let namespace = format!("test-assets-{}", uuid::Uuid::new_v4());
    let store = RedisProviderReplayRepository::new(connection.clone(), &namespace).unwrap();
    let key = format!("{:064x}", 1);
    let first = OpaqueProviderData::new(
        json!({"version":1,"file_id":"file_first"})
            .as_object()
            .unwrap()
            .clone(),
    );
    let next = OpaqueProviderData::new(
        json!({"version":1,"file_id":"file_next"})
            .as_object()
            .unwrap()
            .clone(),
    );
    store.write(&key, &first).await.unwrap();
    assert_eq!(store.store_asset(&key, &first).await.unwrap(), first);
    let prefix = format!("{namespace}:{{provider-replay-v1}}:assets");
    let expiry: i64 = redis::cmd("ZSCORE")
        .arg(format!("{prefix}:expiry"))
        .arg(&key)
        .query_async(&mut connection)
        .await
        .unwrap();
    redis::cmd("ZADD")
        .arg(format!("{prefix}:expiry"))
        .arg(expiry - 20)
        .arg(&key)
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    assert_eq!(store.store_asset(&key, &next).await.unwrap(), first);
    let cold = RedisProviderReplayRepository::new(connection.clone(), &namespace).unwrap();
    assert_eq!(cold.read_asset(&key).await.unwrap(), Some(first.clone()));
    let unchanged: i64 = redis::cmd("ZSCORE")
        .arg(format!("{prefix}:expiry"))
        .arg(&key)
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(unchanged, expiry - 20);
    let ttl: i64 = redis::cmd("TTL")
        .arg(format!("{prefix}:data"))
        .query_async(&mut connection)
        .await
        .unwrap();
    assert!((86_300..=86_400).contains(&ttl));
    store.invalidate_asset(&key, &next).await.unwrap();
    assert_eq!(store.read_asset(&key).await.unwrap(), Some(first.clone()));
    store.invalidate_asset(&key, &first).await.unwrap();
    assert!(store.read_asset(&key).await.unwrap().is_none());
    assert_eq!(store.read(&key).await.unwrap(), Some(first.clone()));

    let (left, right) = tokio::join!(
        store.store_asset(&key, &first),
        cold.store_asset(&key, &next)
    );
    assert_eq!(left.unwrap(), right.unwrap());
    redis::cmd("ZADD")
        .arg(format!("{prefix}:expiry"))
        .arg(0)
        .arg(&key)
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    assert!(store.read_asset(&key).await.unwrap().is_none());
    store.store_asset(&key, &next).await.unwrap();
    store.invalidate_asset(&key, &first).await.unwrap();
    assert_eq!(store.read_asset(&key).await.unwrap(), Some(next.clone()));

    for n in 0..514 {
        store
            .store_asset(&format!("{n:064x}"), &first)
            .await
            .unwrap();
    }
    let count: usize = redis::cmd("HLEN")
        .arg(format!("{prefix}:data"))
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(count, 512);
    let oversized = OpaqueProviderData::new(
        json!({"file_id":"x".repeat(2048)})
            .as_object()
            .unwrap()
            .clone(),
    );
    assert!(store.store_asset(&key, &oversized).await.is_err());
    assert!(store.read_asset("invalid-key").await.is_err());
    assert!(store.invalidate_asset("invalid-key", &first).await.is_err());
    for suffix in ["data", "expiry", "sizes", "assets:data", "assets:expiry"] {
        redis::cmd("DEL")
            .arg(format!("{namespace}:{{provider-replay-v1}}:{suffix}"))
            .query_async::<()>(&mut connection)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn excel_replay_is_immutable_expires_and_respects_capacity() {
    let Some(url) = crate::support::test_env("CPR_TEST_REDIS_URL") else {
        return;
    };
    let mut connection = redis::Client::open(url)
        .unwrap()
        .get_connection_manager()
        .await
        .unwrap();
    let namespace = format!("test-excel-{}", uuid::Uuid::new_v4());
    let store = RedisProviderReplayRepository::new(connection.clone(), &namespace).unwrap();
    let key = format!("{:064x}", 1);
    let payload = OpaqueProviderData::new(
        json!({"input":"synthetic conversation"})
            .as_object()
            .unwrap()
            .clone(),
    );
    store.write(&key, &payload).await.unwrap();
    store.write(&key, &payload).await.unwrap();
    assert_eq!(store.read(&key).await.unwrap(), Some(payload.clone()));
    let different = OpaqueProviderData::new(json!({"input":"other"}).as_object().unwrap().clone());
    assert!(store.write(&key, &different).await.is_err());
    let prefix = format!("{namespace}:{{provider-replay-v1}}");
    let ttl: i64 = redis::cmd("TTL")
        .arg(format!("{prefix}:data"))
        .query_async(&mut connection)
        .await
        .unwrap();
    assert!((3500..=3600).contains(&ttl));
    redis::cmd("ZADD")
        .arg(format!("{prefix}:expiry"))
        .arg(0)
        .arg(&key)
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    assert!(store.read(&key).await.unwrap().is_none());
    for number in 0..2050 {
        store
            .write(&format!("{number:064x}"), &payload)
            .await
            .unwrap();
    }
    let size: usize = redis::cmd("ZCARD")
        .arg(format!("{prefix}:expiry"))
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(size, 2048);
    let stored_size: i64 = redis::cmd("HGET")
        .arg(format!("{prefix}:sizes"))
        .arg("_total")
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(
        stored_size as usize,
        2048 * serde_json::to_vec(payload.expose_to_provider())
            .unwrap()
            .len()
    );
    assert!(store.write("invalid-key", &payload).await.is_err());
    redis::cmd("DEL")
        .arg(format!("{prefix}:data"))
        .arg(format!("{prefix}:expiry"))
        .arg(format!("{prefix}:sizes"))
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
}
