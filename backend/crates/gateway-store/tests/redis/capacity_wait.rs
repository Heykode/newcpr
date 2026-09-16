use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::future::join_all;
use gateway_core::account::{CredentialRevision, ProviderAccountId};
use gateway_core::engine::{AccountWaitMode, ModelRequestId};
use gateway_core::lifecycle::CancellationToken;
use gateway_core::policy::ClientApiKeyId;
use gateway_core::provider_ports::{
    ProviderLeaseAcquisition, ProviderLeaseGuard, ProviderLeasePort,
    ProviderSchedulingLeaseRequest, ProviderWaitLease, ProviderWaitLeaseAcquisition,
    ProviderWaitLeaseRequest, ProviderWaitPromotion,
};
use gateway_core::routing::ProviderKind;
use gateway_core::task::DaemonTask;
use gateway_store::redis::{
    CapacityWaitCleanupWriter, CredentialBoundedLeaseAcquisition, CredentialBoundedLeaseRequest,
    CredentialLeaseRepository, CredentialLeaseScope, RedisCredentialLeaseRepository,
    RedisProviderLeaseCoordinator,
};
use redis::aio::ConnectionManager;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};
use tokio::task::{JoinHandle, JoinSet};
use uuid::Uuid;

#[tokio::test]
async fn capacity_wait_modes_share_atomic_limits_without_execution_or_cursor_changes() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    for (account, cap) in [("sticky", 3), ("fallback", 100)] {
        let requests = (0..cap + 20).map(|_| {
            fixture.port.try_acquire_wait(wait_request(
                account,
                AccountWaitMode::Fallback,
                cap,
                Duration::from_secs(30),
            ))
        });
        let mut guards = Vec::new();
        for outcome in join_all(requests).await {
            if let ProviderWaitLeaseAcquisition::Acquired(guard) =
                outcome.expect("atomic admission")
            {
                guards.push(guard);
            }
        }
        assert_eq!(guards.len(), cap as usize);
        assert_eq!(fixture.waiting(account).await, u64::from(cap));
        assert_eq!(fixture.in_flight(account).await, 0);
        assert!(matches!(
            fixture
                .port
                .try_acquire_wait(wait_request(
                    account,
                    AccountWaitMode::Sticky,
                    3,
                    Duration::from_secs(30),
                ))
                .await
                .expect("shared threshold"),
            ProviderWaitLeaseAcquisition::Full
        ));
        for guard in guards {
            guard.release().await.expect("await normal release");
        }
        assert_eq!(fixture.waiting(account).await, 0);
    }

    let client = ClientApiKeyId::new("key_wait_cursor").expect("client");
    let provider = provider();
    let accounts = [account("sticky")];
    let first = fixture
        .port
        .load_state(&client, &provider, &accounts)
        .await
        .expect("first state");
    for _ in 0..10 {
        let signals = fixture
            .port
            .load_signals(&provider, &accounts)
            .await
            .expect("pure signals");
        assert_eq!(signals[&accounts[0]].in_flight, 0);
        assert_eq!(signals[&accounts[0]].last_started_at, None);
    }
    let next = fixture
        .port
        .load_state(&client, &provider, &accounts)
        .await
        .expect("next state");
    assert_eq!(next.round_robin_cursor(), first.round_robin_cursor() + 1);
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_promotion_is_atomic_and_wait_release_cannot_delete_delivered_execution() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let mut waiters = Vec::new();
    for _ in 0..20 {
        waiters.push(fixture.wait("promote", Duration::from_secs(10)).await);
    }
    let promotions = join_all(waiters.iter_mut().map(|waiter| {
        waiter.try_promote(scheduling(
            "promote",
            3,
            Duration::ZERO,
            Duration::from_secs(30),
        ))
    }))
    .await;
    let mut executing = Vec::new();
    for promotion in promotions {
        match promotion.expect("atomic promotion") {
            ProviderWaitPromotion::Acquired(guard) => executing.push(guard),
            ProviderWaitPromotion::Busy { .. } => {}
            ProviderWaitPromotion::Expired => panic!("unexpired wait"),
        }
    }
    assert_eq!(executing.len(), 3);
    assert_eq!(fixture.waiting("promote").await, 17);
    assert_eq!(fixture.in_flight("promote").await, 3);
    for waiter in waiters {
        waiter
            .release()
            .await
            .expect("release original waiting guard");
    }
    assert_eq!(fixture.waiting("promote").await, 0);
    assert_eq!(fixture.in_flight("promote").await, 3);
    // A fresh coordinator has its own tracked writer but shares execution state.
    let peer_repo =
        RedisCredentialLeaseRepository::new(fixture.connection.clone(), &fixture.namespace)
            .expect("peer repository");
    let peer_request = CredentialBoundedLeaseRequest {
        scope: CredentialLeaseScope::ProviderAccount,
        resource_id: account("promote").as_str().to_owned(),
        owner_id: "legacy-peer".to_owned(),
        max_concurrent: 3,
        request_interval: Duration::ZERO,
        ttl: Duration::from_secs(30),
    };
    assert!(matches!(
        peer_repo
            .try_acquire_bounded_lease(&peer_request)
            .await
            .expect("shared old slots"),
        CredentialBoundedLeaseAcquisition::Busy { .. }
    ));
    // The writer must remain alive until execution ownership is dropped.
    drop(executing);
    fixture.drain().await;
    assert_eq!(fixture.in_flight("promote").await, 0);
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_promotion_honors_existing_slots_live_limit_and_interval() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let held = match fixture
        .repository
        .try_acquire_bounded_lease(&CredentialBoundedLeaseRequest {
            scope: CredentialLeaseScope::ProviderAccount,
            resource_id: account("legacy").as_str().to_owned(),
            owner_id: "legacy-owner".to_owned(),
            max_concurrent: 1,
            request_interval: Duration::ZERO,
            ttl: Duration::from_secs(30),
        })
        .await
        .expect("legacy slot")
    {
        CredentialBoundedLeaseAcquisition::Acquired(guard) => guard,
        CredentialBoundedLeaseAcquisition::Busy { .. } => panic!("empty account"),
    };
    let mut waiter = fixture.wait("legacy", Duration::from_secs(10)).await;
    assert!(matches!(
        waiter
            .try_promote(scheduling(
                "legacy",
                1,
                Duration::ZERO,
                Duration::from_secs(30)
            ))
            .await
            .expect("busy legacy slot"),
        ProviderWaitPromotion::Busy { .. }
    ));
    held.release().await.expect("release legacy slot");
    assert!(matches!(
        waiter
            .try_promote(scheduling(
                "legacy",
                2,
                Duration::from_secs(10),
                Duration::from_secs(30)
            ))
            .await
            .expect("interval remains blocked"),
        ProviderWaitPromotion::Busy { .. }
    ));
    let execution = promoted(
        waiter
            .try_promote(scheduling(
                "legacy",
                2,
                Duration::ZERO,
                Duration::from_secs(30),
            ))
            .await
            .expect("promote after capacity increase"),
    );
    waiter.release().await.expect("disarmed waiter");
    assert_eq!(fixture.in_flight("legacy").await, 1);
    drop(execution);
    fixture.drain().await;
    assert_eq!(fixture.in_flight("legacy").await, 0);
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_members_expire_independently_and_do_not_refresh_execution_deadline() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let mut short = fixture.wait("ttl", Duration::from_millis(60)).await;
    let long = fixture.wait("ttl", Duration::from_secs(10)).await;
    tokio::time::sleep(Duration::from_millis(90)).await;
    assert!(matches!(
        short
            .try_promote(scheduling(
                "ttl",
                1,
                Duration::ZERO,
                Duration::from_secs(30)
            ))
            .await
            .expect("expired waiting token"),
        ProviderWaitPromotion::Expired
    ));
    assert_eq!(fixture.waiting("ttl").await, 1);
    short.release().await.expect("repeat expired release");
    assert_eq!(fixture.waiting("ttl").await, 1);
    long.release().await.expect("remaining waiter");

    let mut waiter = fixture.wait("execution-ttl", Duration::from_secs(10)).await;
    let request = scheduling(
        "execution-ttl",
        1,
        Duration::ZERO,
        Duration::from_millis(140),
    );
    tokio::time::sleep(Duration::from_millis(60)).await;
    let executing = promoted(waiter.try_promote(request).await.expect("short execution"));
    assert_eq!(fixture.in_flight("execution-ttl").await, 1);
    tokio::time::sleep(Duration::from_millis(110)).await;
    assert_eq!(fixture.in_flight("execution-ttl").await, 0);
    drop(executing);
    drop(waiter);
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_deleted_token_and_mismatched_owner_fail_closed() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let mut waiter = fixture.wait("deleted", Duration::from_secs(10)).await;
    redis::cmd("DEL")
        .arg(fixture.waiting_key("deleted"))
        .query_async::<i64>(&mut fixture.connection)
        .await
        .expect("isolated cache loss");
    assert!(matches!(
        waiter
            .try_promote(scheduling(
                "deleted",
                1,
                Duration::ZERO,
                Duration::from_secs(30)
            ))
            .await
            .expect("missing token"),
        ProviderWaitPromotion::Expired
    ));
    let mut mismatch = fixture.wait("owner-a", Duration::from_secs(10)).await;
    assert!(
        mismatch
            .try_promote(scheduling(
                "owner-b",
                1,
                Duration::ZERO,
                Duration::from_secs(30)
            ))
            .await
            .is_err()
    );
    drop(mismatch);
    drop(waiter);
    fixture.drain().await;
    assert_eq!(fixture.in_flight("deleted").await, 0);
    assert_eq!(fixture.in_flight("owner-b").await, 0);
    assert_eq!(fixture.waiting("owner-a").await, 0);
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_unpolled_release_and_promotion_are_owned_and_shutdown_drains() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let waiter = fixture.wait("release-drop", Duration::from_secs(10)).await;
    drop(waiter.release());
    let mut waiter = fixture
        .wait("promotion-drop", Duration::from_secs(10))
        .await;
    drop(waiter.try_promote(scheduling(
        "promotion-drop",
        1,
        Duration::ZERO,
        Duration::from_secs(30),
    )));
    assert!(matches!(
        waiter
            .try_promote(scheduling(
                "promotion-drop",
                1,
                Duration::ZERO,
                Duration::from_secs(30)
            ))
            .await
            .expect("cancelled ownership is terminal"),
        ProviderWaitPromotion::Expired
    ));
    let untouched = fixture.wait("untouched", Duration::from_secs(10)).await;
    assert_eq!(fixture.waiting("untouched").await, 1);
    untouched
        .release()
        .await
        .expect("normal release leaves other cleanup intact");
    fixture.drain().await;
    assert_eq!(fixture.waiting("release-drop").await, 0);
    assert_eq!(fixture.waiting("promotion-drop").await, 0);
    assert_eq!(fixture.waiting("untouched").await, 0);
    assert!(
        fixture
            .port
            .try_acquire_wait(wait_request(
                "closed-writer",
                AccountWaitMode::Sticky,
                3,
                Duration::from_secs(10),
            ))
            .await
            .is_err()
    );
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_lost_enqueue_reply_cleans_committed_real_redis_write() {
    fault_scenario(Fault::LoseReply, false).await;
}

#[tokio::test]
async fn capacity_wait_lost_promotion_reply_cleans_undelivered_execution() {
    fault_scenario(Fault::LoseReply, true).await;
}

#[tokio::test]
async fn capacity_wait_tombstone_rejects_enqueue_arriving_after_cancel() {
    fault_scenario(Fault::DelayCommand, false).await;
}

#[tokio::test]
async fn capacity_wait_tombstone_rejects_promotion_arriving_after_cancel() {
    fault_scenario(Fault::DelayCommand, true).await;
}

#[tokio::test]
async fn capacity_wait_cancelled_future_cleans_write_before_reply_delivery() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let mut proxy = FaultProxy::start(Fault::HoldReply, 2).await;
    let connection = proxy.connection().await;
    let repository = RedisCredentialLeaseRepository::new(connection, &fixture.namespace)
        .expect("proxy repository");
    let (port, writer) = RedisProviderLeaseCoordinator::new(repository);
    let mut future = port.try_acquire_wait(wait_request(
        "cancel-pending",
        AccountWaitMode::Sticky,
        3,
        Duration::from_secs(10),
    ));
    tokio::select! {
        result = &mut future => panic!("reply must be held: {result:?}"),
        _ = proxy.captured.recv() => {}
    }
    assert_eq!(fixture.waiting("cancel-pending").await, 1);
    drop(future);
    proxy.resume.notify_one();
    drain(writer).await;
    assert_eq!(fixture.waiting("cancel-pending").await, 0);
    proxy.finish().await;
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_fast_acquire_ignores_full_wait_queue_and_cleans_on_drop() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let waiters = join_all((0..100).map(|_| fixture.wait("fast", Duration::from_secs(10)))).await;
    assert!(matches!(
        fixture
            .port
            .try_acquire_wait(wait_request(
                "fast",
                AccountWaitMode::Fallback,
                100,
                Duration::from_secs(10)
            ))
            .await
            .expect("full wait queue"),
        ProviderWaitLeaseAcquisition::Full
    ));
    let executing = fast_acquired(
        fixture
            .port
            .try_acquire_scheduling(scheduling(
                "fast",
                1,
                Duration::ZERO,
                Duration::from_secs(10),
            ))
            .await
            .expect("fast acquisition independent of wait count"),
    );
    assert_eq!(fixture.waiting("fast").await, 100);
    assert_eq!(fixture.in_flight("fast").await, 1);
    assert!(matches!(
        fixture
            .port
            .try_acquire_scheduling(scheduling(
                "fast",
                1,
                Duration::ZERO,
                Duration::from_secs(10)
            ))
            .await
            .expect("fast capacity guard"),
        ProviderLeaseAcquisition::Busy { .. }
    ));
    drop(executing);
    for waiter in waiters {
        waiter.release().await.expect("normal wait release");
    }
    fixture.drain().await;
    assert_eq!(fixture.in_flight("fast").await, 0);
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_cancelled_promotion_cleans_committed_slot_and_cannot_retry() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let mut proxy = FaultProxy::start(Fault::HoldReply, 5).await;
    let repository =
        RedisCredentialLeaseRepository::new(proxy.connection().await, &fixture.namespace)
            .expect("proxy repository");
    let (port, writer) = RedisProviderLeaseCoordinator::new(repository);
    let mut waiter = acquired(
        port.try_acquire_wait(wait_request(
            "cancel-promote",
            AccountWaitMode::Sticky,
            3,
            Duration::from_secs(10),
        ))
        .await
        .expect("waiting owner"),
    );
    let mut promotion = waiter.try_promote(scheduling(
        "cancel-promote",
        1,
        Duration::ZERO,
        Duration::from_secs(10),
    ));
    tokio::select! {
        result = &mut promotion => panic!("promotion reply must be held: {result:?}"),
        command = proxy.captured.recv() => { command.expect("committed promotion"); }
    }
    assert_eq!(fixture.waiting("cancel-promote").await, 0);
    assert_eq!(fixture.in_flight("cancel-promote").await, 1);
    drop(promotion);
    proxy.resume.notify_one();
    assert!(matches!(
        waiter
            .try_promote(scheduling(
                "cancel-promote",
                1,
                Duration::ZERO,
                Duration::from_secs(10)
            ))
            .await
            .expect("cancelled promotion is terminal"),
        ProviderWaitPromotion::Expired
    ));
    waiter.release().await.expect("disarmed waiting owner");
    drain(writer).await;
    assert_eq!(fixture.in_flight("cancel-promote").await, 0);
    proxy.finish().await;
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_failed_explicit_release_is_retried_by_tracked_writer() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let mut proxy = FaultProxy::start(Fault::DelayCommand, 3).await;
    let repository =
        RedisCredentialLeaseRepository::new(proxy.connection().await, &fixture.namespace)
            .expect("proxy repository");
    let (port, writer) = RedisProviderLeaseCoordinator::new(repository);
    let waiter = acquired(
        port.try_acquire_wait(wait_request(
            "failed-release",
            AccountWaitMode::Sticky,
            3,
            Duration::from_secs(10),
        ))
        .await
        .expect("wait before failed release"),
    );
    assert!(waiter.release().await.is_err());
    let delayed = proxy.captured.recv().await.expect("failed release command");
    assert_eq!(fixture.waiting("failed-release").await, 1);
    drain(writer).await;
    assert_eq!(fixture.waiting("failed-release").await, 0);
    replay(&mut fixture.connection, &delayed).await;
    assert_eq!(fixture.waiting("failed-release").await, 0);
    proxy.finish().await;
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_fast_acquire_cancellation_before_send_leaves_no_state() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    drop(fixture.port.try_acquire_scheduling(scheduling(
        "unpolled-fast",
        1,
        Duration::ZERO,
        Duration::from_secs(10),
    )));
    fixture.drain().await;
    assert_eq!(fixture.in_flight("unpolled-fast").await, 0);
    assert_eq!(fixture.waiting("unpolled-fast").await, 0);
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_fast_lost_reply_is_owned_and_late_write_is_fenced() {
    for fault in [Fault::LoseReply, Fault::DelayCommand, Fault::HoldReply] {
        let Some(mut fixture) = Fixture::new().await else {
            return;
        };
        let warm = fast_acquired(
            fixture
                .port
                .try_acquire_scheduling(scheduling(
                    "warm-fast",
                    1,
                    Duration::ZERO,
                    Duration::from_secs(10),
                ))
                .await
                .expect("load real execution script"),
        );
        drop(warm);
        let mut proxy = FaultProxy::start(fault, 5).await;
        let repository =
            RedisCredentialLeaseRepository::new(proxy.connection().await, &fixture.namespace)
                .expect("proxy repository");
        let (port, writer) = RedisProviderLeaseCoordinator::new(repository);
        let mut pending = port.try_acquire_scheduling(scheduling(
            "fast-fault",
            1,
            Duration::ZERO,
            Duration::from_secs(10),
        ));
        let captured = if matches!(fault, Fault::HoldReply) {
            let command = tokio::select! {
                result = &mut pending => panic!("fast reply must be held: {result:?}"),
                command = proxy.captured.recv() => command.expect("captured fast write"),
            };
            drop(pending);
            proxy.resume.notify_one();
            command
        } else {
            assert!(pending.await.is_err());
            proxy.captured.recv().await.expect("captured fast command")
        };
        assert_eq!(
            fixture.in_flight("fast-fault").await,
            u32::from(!matches!(fault, Fault::DelayCommand))
        );
        drain(writer).await;
        assert_eq!(fixture.in_flight("fast-fault").await, 0);
        let redis::Value::Array(result) = replay(&mut fixture.connection, &captured).await else {
            panic!("execution reply");
        };
        assert_eq!(result[0], redis::Value::Int(-1));
        assert_eq!(fixture.in_flight("fast-fault").await, 0);
        assert_eq!(fixture.waiting("fast-fault").await, 0);
        proxy.finish().await;
        fixture.finish().await;
    }
}

#[tokio::test]
async fn capacity_wait_running_writer_cleans_promptly_and_retries_redis_disconnect() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let mut proxy = FaultProxy::start(Fault::DelayCommand, 3).await;
    let repository =
        RedisCredentialLeaseRepository::new(proxy.connection().await, &fixture.namespace)
            .expect("proxy repository");
    let (port, writer) = RedisProviderLeaseCoordinator::new(repository);
    let waiter = acquired(
        port.try_acquire_wait(wait_request(
            "cleanup-retry",
            AccountWaitMode::Sticky,
            3,
            Duration::from_secs(10),
        ))
        .await
        .expect("wait before cleanup fault"),
    );
    let cancellation = CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let worker = tokio::spawn(async move { writer.run(worker_cancellation).await });
    let started = tokio::time::Instant::now();
    drop(waiter);
    tokio::time::timeout(Duration::from_secs(4), async {
        while fixture.waiting("cleanup-retry").await != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("live worker retries before original deadline");
    assert!(started.elapsed() < Duration::from_secs(4));
    let captured_cancel = proxy
        .captured
        .recv()
        .await
        .expect("faulted cleanup command");
    let fresh = fixture.wait("cleanup-retry", Duration::from_secs(10)).await;
    replay(&mut fixture.connection, &captured_cancel).await;
    replay(&mut fixture.connection, &captured_cancel).await;
    assert_eq!(fixture.waiting("cleanup-retry").await, 1);
    fresh.release().await.expect("new owner remains intact");
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(6), worker)
        .await
        .expect("worker stops")
        .expect("worker joins")
        .expect("worker drained");
    proxy.finish().await;
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_shutdown_keeps_receiving_drops_after_a_long_http_drain() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let waiting = fixture.wait("late-wait", Duration::from_secs(120)).await;
    let execution = fast_acquired(
        fixture
            .port
            .try_acquire_scheduling(scheduling(
                "late-execution",
                1,
                Duration::ZERO,
                Duration::from_secs(600),
            ))
            .await
            .expect("execution before shutdown"),
    );
    let mut promotion_wait = fixture
        .wait("late-promoted", Duration::from_secs(120))
        .await;
    let promoted_execution = promoted(
        promotion_wait
            .try_promote(scheduling(
                "late-promoted",
                1,
                Duration::ZERO,
                Duration::from_secs(600),
            ))
            .await
            .expect("promotion before shutdown"),
    );
    promotion_wait
        .release()
        .await
        .expect("promotion wait disarmed");

    let writer = fixture.writer.take().expect("cleanup writer");
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let worker = tokio::spawn(async move { writer.run(cancellation).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !worker.is_finished(),
        "live ownership keeps cleanup receiver open"
    );
    assert!(
        fixture
            .port
            .try_acquire_wait(wait_request(
                "no-new-wait",
                AccountWaitMode::Sticky,
                3,
                Duration::from_secs(10),
            ))
            .await
            .is_err()
    );
    assert!(
        fixture
            .port
            .try_acquire_scheduling(scheduling(
                "no-new-execution",
                1,
                Duration::ZERO,
                Duration::from_secs(10),
            ))
            .await
            .is_err()
    );
    // A configured HTTP drain may outlive the former Store-only 35-second cutoff.
    tokio::time::sleep(Duration::from_secs(36)).await;
    assert!(
        !worker.is_finished(),
        "only the outer supervisor may terminate cleanup with live owners"
    );
    assert_eq!(fixture.waiting("late-wait").await, 1);
    assert_eq!(fixture.in_flight("late-execution").await, 1);
    assert_eq!(fixture.in_flight("late-promoted").await, 1);
    let started = tokio::time::Instant::now();
    drop(waiting);
    drop(execution);
    drop(promoted_execution);
    tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .expect("late cleanup completes promptly")
        .expect("join shutdown worker")
        .expect("shutdown drain");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(fixture.waiting("late-wait").await, 0);
    assert_eq!(fixture.in_flight("late-execution").await, 0);
    assert_eq!(fixture.in_flight("late-promoted").await, 0);
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_unreturned_owner_requires_outer_shutdown_timeout_and_abort() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let waiting = fixture
        .wait("never-returned", Duration::from_secs(60))
        .await;
    let writer = fixture.writer.take().expect("cleanup writer");
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let mut worker = tokio::spawn(async move { writer.run(cancellation).await });
    // This is a caller-owned deadline, not a standalone daemon shutdown promise.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut worker)
            .await
            .is_err()
    );
    worker.abort();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .expect("outer shutdown joins the aborted worker")
            .expect_err("worker was aborted by its caller")
            .is_cancelled()
    );
    assert_eq!(fixture.waiting("never-returned").await, 1);
    waiting
        .release()
        .await
        .expect("explicit release remains available after outer shutdown abort");
    fixture.finish().await;
}

#[tokio::test]
async fn capacity_wait_cleanup_queue_is_bounded_and_overflow_retains_only_original_ttl() {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let deadline = SystemTime::now() + Duration::from_secs(15);
    // Fill the cleanup queue without flooding Redis's independent response timeout.
    let mut waiting = Vec::with_capacity(4097);
    for _ in 0..4097 {
        let request = ProviderWaitLeaseRequest::new(
            provider(),
            account("overflow"),
            ModelRequestId::new(format!("req_{}", Uuid::new_v4())).expect("request"),
            AccountWaitMode::Fallback,
            NonZeroU32::new(5000).expect("test queue limit"),
            deadline,
        );
        waiting.push(acquired(
            fixture
                .port
                .try_acquire_wait(request)
                .await
                .expect("enqueue overflow fixture"),
        ));
    }
    drop(waiting);
    assert_eq!(fixture.waiting("overflow").await, 4097);
    fixture.drain().await;
    assert_eq!(
        fixture.waiting("overflow").await,
        1,
        "4096 accepted cleanup commands"
    );
    let expires_at: u64 = redis::cmd("PEXPIRETIME")
        .arg(fixture.waiting_key("overflow"))
        .query_async(&mut fixture.connection)
        .await
        .expect("overflow queue expiry");
    assert_eq!(
        expires_at,
        u64::try_from(deadline.duration_since(UNIX_EPOCH).unwrap().as_millis()).unwrap(),
        "overflow must not extend the original waiting deadline"
    );
    let remaining = deadline
        .duration_since(SystemTime::now())
        .unwrap_or_default();
    tokio::time::sleep(remaining + Duration::from_millis(30)).await;
    assert_eq!(fixture.waiting("overflow").await, 0);
    fixture.finish().await;
}

async fn fault_scenario(fault: Fault, promotion: bool) {
    let Some(mut fixture) = Fixture::new().await else {
        return;
    };
    let mut warm = fixture.wait("warm-script", Duration::from_secs(10)).await;
    if promotion {
        drop(promoted(
            warm.try_promote(scheduling(
                "warm-script",
                1,
                Duration::ZERO,
                Duration::from_secs(30),
            ))
            .await
            .expect("warm promotion script"),
        ));
    }
    warm.release().await.expect("warm script cleanup");
    let mut proxy = FaultProxy::start(fault, if promotion { 5 } else { 2 }).await;
    let connection = proxy.connection().await;
    let repository = RedisCredentialLeaseRepository::new(connection, &fixture.namespace)
        .expect("proxy repository");
    let (port, writer) = RedisProviderLeaseCoordinator::new(repository);
    if promotion {
        let mut waiter = acquired(
            port.try_acquire_wait(wait_request(
                "fault",
                AccountWaitMode::Sticky,
                3,
                Duration::from_secs(10),
            ))
            .await
            .expect("wait before promotion fault"),
        );
        assert!(
            waiter
                .try_promote(scheduling(
                    "fault",
                    1,
                    Duration::ZERO,
                    Duration::from_secs(30)
                ))
                .await
                .is_err()
        );
        assert_eq!(
            fixture.in_flight("fault").await,
            u32::from(matches!(fault, Fault::LoseReply))
        );
        drop(waiter);
    } else {
        assert!(
            port.try_acquire_wait(wait_request(
                "fault",
                AccountWaitMode::Sticky,
                3,
                Duration::from_secs(10),
            ))
            .await
            .is_err()
        );
        assert_eq!(
            fixture.waiting("fault").await,
            u64::from(matches!(fault, Fault::LoseReply))
        );
    }
    let captured = proxy.captured.recv().await.expect("captured real command");
    drain(writer).await;
    assert_eq!(fixture.waiting("fault").await, 0);
    assert_eq!(fixture.in_flight("fault").await, 0);

    // Replaying the exact late bytes also checks a repeated cancellation cannot
    // resurrect ownership or remove a fresh request on the same account.
    let fresh = fixture.wait("fault", Duration::from_secs(10)).await;
    let result = replay(&mut fixture.connection, &captured).await;
    match result {
        redis::Value::Int(-1) => assert!(!promotion),
        redis::Value::Array(values) => assert_eq!(values[0], redis::Value::Int(-1)),
        other => panic!("late operation must be fenced: {other:?}"),
    }
    assert_eq!(fixture.waiting("fault").await, 1);
    assert_eq!(fixture.in_flight("fault").await, 0);
    fresh.release().await.expect("fresh owner release");
    proxy.finish().await;
    fixture.finish().await;
}

struct Fixture {
    repository: RedisCredentialLeaseRepository,
    port: RedisProviderLeaseCoordinator,
    writer: Option<CapacityWaitCleanupWriter>,
    connection: ConnectionManager,
    namespace: String,
}

impl Fixture {
    async fn new() -> Option<Self> {
        let url = crate::support::test_env("CPR_TEST_REDIS_URL")?;
        let connection = redis::Client::open(url)
            .expect("test URL")
            .get_connection_manager()
            .await
            .expect("isolated Redis");
        let namespace = format!("capacity-wait-test-{}", Uuid::new_v4());
        let repository = RedisCredentialLeaseRepository::new(connection.clone(), &namespace)
            .expect("repository");
        let (port, writer) = RedisProviderLeaseCoordinator::new(repository.clone());
        Some(Self {
            repository,
            port,
            writer: Some(writer),
            connection,
            namespace,
        })
    }

    async fn wait(&self, account: &str, ttl: Duration) -> Box<dyn ProviderWaitLease> {
        acquired(
            self.port
                .try_acquire_wait(wait_request(account, AccountWaitMode::Fallback, 100, ttl))
                .await
                .expect("acquire waiting token"),
        )
    }

    fn active_key(&self, account: &str) -> String {
        let fingerprint = hex::encode(Sha256::digest(format!("acct_{account}").as_bytes()));
        format!("{}:lease:account:{{{fingerprint}}}:active", self.namespace)
    }

    fn waiting_key(&self, account: &str) -> String {
        format!("{}:waiting", self.active_key(account))
    }

    async fn waiting(&mut self, account: &str) -> u64 {
        redis::cmd("ZCARD")
            .arg(self.waiting_key(account))
            .query_async(&mut self.connection)
            .await
            .expect("waiting count")
    }

    async fn in_flight(&self, account: &str) -> u32 {
        self.repository
            .credential_runtime_signals(&[format!("acct_{account}")])
            .await
            .expect("execution count")[0]
            .in_flight
    }

    async fn drain(&mut self) {
        if let Some(writer) = self.writer.take() {
            drain(writer).await;
        }
    }

    async fn finish(mut self) {
        self.drain().await;
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(format!("{}:*", self.namespace))
            .query_async(&mut self.connection)
            .await
            .expect("isolated namespace keys");
        if !keys.is_empty() {
            redis::cmd("DEL")
                .arg(keys)
                .query_async::<i64>(&mut self.connection)
                .await
                .expect("remove only this test namespace");
        }
    }
}

async fn drain(writer: CapacityWaitCleanupWriter) {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(6), writer.run(cancellation))
        .await
        .expect("bounded cleanup drain")
        .expect("cleanup worker");
}

fn provider() -> ProviderKind {
    ProviderKind::new("openai").expect("provider")
}
fn account(value: &str) -> ProviderAccountId {
    ProviderAccountId::new(format!("acct_{value}")).expect("account")
}

fn wait_request(
    account_id: &str,
    mode: AccountWaitMode,
    cap: u32,
    ttl: Duration,
) -> ProviderWaitLeaseRequest {
    ProviderWaitLeaseRequest::new(
        provider(),
        account(account_id),
        ModelRequestId::new(format!("req_{}", Uuid::new_v4())).expect("request"),
        mode,
        NonZeroU32::new(cap).expect("wait cap"),
        SystemTime::now() + ttl,
    )
}

fn scheduling(
    account_id: &str,
    cap: u32,
    interval: Duration,
    ttl: Duration,
) -> ProviderSchedulingLeaseRequest {
    ProviderSchedulingLeaseRequest::new(
        provider(),
        account(account_id),
        CredentialRevision::new(1).expect("revision"),
        NonZeroU32::new(cap).expect("execution cap"),
        interval,
        SystemTime::now() + ttl,
    )
}

fn acquired(outcome: ProviderWaitLeaseAcquisition) -> Box<dyn ProviderWaitLease> {
    match outcome {
        ProviderWaitLeaseAcquisition::Acquired(guard) => guard,
        ProviderWaitLeaseAcquisition::Full => panic!("waiting queue unexpectedly full"),
    }
}

fn promoted(outcome: ProviderWaitPromotion) -> Box<dyn ProviderLeaseGuard> {
    match outcome {
        ProviderWaitPromotion::Acquired(guard) => guard,
        other => panic!("expected execution slot: {other:?}"),
    }
}

fn fast_acquired(outcome: ProviderLeaseAcquisition) -> Box<dyn ProviderLeaseGuard> {
    match outcome {
        ProviderLeaseAcquisition::Acquired(guard) => guard,
        other => panic!("expected fast execution slot: {other:?}"),
    }
}

#[derive(Clone, Copy)]
enum Fault {
    LoseReply,
    DelayCommand,
    HoldReply,
}

struct FaultProxy {
    url: String,
    captured: mpsc::UnboundedReceiver<Vec<u8>>,
    resume: Arc<Notify>,
    shutdown: Arc<Notify>,
    task: JoinHandle<()>,
}

impl FaultProxy {
    async fn start(fault: Fault, key_count: u8) -> Self {
        let mut url =
            url::Url::parse(&std::env::var("CPR_TEST_REDIS_URL").expect("real Redis required"))
                .expect("test URL");
        assert!(matches!(
            url.host_str(),
            Some("127.0.0.1" | "localhost" | "::1")
        ));
        let upstream = format!(
            "{}:{}",
            url.host_str().expect("host"),
            url.port().unwrap_or(6379)
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("fault proxy");
        url.set_host(Some("127.0.0.1")).expect("proxy host");
        url.set_port(Some(listener.local_addr().expect("proxy addr").port()))
            .expect("proxy port");
        let (sender, captured) = mpsc::unbounded_channel();
        let resume = Arc::new(Notify::new());
        let shutdown = Arc::new(Notify::new());
        let stop = Arc::clone(&shutdown);
        let proceed = Arc::clone(&resume);
        let task = tokio::spawn(async move {
            let fired = Arc::new(AtomicBool::new(false));
            let mut sessions = JoinSet::new();
            loop {
                tokio::select! {
                    () = stop.notified() => break,
                    Some(result) = sessions.join_next(), if !sessions.is_empty() => {
                        result.expect("proxy connection task");
                    }
                    accepted = listener.accept() => {
                        let (client, _) = accepted.expect("proxy accept");
                        let upstream = upstream.clone();
                        let fired = Arc::clone(&fired);
                        let sender = sender.clone();
                        let proceed = Arc::clone(&proceed);
                        sessions.spawn(async move {
                            proxy_connection(client, &upstream, fault, key_count, fired, sender, proceed)
                                .await;
                        });
                    }
                }
            }
            sessions.abort_all();
            while sessions.join_next().await.is_some() {}
        });
        Self {
            url: url.into(),
            captured,
            resume,
            shutdown,
            task,
        }
    }

    async fn connection(&self) -> ConnectionManager {
        redis::Client::open(self.url.as_str())
            .expect("proxy URL")
            .get_connection_manager_with_config(
                redis::aio::ConnectionManagerConfig::new()
                    .set_connection_timeout(Some(Duration::from_secs(2)))
                    .set_response_timeout(Some(Duration::from_secs(2))),
            )
            .await
            .expect("proxy Redis connection")
    }

    async fn finish(self) {
        self.shutdown.notify_one();
        self.task.await.expect("join fault proxy");
    }
}

async fn proxy_connection(
    mut client: TcpStream,
    upstream: &str,
    fault: Fault,
    key_count: u8,
    fired: Arc<AtomicBool>,
    sender: mpsc::UnboundedSender<Vec<u8>>,
    resume: Arc<Notify>,
) {
    let mut server = TcpStream::connect(upstream)
        .await
        .expect("real isolated Redis");
    while let Some(command) = frame(&mut client).await {
        let args = command_args(&command);
        let target = args
            .first()
            .is_some_and(|value| value.eq_ignore_ascii_case(b"EVALSHA"))
            && args
                .get(2)
                .is_some_and(|value| *value == key_count.to_string().as_bytes())
            && !fired.swap(true, Ordering::SeqCst);
        if target && matches!(fault, Fault::DelayCommand) {
            sender.send(command).expect("capture delayed command");
            return;
        }
        if server.write_all(&command).await.is_err() {
            return;
        }
        let Some(reply) = frame(&mut server).await else {
            return;
        };
        // A cache miss is not a committed Lua write; allow Script's load/retry.
        if target && reply.starts_with(b"-NOSCRIPT") {
            fired.store(false, Ordering::SeqCst);
        } else if target {
            sender.send(command).expect("capture committed command");
            match fault {
                Fault::LoseReply => return,
                Fault::HoldReply => resume.notified().await,
                Fault::DelayCommand => unreachable!(),
            }
        }
        if client.write_all(&reply).await.is_err() {
            return;
        }
    }
}

async fn frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let byte = stream.read_u8().await.ok()?;
        bytes.push(byte);
        if byte == b'\n' && redis::parse_redis_value(&bytes).is_ok() {
            return Some(bytes);
        }
        assert!(bytes.len() < 1_048_576, "bounded test RESP frame");
    }
}

fn command_args(command: &[u8]) -> Vec<Vec<u8>> {
    redis::from_redis_value(redis::parse_redis_value(command).expect("RESP command"))
        .expect("command arguments")
}

async fn replay(connection: &mut ConnectionManager, bytes: &[u8]) -> redis::Value {
    let args = command_args(bytes);
    let mut command = redis::cmd(std::str::from_utf8(&args[0]).expect("command name"));
    for arg in &args[1..] {
        command.arg(arg);
    }
    command
        .query_async(connection)
        .await
        .expect("replay real delayed command")
}
