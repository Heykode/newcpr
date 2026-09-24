use std::{num::NonZeroU32, time::Duration};

use gateway_core::{routing::ConfigRevision, runtime::AccountConcurrencyHandle};
use tokio::{io::DuplexStream, time::timeout};
use tokio_tungstenite::{WebSocketStream, tungstenite::protocol::Role};

use super::*;
use crate::transport::websocket::pump::{PumpLogContext, PumpedWebSocket};

fn pool(limit: u32) -> (CodexWebSocketPool, AccountConcurrencyHandle) {
    let limits = AccountConcurrencyHandle::default();
    publish(&limits, 1, limit);
    let pool =
        CodexWebSocketPool::new(Duration::from_secs(60)).with_account_concurrency(limits.clone());
    (pool, limits)
}

fn publish(handle: &AccountConcurrencyHandle, revision: u64, limit: u32) {
    assert!(handle.publish(
        ConfigRevision::new(revision).unwrap(),
        [("account".to_owned(), NonZeroU32::new(limit).unwrap())].into(),
    ));
}

fn key(index: usize) -> CodexWebSocketPoolKey {
    CodexWebSocketPoolKey::new(
        "https://upstream.invalid",
        "account",
        format!("session-{index}"),
    )
}

#[test]
fn client_scope_participates_in_key_equality_hash_and_diagnostics() {
    use std::hash::{DefaultHasher, Hash as _, Hasher as _};

    let hash = |key: &CodexWebSocketPoolKey| {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    };
    let legacy = key(1);
    let empty = key(1).with_client_scope("");
    let first = key(1).with_client_scope("client-key-a");
    let same = key(1).with_client_scope("client-key-a");
    let other = key(1).with_client_scope("client-key-b");
    assert_eq!(legacy, empty);
    assert_eq!(hash(&legacy), hash(&empty));
    assert_eq!(legacy.stable_hash(), empty.stable_hash());
    assert_eq!(first, same);
    assert_eq!(hash(&first), hash(&same));
    assert_eq!(first.stable_hash(), same.stable_hash());
    for different in [&other, &empty] {
        assert_ne!(&first, different);
        assert_ne!(hash(&first), hash(different));
        assert_ne!(first.stable_hash(), different.stable_hash());
    }
    assert_eq!(first.clone().with_client_scope("client-key-a"), first);
    assert_eq!(first.clone().with_client_scope(""), legacy);
    assert!(!format!("{first:?}").contains("client-key-a"));
}

#[test]
fn client_scope_fences_routing_owner_and_logical_connection_matches() {
    let owner = key(1)
        .with_client_scope("client-key-a")
        .with_connection_profile("old-profile")
        .with_downstream_connection_id("old-downstream")
        .with_egress_key("proxy");
    let reconnected = key(1)
        .with_client_scope("client-key-a")
        .with_connection_profile("new-profile")
        .with_downstream_connection_id("new-downstream")
        .with_egress_key("proxy");
    assert_ne!(owner, reconnected);
    assert!(owner.matches_routing_owner(&reconnected));
    assert!(owner.same_logical_connection(&reconnected));
    for scope in ["client-key-b", ""] {
        let other = reconnected.clone().with_client_scope(scope);
        assert!(!owner.matches_routing_owner(&other));
        assert!(!other.matches_routing_owner(&owner));
        assert!(!owner.same_logical_connection(&other));
        assert!(!other.same_logical_connection(&owner));
    }
}

#[test]
fn managed_model_versions_isolate_new_chains_without_changing_exact_owner_scope() {
    let legacy = key(1).with_client_scope("client-a");
    let first = legacy.clone().with_model_state("model-a", 1);
    let same = legacy.clone().with_model_state("model-a", 1);
    let updated = legacy.clone().with_model_state("model-a", 2);
    let other_model = legacy.clone().with_model_state("model-b", 1);
    assert_eq!(first, same);
    for other in [&legacy, &updated, &other_model] {
        assert_ne!(&first, other);
        assert_ne!(first.stable_hash(), other.stable_hash());
    }
    assert!(first.matches_routing_owner(&updated));
    assert!(first.matches_routing_owner(&legacy));
    assert!(first.same_logical_connection(&updated));
    assert!(!first.matches_routing_owner(&updated.with_client_scope("client-b")));
}

#[tokio::test]
async fn managed_state_update_and_disable_preserve_the_exact_response_socket() {
    let (pool, _) = pool(2);
    let owner = key(1).with_model_state("model-a", 1);
    let (mut connection, lease, _peer) = connected(&pool, &owner).await;
    let connection_id = connection.websocket.connection_id();
    connection
        .continuation
        .record_completed("response-owned".to_owned());
    lease.put(connection).await;
    for next in [key(1).with_model_state("model-a", 2), key(1)] {
        assert_eq!(
            pool.routing_owner(&next, Some("response-owned")),
            Some(owner.clone()),
        );
        match pool.acquire(&next, Some("response-owned")).await {
            WebSocketPoolAcquire::Reused { connection, lease } => {
                assert_eq!(connection.websocket.connection_id(), connection_id);
                lease.put(*connection).await;
            }
            _ => panic!("continuation must retain its physical owner"),
        }
    }
    pool.shutdown().await;
}

#[tokio::test]
async fn expired_managed_pool_rejects_new_chains_but_keeps_exact_continuation() {
    let (pool, _) = pool(2);
    let owner = key(1)
        .with_model_state("model-a", 1)
        .with_state_expiry(Some(
            std::time::SystemTime::now() + Duration::from_secs(3600),
        ));
    let (mut connection, lease, _peer) = connected(&pool, &owner).await;
    let connection_id = connection.websocket.connection_id();
    connection
        .continuation
        .record_completed("response-owned".to_owned());
    pool.retire_managed_state("account", "model-a", 2);
    assert!(
        !connection.websocket.is_closed(),
        "retirement must not interrupt a response"
    );
    lease.put(connection).await;
    pool.maintain_idle_connections().await;
    assert!(pool.managed_version_retired("account", "model-a", 1));
    assert!(!pool.managed_version_retired("account", "model-b", 1));
    assert!(!pool.managed_version_retired("other-account", "model-a", 1));
    assert!(matches!(
        pool.acquire(&owner, None).await,
        WebSocketPoolAcquire::Bypass(WebSocketPoolBypassReason::Disabled)
    ));
    match pool.acquire(&key(1), Some("response-owned")).await {
        WebSocketPoolAcquire::Reused { connection, lease } => {
            assert_eq!(connection.websocket.connection_id(), connection_id);
            lease.put(*connection).await;
        }
        _ => panic!("expired state must preserve exact continuation ownership"),
    }
    assert!(pool.managed_version_retired("account", "model-a", 1));
    pool.shutdown().await;
}

#[tokio::test]
async fn same_state_renewal_preserves_socket_identity_and_cannot_revive_retired_generations() {
    let (pool, _) = pool(2);
    let now = std::time::SystemTime::now();
    let owner = key(1)
        .with_model_state("model-a", 1)
        .with_state_expiry(Some(now + Duration::from_secs(60)));
    let (connection, lease, _peer) = connected(&pool, &owner).await;
    let connection_id = connection.websocket.connection_id();
    pool.renew_managed_state("account", "model-a", 1, now + Duration::from_secs(180));
    pool.renew_managed_state("account", "model-a", 1, now + Duration::from_secs(90));
    lease.put(connection).await;
    {
        let state = pool.lock_state();
        let (stored, _) = state.slots.get_key_value(&owner).unwrap();
        assert_eq!(stored.stable_hash(), owner.stable_hash());
        assert_eq!(
            stored.turn_state_expires_at,
            Some(now + Duration::from_secs(180))
        );
    }
    match pool.acquire(&owner, None).await {
        WebSocketPoolAcquire::Reused { connection, lease } => {
            assert_eq!(connection.websocket.connection_id(), connection_id);
            // Retire a generation even if its old local lease has already elapsed.
            {
                let mut state = pool.lock_state();
                let slot = state.slots.remove(&owner).unwrap();
                state.slots.insert(
                    owner
                        .clone()
                        .with_state_expiry(Some(now - Duration::from_secs(1))),
                    slot,
                );
            }
            pool.retire_managed_state("account", "model-a", 2);
            pool.renew_managed_state("account", "model-a", 1, now + Duration::from_secs(240));
            assert!(pool.managed_version_retired("account", "model-a", 1));
            lease.put(*connection).await;
        }
        _ => panic!("renewal must preserve the physical socket"),
    }
    pool.shutdown().await;
}

#[tokio::test]
async fn retired_managed_socket_without_continuation_closes_on_return() {
    let (pool, _) = pool(2);
    let owner = key(1).with_model_state("model-a", 1);
    let (connection, lease, _peer) = connected(&pool, &owner).await;
    let termination = connection.websocket.termination_handle();
    pool.retire_managed_state("account", "model-a", 2);
    assert!(!connection.websocket.is_closed());
    lease.put(connection).await;
    timeout(Duration::from_secs(1), termination.wait())
        .await
        .unwrap();
    pool.shutdown().await;
}

#[tokio::test]
async fn ttl_expiry_closes_idle_socket_without_response_owner() {
    let (pool, _) = pool(2);
    let owner = key(1)
        .with_model_state("model-a", 1)
        .with_state_expiry(Some(
            std::time::SystemTime::now() + Duration::from_secs(3600),
        ));
    let (connection, lease, _peer) = connected(&pool, &owner).await;
    let termination = connection.websocket.termination_handle();
    lease.put(connection).await;
    {
        let mut state = pool.lock_state();
        let slot = state.slots.remove(&owner).unwrap();
        state
            .slots
            .insert(owner.with_state_expiry(Some(std::time::UNIX_EPOCH)), slot);
    }
    pool.maintain_idle_connections().await;
    timeout(Duration::from_secs(1), termination.wait())
        .await
        .unwrap();
    pool.shutdown().await;
}

#[tokio::test]
async fn client_scope_isolates_live_sockets_but_keeps_same_client_exact_owner() {
    let (pool, _) = pool(2);
    let owner = key(1)
        .with_client_scope("client-key-a")
        .with_downstream_connection_id("old-downstream");
    let (mut connection, lease, _peer) = connected(&pool, &owner).await;
    let connection_id = connection.websocket.connection_id();
    connection
        .continuation
        .record_completed("response-shared".to_owned());
    lease.put(connection).await;
    let other = owner.clone().with_client_scope("client-key-b");
    assert!(
        pool.routing_owner(&other, Some("response-shared"))
            .is_none()
    );
    assert!(matches!(
        pool.acquire(&other, Some("response-shared")).await,
        WebSocketPoolAcquire::Bypass(WebSocketPoolBypassReason::ContinuationNotFound)
    ));
    let other_opening = opening(&pool, &other);
    assert_eq!(pool.lock_state().slots.len(), 2);
    drop(other_opening);

    let reconnected = owner
        .clone()
        .with_connection_profile("new-profile")
        .with_downstream_connection_id("new-downstream");
    assert_eq!(
        pool.routing_owner(&reconnected, Some("response-shared")),
        Some(owner)
    );
    match pool.acquire(&reconnected, Some("response-shared")).await {
        WebSocketPoolAcquire::Reused { connection, lease } => {
            assert_eq!(connection.websocket.connection_id(), connection_id);
            lease.put(*connection).await;
        }
        _ => panic!("same-client reconnect must retain the exact physical owner"),
    }
    pool.shutdown().await;
}

#[tokio::test]
async fn client_scope_isolates_tombstones_and_preserves_same_client_reconnect_lookup() {
    let (pool, _) = pool(1);
    let owner = key(1)
        .with_client_scope("client-key-a")
        .with_downstream_connection_id("old-downstream");
    let (mut connection, lease, _peer) = connected(&pool, &owner).await;
    let connection_id = connection.websocket.connection_id();
    connection
        .continuation
        .record_completed("response-shared".to_owned());
    lease.put(connection).await;
    let replacement = pool.acquire(&key(2), None).await;
    assert!(matches!(replacement, WebSocketPoolAcquire::Connect(_)));
    drop(replacement);

    let reconnected = owner
        .with_connection_profile("new-profile")
        .with_downstream_connection_id("new-downstream");
    let other = reconnected.clone().with_client_scope("client-key-b");
    assert!(matches!(
        pool.acquire(&other, Some("response-shared")).await,
        WebSocketPoolAcquire::Bypass(WebSocketPoolBypassReason::ContinuationNotFound)
    ));
    let observation = match pool.acquire(&reconnected, Some("response-shared")).await {
        WebSocketPoolAcquire::ContinuationLost(observation) => observation,
        _ => panic!("same-client reconnect must find its eviction tombstone"),
    };
    assert_eq!(observation.connection_id(), connection_id);

    let now = tokio::time::Instant::now();
    {
        let mut state = pool.lock_state();
        state.remember_continuation_loss(
            &other,
            Some("response-shared"),
            observation.with_exit_reason("other-client-loss"),
            now,
        );
        assert_ne!(
            state
                .continuation_loss(&reconnected, "response-shared", now)
                .unwrap()
                .exit_reason(),
            "other-client-loss"
        );
        assert_eq!(
            state
                .continuation_loss(&other, "response-shared", now)
                .unwrap()
                .exit_reason(),
            "other-client-loss"
        );
    }
    pool.shutdown().await;
}

fn opening(pool: &CodexWebSocketPool, key: &CodexWebSocketPoolKey) -> WebSocketPoolConnectLease {
    let (acquire, close) = pool.acquire_once(key, None);
    assert!(close.is_empty());
    match acquire {
        Some(WebSocketPoolAcquire::Connect(lease)) => lease,
        _ => panic!("expected an available account slot"),
    }
}

async fn connected(
    pool: &CodexWebSocketPool,
    key: &CodexWebSocketPoolKey,
) -> (PooledWebSocketConnection, WebSocketPoolLease, DuplexStream) {
    connected_with_buffer(pool, key, 4096).await
}

async fn connected_with_buffer(
    pool: &CodexWebSocketPool,
    key: &CodexWebSocketPoolKey,
    buffer_size: usize,
) -> (PooledWebSocketConnection, WebSocketPoolLease, DuplexStream) {
    let opening = opening(pool, key);
    let (stream, peer) = tokio::io::duplex(buffer_size);
    let socket = WebSocketStream::from_raw_socket(stream, Role::Client, None).await;
    let connection = PooledWebSocketConnection {
        websocket: PumpedWebSocket::new(
            Box::new(socket),
            PumpKeepalive::disabled(),
            PumpLogContext::new(None, None),
            None,
        ),
        metadata: CodexWebSocketConnectionMetadata {
            turn_state: None,
            set_cookie_headers: Vec::new(),
            rate_limit_headers: Vec::new(),
            rate_limit_observed_at: std::time::SystemTime::now(),
            response_metadata: Default::default(),
            diagnostics: Default::default(),
        },
        continuation: WebSocketContinuationState::default(),
        created_at: tokio::time::Instant::now(),
    };
    match opening.connected_reserved(connection).await {
        Ok((connection, lease)) => (*connection, lease, peer),
        Err(_) => panic!("opening must retain its reserved slot"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_capacity_reservation_is_atomic_across_sessions_profiles_and_egress() {
    let (pool, _) = pool(5);
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..64 {
        let pool = pool.clone();
        tasks.spawn(async move {
            let key = key(index)
                .with_connection_profile(format!("profile-{index}"))
                .with_egress_key(&format!("proxy-ipv6-{index}"));
            let (acquire, close) = pool.acquire_once(&key, None);
            assert!(close.is_empty());
            acquire
        });
    }
    let mut leases = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Some(WebSocketPoolAcquire::Connect(lease)) = result.unwrap() {
            leases.push(lease);
        }
    }
    assert_eq!(leases.len(), 5);
    assert_eq!(
        CodexWebSocketPool::account_slots(&pool.lock_state(), "account"),
        5
    );
    drop(leases);
    assert_eq!(
        CodexWebSocketPool::account_slots(&pool.lock_state(), "account"),
        0
    );
}

#[tokio::test]
async fn account_capacity_does_not_impose_a_global_opening_limit() {
    let pool = CodexWebSocketPool::new(Duration::from_secs(60));
    let leases = (0..32)
        .map(|index| {
            opening(
                &pool,
                &CodexWebSocketPoolKey::new(
                    "https://upstream.invalid",
                    format!("account-{index}"),
                    "session",
                ),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(leases.len(), 32);
    assert_eq!(pool.lock_state().slots.len(), 32);
    drop(leases);
}

#[tokio::test(start_paused = true)]
async fn account_capacity_wait_observes_live_increases_and_cancelled_openings() {
    let (pool, limits) = pool(1);
    let first = opening(&pool, &key(1));
    let wait_pool = pool.clone();
    let mut waiting = tokio::spawn(async move { wait_pool.acquire(&key(2), None).await });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    publish(&limits, 2, 2);
    let second = match timeout(Duration::from_secs(1), &mut waiting)
        .await
        .unwrap()
        .unwrap()
    {
        WebSocketPoolAcquire::Connect(lease) => lease,
        _ => panic!("live increase must wake the account waiter"),
    };
    publish(&limits, 3, 1);
    assert!(pool.acquire_once(&key(3), None).0.is_none());
    drop(first);
    assert!(pool.acquire_once(&key(3), None).0.is_none());
    drop(second);
    drop(opening(&pool, &key(3)));
}

#[tokio::test(start_paused = true)]
async fn account_capacity_cancelled_waiter_does_not_leave_a_slot() {
    let (pool, _) = pool(1);
    let first = opening(&pool, &key(1));
    let wait_pool = pool.clone();
    let waiting = tokio::spawn(async move { wait_pool.acquire(&key(2), None).await });
    tokio::task::yield_now().await;
    waiting.abort();
    assert!(matches!(waiting.await, Err(error) if error.is_cancelled()));
    assert_eq!(pool.lock_state().slots.len(), 1);
    drop(first);
    drop(opening(&pool, &key(2)));
}

#[tokio::test]
async fn account_capacity_counts_idle_busy_and_opening_together() {
    let (pool, _) = pool(3);
    let (idle, idle_lease, _idle_peer) = connected(&pool, &key(1)).await;
    idle_lease.put(idle).await;
    let (_busy, busy_lease, _busy_peer) = connected(&pool, &key(2)).await;
    let connecting = opening(&pool, &key(3));
    assert_eq!(pool.lock_state().slots.len(), 3);
    let (decision, close) = pool.acquire_once(&key(4), None);
    assert!(decision.is_none());
    assert_eq!(close.len(), 1);
    // Retiring idle still occupies capacity until the actual close completes.
    assert!(pool.acquire_once(&key(5), None).0.is_none());
    assert_eq!(pool.lock_state().slots.len(), 3);
    pool.close_retired_connections(close).await;
    drop(opening(&pool, &key(4)));
    drop(connecting);
    drop(busy_lease);
    pool.shutdown().await;
}

#[tokio::test]
async fn account_capacity_shrink_keeps_busy_responses_and_retires_only_excess_returns() {
    let (pool, limits) = pool(3);
    let (first, first_lease, _first_peer) = connected(&pool, &key(1)).await;
    let (second, second_lease, _second_peer) = connected(&pool, &key(2)).await;
    let (mut third, third_lease, _third_peer) = connected(&pool, &key(3)).await;
    third
        .continuation
        .record_completed("response-third".to_owned());
    publish(&limits, 2, 1);
    pool.maintain_idle_connections().await;
    assert_eq!(pool.lock_state().slots.len(), 3);
    assert!(!first.websocket.is_closed());
    assert!(!second.websocket.is_closed());
    assert!(!third.websocket.is_closed());
    first_lease.put(first).await;
    second_lease.put(second).await;
    third_lease.put(third).await;
    assert_eq!(pool.lock_state().slots.len(), 1);
    match pool.acquire(&key(3), Some("response-third")).await {
        WebSocketPoolAcquire::Reused { connection, lease } => lease.put(*connection).await,
        _ => panic!("remaining exact continuation must keep its owner"),
    }
    pool.shutdown().await;
}

#[tokio::test]
async fn account_capacity_simultaneous_returns_do_not_retire_more_than_the_excess() {
    let (pool, limits) = pool(3);
    let (first, first_lease, _first_peer) = connected(&pool, &key(1)).await;
    let (second, second_lease, _second_peer) = connected(&pool, &key(2)).await;
    let (third, third_lease, _third_peer) = connected(&pool, &key(3)).await;
    publish(&limits, 2, 1);
    tokio::join!(
        first_lease.put(first),
        second_lease.put(second),
        third_lease.put(third)
    );
    let retained = pool
        .lock_state()
        .slots
        .values()
        .filter(|slot| matches!(slot, WebSocketPoolSlot::Idle { .. }))
        .count();
    assert_eq!(retained, 1);
    pool.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn account_capacity_shrink_resolves_exact_owner_before_trimming_other_idle_sockets() {
    let (pool, limits) = pool(2);
    let (mut first, first_lease, _first_peer) = connected(&pool, &key(1)).await;
    first
        .continuation
        .record_completed("response-first".to_owned());
    first_lease.put(first).await;
    tokio::time::advance(Duration::from_millis(1)).await;
    let (second, second_lease, _second_peer) = connected_with_buffer(&pool, &key(2), 1).await;
    second_lease.put(second).await;
    publish(&limits, 2, 1);
    let reconnected = key(1)
        .with_connection_profile("updated-profile")
        .with_downstream_connection_id("new-downstream-connection");
    // The other socket cannot flush its Close yet. Reusing the exact owner must not wait on it.
    match timeout(
        Duration::from_millis(10),
        pool.acquire(&reconnected, Some("response-first")),
    )
    .await
    {
        Ok(WebSocketPoolAcquire::Reused { connection, lease }) => {
            assert_eq!(
                connection.continuation.latest_response_id(),
                Some("response-first")
            );
            lease.put(*connection).await;
        }
        _ => {
            panic!("exact owner must be resolved before trimming, without waiting for other Close")
        }
    }
    timeout(Duration::from_secs(5), pool.shutdown())
        .await
        .unwrap();
}

#[tokio::test]
async fn account_capacity_reclaimed_continuation_has_a_tombstone() {
    let (pool, _) = pool(1);
    let (mut connection, lease, _peer) = connected(&pool, &key(1)).await;
    connection
        .continuation
        .record_completed("response-1".to_owned());
    lease.put(connection).await;
    let new_opening = pool.acquire(&key(2), None).await;
    assert!(matches!(new_opening, WebSocketPoolAcquire::Connect(_)));
    assert!(matches!(
        pool.acquire(&key(1), Some("response-1")).await,
        WebSocketPoolAcquire::ContinuationLost(_),
    ));
    drop(new_opening);
}

#[tokio::test(start_paused = true)]
async fn account_capacity_maintenance_does_not_erase_busy_or_cancelled_opening_reservations() {
    let (pool, _) = pool(2);
    let (_connection, busy, _peer) = connected(&pool, &key(1)).await;
    let connecting = opening(&pool, &key(2));
    tokio::time::advance(Duration::from_secs(61)).await;
    pool.maintain_idle_connections().await;
    assert_eq!(pool.lock_state().slots.len(), 2);
    assert!(connecting.cancellation_token().is_cancelled());
    assert!(pool.acquire_once(&key(3), None).0.is_none());
    drop(connecting);
    drop(busy);
    timeout(Duration::from_secs(1), async {
        while !pool.lock_state().slots.is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("cancelled busy socket must finish releasing its capacity");
}

#[tokio::test]
async fn account_capacity_uses_stale_snapshot_but_missing_snapshots_fail_closed() {
    let (pool, limits) = pool(1);
    limits.suspend();
    let acquired = pool.acquire(&key(1), None).await;
    assert!(!matches!(acquired, WebSocketPoolAcquire::LocalEgress(_)));
    drop(acquired);

    let empty =
        CodexWebSocketPool::default().with_account_concurrency(AccountConcurrencyHandle::default());
    assert!(matches!(
        empty.acquire(&key(1), None).await,
        WebSocketPoolAcquire::LocalEgress(_)
    ));

    let suspended = AccountConcurrencyHandle::default();
    suspended.suspend();
    let suspended_pool = CodexWebSocketPool::default().with_account_concurrency(suspended);
    assert!(matches!(
        suspended_pool.acquire(&key(1), None).await,
        WebSocketPoolAcquire::LocalEgress(_)
    ));

    limits.publish(ConfigRevision::new(2).unwrap(), Default::default());
    assert!(matches!(
        pool.acquire(&key(1), None).await,
        WebSocketPoolAcquire::LocalEgress(_)
    ));
    assert!(pool.lock_state().slots.is_empty());
    pool.shutdown().await;
    empty.shutdown().await;
    suspended_pool.shutdown().await;
}
