use gateway_admin::{
    model::{
        MutationActor, MutationContext,
        proxies::{NewProxy, ProxyTestResult, UpdateProxy},
    },
    ports::{proxy::ProxyStore, store::AdminStoreErrorKind},
};
use gateway_core::account::OutboundProxy;
use gateway_store::postgres::PgProxyRepository;

use super::TestDatabase;

fn result(success: bool) -> ProxyTestResult {
    ProxyTestResult {
        success,
        latency_ms: 10,
        exit_ip: None,
        exit_ipv4: None,
        exit_ipv6: None,
        message: "Synthetic proxy test".to_owned(),
    }
}

#[tokio::test]
async fn stale_proxy_test_releases_lock_before_next_valid_result() {
    let Some(database) = TestDatabase::create("proxy_test_lock_release").await else {
        return;
    };
    let first = PgProxyRepository::new(database.pool.clone());
    let second = PgProxyRepository::new(database.pool.clone());
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "proxy-test-lock-release".to_owned(),
    };
    let created = first
        .create(
            NewProxy {
                name: "Lock fixture".to_owned(),
                proxy: OutboundProxy::parse("http://127.0.0.1:8080").unwrap(),
                request_location: None,
            },
            &context,
        )
        .await
        .unwrap()
        .record;
    let current = first
        .update(
            UpdateProxy {
                id: created.id.clone(),
                revision: created.revision,
                name: "Updated lock fixture".to_owned(),
                proxy: None,
                request_location: None,
            },
            &context,
        )
        .await
        .unwrap()
        .record;

    for attempt in 0..20 {
        assert_eq!(
            first
                .record_test(&created.id, created.revision, result(false), &context)
                .await
                .unwrap_err()
                .kind(),
            AdminStoreErrorKind::Conflict
        );
        // No sleep or retry: the stale writer must have released its lock.
        let saved = second
            .record_test(&current.id, current.revision, result(true), &context)
            .await
            .unwrap();
        assert_eq!(saved.last_test, Some(result(true)), "attempt {attempt}");
    }
    database.close().await;
}
