use super::proxy_capacity::{acquire, key};
use gateway_core::account::OutboundProxy;
use std::time::Duration;
use tokio::time::timeout;

fn proxy(limit: u32) -> OutboundProxy {
    OutboundProxy::parse(&format!(
        "http://{}:secret@127.0.0.1:18080",
        uuid::Uuid::new_v4()
    ))
    .unwrap()
    .with_max_websocket_connections(limit)
}

#[tokio::test]
async fn proxy_budget_counts_unlimited_sockets_and_releases_cancelled_waiters() {
    let unlimited = proxy(0);
    let limited = unlimited.clone().with_max_websocket_connections(1);
    let first = acquire(Some(&unlimited), None).await.unwrap();
    let second = acquire(Some(&unlimited), None).await.unwrap();
    assert!(
        timeout(Duration::from_millis(10), acquire(Some(&limited), None))
            .await
            .is_err()
    );
    drop(first);
    assert!(
        timeout(Duration::from_millis(10), acquire(Some(&limited), None))
            .await
            .is_err()
    );
    drop(second);
    let next = timeout(Duration::from_secs(1), acquire(Some(&limited), None))
        .await
        .unwrap()
        .unwrap();
    drop(next);
    assert!(
        timeout(Duration::from_secs(1), acquire(Some(&limited), None))
            .await
            .unwrap()
            .is_ok()
    );
}

#[tokio::test]
async fn proxy_budget_separates_credentials_and_direct_connections() {
    let first = proxy(1);
    let second = proxy(1);
    assert_ne!(key(Some(&first)), key(Some(&second)));
    let _first = acquire(Some(&first), None).await.unwrap();
    let _second = timeout(Duration::from_secs(1), acquire(Some(&second), None))
        .await
        .unwrap()
        .unwrap();
    assert!(acquire(None, None).await.unwrap().is_none());
}

#[tokio::test]
async fn proxy_budget_cannot_oversell_under_concurrent_acquisition() {
    let proxy = proxy(3);
    let held = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let proxy = proxy.clone();
        let held = held.clone();
        tasks.spawn(async move {
            if let Ok(Ok(permit)) =
                timeout(Duration::from_millis(30), acquire(Some(&proxy), None)).await
            {
                held.lock().await.push(permit);
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    assert_eq!(held.lock().await.len(), 3);
    held.lock().await.clear();
    assert!(
        timeout(Duration::from_secs(1), acquire(Some(&proxy), None))
            .await
            .unwrap()
            .is_ok()
    );
}
