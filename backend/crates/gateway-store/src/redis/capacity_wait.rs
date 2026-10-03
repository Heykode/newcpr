//! Account waiting ownership, atomic promotion and bounded cancellation cleanup.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use gateway_core::account::ProviderAccountId;
use gateway_core::lifecycle::{CancellationToken, Deadline};
use gateway_core::provider_ports::{
    ProviderLeaseAcquisition, ProviderSchedulingLeaseRequest, ProviderStoreError,
    ProviderStoreErrorKind, ProviderWaitLease, ProviderWaitLeaseAcquisition,
    ProviderWaitLeaseRequest, ProviderWaitPromotion,
};
use gateway_core::task::{DaemonTask, WorkerTaskError};
use redis::{Script, aio::ConnectionManager};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    CredentialLeaseRequest, CredentialLeaseScope, MAX_REDIS_EXACT_INTEGER,
    RedisCredentialLeaseRepository, resource_fingerprint,
};

const QUEUE_CAPACITY: usize = 4_096;
const CLEANUP_CONCURRENCY: usize = 32;
const IO_TIMEOUT: Duration = Duration::from_millis(250);
const RETRY_DELAY: Duration = Duration::from_millis(100);
const SIGNAL_TTL_MILLIS: u64 = 86_400_000;

const RENEW_EXECUTION_SCRIPT: &str = r#"
local clock = redis.call('TIME')
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local expires = tonumber(redis.call('ZSCORE', KEYS[1], ARGV[1]) or '0')
if expires <= now or redis.call('EXISTS', KEYS[2]) == 1 then return 0 end
local ttl = tonumber(ARGV[2])
redis.call('ZADD', KEYS[1], 'XX', now + ttl, ARGV[1])
if redis.call('PTTL', KEYS[1]) < ttl then redis.call('PEXPIRE', KEYS[1], ttl) end
return 1
"#;

// All keys have the existing execution account hash tag. Both modes use KEYS[1].
const ENQUEUE_SCRIPT: &str = r#"
local clock = redis.call('TIME')
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local deadline = tonumber(ARGV[2])
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now)
redis.call('ZREMRANGEBYSCORE', KEYS[3], '-inf', now)
if deadline <= now or redis.call('EXISTS', KEYS[2]) == 1 then
  return -1
end
local existing = redis.call('ZSCORE', KEYS[1], ARGV[1])
if existing then
  return 1
end
if redis.call('ZCARD', KEYS[1]) >= tonumber(ARGV[3]) then
  return 0
end
redis.call('ZADD', KEYS[1], deadline, ARGV[1])
if ARGV[4] == '1' then
  redis.call('ZADD', KEYS[3], deadline, ARGV[1])
  if redis.call('PTTL', KEYS[3]) < deadline - now then
    redis.call('PEXPIREAT', KEYS[3], deadline)
  end
end
if redis.call('PTTL', KEYS[1]) < deadline - now then
  redis.call('PEXPIREAT', KEYS[1], deadline)
end
return 1
"#;

const EXECUTION_SCRIPT: &str = r#"
local clock = redis.call('TIME')
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now)
redis.call('ZREMRANGEBYSCORE', KEYS[2], '-inf', now)
redis.call('ZREMRANGEBYSCORE', KEYS[6], '-inf', now)
local wait_deadline = tonumber(ARGV[2])
local execution_deadline = tonumber(ARGV[3])
if wait_deadline <= now or execution_deadline <= now
    or redis.call('EXISTS', KEYS[5]) == 1 then
  return {-1, '0'}
end
if ARGV[7] == '1' and not redis.call('ZSCORE', KEYS[1], ARGV[1]) then
  return {-1, '0'}
end
if redis.call('ZSCORE', KEYS[2], ARGV[1]) then
  return {1, '0'}
end
-- Recheck priority atomically with admission, including a displaced normal waiter.
if ARGV[8] ~= '1' and redis.call('ZCARD', KEYS[6]) > 0 then
  return {0, '100'}
end
local retry = 0
if redis.call('ZCARD', KEYS[2]) >= tonumber(ARGV[4]) then
  local earliest = redis.call('ZRANGE', KEYS[2], 0, 0, 'WITHSCORES')
  retry = math.max(1, math.ceil(tonumber(earliest[2]) - now))
end
local interval = tonumber(ARGV[5])
local last_started = tonumber(redis.call('GET', KEYS[4]) or '0')
if last_started > 0 and now - last_started < interval then
  retry = math.max(retry, interval - (now - last_started))
end
if retry > 0 then
  return {0, tostring(retry)}
end
local fence = redis.call('INCR', KEYS[3])
if fence > 9007199254740991 then
  return redis.error_reply('capacity wait fencing token exceeds exact Lua integer range')
end
redis.call('ZADD', KEYS[2], execution_deadline, ARGV[1])
if redis.call('PTTL', KEYS[2]) < execution_deadline - now then
  redis.call('PEXPIREAT', KEYS[2], execution_deadline)
end
redis.call('SET', KEYS[4], tostring(now), 'PX', ARGV[6])
if redis.call('PTTL', KEYS[3]) < tonumber(ARGV[6]) then
  redis.call('PEXPIRE', KEYS[3], ARGV[6])
end
redis.call('ZREM', KEYS[1], ARGV[1])
redis.call('ZREM', KEYS[6], ARGV[1])
if redis.call('ZCARD', KEYS[1]) == 0 then
  redis.call('DEL', KEYS[1])
end
return {1, tostring(fence)}
"#;

const CANCEL_SCRIPT: &str = r#"
local clock = redis.call('TIME')
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local deadline = tonumber(ARGV[2])
if deadline > now then
  redis.call('SET', KEYS[3], '1', 'PXAT', deadline)
end
redis.call('ZREM', KEYS[1], ARGV[1])
redis.call('ZREM', KEYS[2], ARGV[1])
redis.call('ZREM', KEYS[4], ARGV[1])
if redis.call('ZCARD', KEYS[1]) == 0 then
  redis.call('DEL', KEYS[1])
end
if redis.call('ZCARD', KEYS[2]) == 0 then
  redis.call('DEL', KEYS[2])
end
return 1
"#;

pub(super) struct RedisCapacityWait {
    connection: ConnectionManager,
    repository: RedisCredentialLeaseRepository,
    sender: mpsc::Sender<Cleanup>,
    process_id: Uuid,
    lifecycle: Arc<CleanupLifecycle>,
}

#[derive(Default)]
struct CleanupLifecycle {
    stopping: AtomicBool,
    owners: AtomicUsize,
    changed: Notify,
}

impl RedisCapacityWait {
    pub(super) async fn waiting_counts(
        &self,
        accounts: &[ProviderAccountId],
    ) -> Result<std::collections::BTreeMap<ProviderAccountId, u32>, ProviderStoreError> {
        let mut counts = std::collections::BTreeMap::new();
        let mut connection = self.connection.clone();
        let script = Script::new(
            "local t = redis.call('TIME'); local now = tonumber(t[1])*1000 + math.floor(tonumber(t[2])/1000); return redis.call('ZCOUNT', KEYS[1], '('..now, '+inf')",
        );
        for account in accounts {
            let keys = self
                .repository
                .keys(&CredentialLeaseRequest {
                    scope: CredentialLeaseScope::ProviderAccount,
                    resource_id: account.as_str().to_owned(),
                    owner_id: "queue-pressure".to_owned(),
                    ttl: Duration::from_secs(1),
                })
                .map_err(|_| invalid("queue pressure keys"))?;
            let count: u32 = script
                .key(format!("{}:waiting", keys[0]))
                .invoke_async(&mut connection)
                .await
                .map_err(|_| unavailable("read account queue pressure"))?;
            counts.insert(account.clone(), count);
        }
        Ok(counts)
    }

    pub(super) fn new(
        connection: ConnectionManager,
        repository: RedisCredentialLeaseRepository,
    ) -> (Self, CapacityWaitCleanupWriter) {
        let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
        let lifecycle = Arc::new(CleanupLifecycle::default());
        (
            Self {
                connection: connection.clone(),
                repository,
                sender,
                process_id: Uuid::new_v4(),
                lifecycle: Arc::clone(&lifecycle),
            },
            CapacityWaitCleanupWriter {
                connection,
                receiver: Mutex::new(receiver),
                lifecycle,
            },
        )
    }

    fn ownership(
        &self,
        account: &ProviderAccountId,
        identity: &str,
        deadline: SystemTime,
        quality: bool,
    ) -> Result<(Ownership, [String; 3]), ProviderStoreError> {
        if self.sender.is_closed() || self.lifecycle.stopping.load(Ordering::SeqCst) {
            return Err(unavailable("capacity wait cleanup worker unavailable"));
        }
        let mut keys = self
            .repository
            .keys(&CredentialLeaseRequest {
                scope: CredentialLeaseScope::ProviderAccount,
                resource_id: account.as_str().to_owned(),
                owner_id: "capacity-wait".to_owned(),
                ttl: Duration::from_secs(1),
            })
            .map_err(|_| invalid("capacity wait account keys"))?;
        // Keep the account hash tag, interval and fence; isolate active/cleanup keys.
        if quality {
            keys[0].push_str(":quality");
        }
        let fingerprint = resource_fingerprint("capacity wait request", identity)
            .map_err(|_| invalid("capacity wait request identity"))?;
        let token = format!(
            "wait|{}|{fingerprint}|{}",
            self.process_id.simple(),
            Uuid::new_v4().simple()
        );
        let cleanup = Cleanup {
            waiting_key: format!("{}:waiting", keys[0]),
            priority_key: format!("{}:waiting:priority", keys[0]),
            active_key: keys[0].clone(),
            cancelled_key: format!("{}:cancelled:{token}", keys[0]),
            token,
            wait_deadline: timestamp(deadline)?,
            cleanup_deadline: deadline,
        };
        self.lifecycle.owners.fetch_add(1, Ordering::SeqCst);
        let owner = Ownership {
            connection: self.connection.clone(),
            sender: self.sender.clone(),
            cleanup: Some(cleanup),
            lifecycle: Arc::clone(&self.lifecycle),
            renewal: None,
            execution_deadline: None,
        };
        // Pair with shutdown's stop-then-count sequence; never submit a write
        // after a zero-owner shutdown has closed the cleanup receiver.
        if self.lifecycle.stopping.load(Ordering::SeqCst) {
            return Err(unavailable("capacity wait cleanup worker stopping"));
        }
        Ok((owner, keys))
    }

    pub(super) fn acquire_scheduling(
        &self,
        request: ProviderSchedulingLeaseRequest,
    ) -> BoxFuture<'_, Result<ProviderLeaseAcquisition, ProviderStoreError>> {
        Box::pin(async move {
            let deadline = request.deadline().lease_deadline();
            let (mut owner, keys) = self.ownership(
                request.account_id(),
                "scheduling",
                deadline,
                request.is_quality_check(),
            )?;
            let (outcome, value) = owner.acquire_execution(&keys, &request, false).await?;
            match outcome {
                1 => {
                    owner.maintain(&request);
                    Ok(ProviderLeaseAcquisition::Acquired(Box::new(owner)))
                }
                0 => {
                    let retry_after = retry_interval(&value)?;
                    owner.cleanup = None;
                    Ok(ProviderLeaseAcquisition::Busy {
                        retry_after: Some(retry_after),
                    })
                }
                -1 => {
                    owner.release().await?;
                    Err(unavailable("acquire expired scheduling lease"))
                }
                _ => Err(invalid("invalid scheduling acquisition result")),
            }
        })
    }

    pub(super) fn acquire(
        &self,
        request: ProviderWaitLeaseRequest,
    ) -> BoxFuture<'_, Result<ProviderWaitLeaseAcquisition, ProviderStoreError>> {
        Box::pin(async move {
            let deadline = timestamp(request.deadline())?;
            // Ownership exists before the first write, including a lost Redis reply.
            let (mut owner, keys) = self.ownership(
                request.account_id(),
                request.request_id().as_str(),
                request.deadline(),
                false,
            )?;
            let cleanup = owner.cleanup.as_ref().expect("pending ownership");
            let mut connection = self.connection.clone();
            let outcome = Script::new(ENQUEUE_SCRIPT)
                .key(&cleanup.waiting_key)
                .key(&cleanup.cancelled_key)
                .key(&cleanup.priority_key)
                .arg(&cleanup.token)
                .arg(deadline)
                .arg(request.max_waiting().get())
                .arg(u8::from(request.priority()))
                .invoke_async::<i64>(&mut connection)
                .await
                .map_err(|_| unavailable("enqueue account capacity wait"))?;
            match outcome {
                1 => Ok(ProviderWaitLeaseAcquisition::Acquired(Box::new(
                    RedisWaitLease {
                        request,
                        keys,
                        owner: Some(owner),
                    },
                ))),
                0 => {
                    owner.cleanup = None;
                    Ok(ProviderWaitLeaseAcquisition::Full)
                }
                -1 => {
                    owner.release().await?;
                    Err(unavailable("enqueue expired account capacity wait"))
                }
                _ => Err(invalid("invalid capacity wait admission result")),
            }
        })
    }
}

struct RedisWaitLease {
    request: ProviderWaitLeaseRequest,
    keys: [String; 3],
    owner: Option<Ownership>,
}

impl ProviderWaitLease for RedisWaitLease {
    fn try_promote(
        &mut self,
        request: ProviderSchedulingLeaseRequest,
    ) -> BoxFuture<'_, Result<ProviderWaitPromotion, ProviderStoreError>> {
        // Moving ownership into the future fences a cancelled promotion even if
        // its caller keeps the waiting guard alive and tries again.
        let owner = self.owner.take();
        Box::pin(async move {
            let Some(mut owner) = owner else {
                return Ok(ProviderWaitPromotion::Expired);
            };
            if request.provider_kind() != self.request.provider_kind()
                || request.account_id() != self.request.account_id()
                || request.is_quality_check()
                || request.priority() != self.request.priority()
            {
                return Err(invalid("capacity promotion owner mismatch"));
            }
            let (outcome, value) = owner.acquire_execution(&self.keys, &request, true).await?;
            match outcome {
                1 => {
                    // No await between delivery and disarming the waiting guard.
                    owner.maintain(&request);
                    Ok(ProviderWaitPromotion::Acquired(Box::new(owner)))
                }
                0 => {
                    let retry_after = retry_interval(&value)?;
                    self.owner = Some(owner);
                    Ok(ProviderWaitPromotion::Busy {
                        retry_after: Some(retry_after),
                    })
                }
                -1 => {
                    owner.release().await?;
                    Ok(ProviderWaitPromotion::Expired)
                }
                _ => Err(invalid("invalid capacity promotion result")),
            }
        })
    }

    fn release(mut self: Box<Self>) -> BoxFuture<'static, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            if let Some(owner) = self.owner.take() {
                owner.release().await?;
            }
            Ok(())
        })
    }
}

struct Cleanup {
    waiting_key: String,
    priority_key: String,
    active_key: String,
    cancelled_key: String,
    token: String,
    wait_deadline: u64,
    cleanup_deadline: SystemTime,
}

struct Ownership {
    connection: ConnectionManager,
    sender: mpsc::Sender<Cleanup>,
    cleanup: Option<Cleanup>,
    lifecycle: Arc<CleanupLifecycle>,
    renewal: Option<crate::lease_renewal::LeaseRenewal>,
    execution_deadline: Option<Deadline>,
}

impl Ownership {
    fn maintain(&mut self, request: &ProviderSchedulingLeaseRequest) {
        let cleanup = self.cleanup.as_ref().expect("execution ownership");
        let active_key = cleanup.active_key.clone();
        let cancelled_key = cleanup.cancelled_key.clone();
        let token = cleanup.token.clone();
        let connection = self.connection.clone();
        self.execution_deadline = Some(request.deadline());
        self.renewal = Some(crate::lease_renewal::LeaseRenewal::spawn(
            request.deadline(),
            Some(request.cancellation()),
            move |ttl| {
                let mut connection = connection.clone();
                let active_key = active_key.clone();
                let cancelled_key = cancelled_key.clone();
                let token = token.clone();
                Box::pin(async move {
                    let renewed = Script::new(RENEW_EXECUTION_SCRIPT)
                        .key(active_key)
                        .key(cancelled_key)
                        .arg(token)
                        .arg(u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX))
                        .invoke_async::<i64>(&mut connection)
                        .await
                        .map_err(|_| crate::redis_unavailable("renew account execution"))?;
                    Ok(renewed == 1)
                })
            },
        ));
    }

    fn stop_renewal(&mut self) {
        self.renewal.take();
        // Cleanup retries must cover the latest possible renewed member, not
        // the initial acquisition horizon. Lua still removes only this owner.
        if let (Some(deadline), Some(cleanup)) = (self.execution_deadline, &mut self.cleanup) {
            cleanup.cleanup_deadline = cleanup.cleanup_deadline.max(deadline.lease_deadline());
        }
    }

    async fn acquire_execution(
        &mut self,
        keys: &[String; 3],
        request: &ProviderSchedulingLeaseRequest,
        require_wait: bool,
    ) -> Result<(i64, String), ProviderStoreError> {
        if self.lifecycle.stopping.load(Ordering::SeqCst) {
            return Err(unavailable("capacity wait cleanup worker stopping"));
        }
        let execution_deadline = request.deadline().lease_deadline();
        let execution_millis = timestamp(execution_deadline)?;
        let interval_millis = u64::try_from(request.request_interval().as_millis())
            .ok()
            .filter(|value| *value <= MAX_REDIS_EXACT_INTEGER)
            .ok_or_else(|| invalid("capacity promotion interval out of range"))?;
        let cleanup = self.cleanup.as_mut().expect("pending ownership");
        cleanup.cleanup_deadline = cleanup.cleanup_deadline.max(execution_deadline);
        let mut connection = self.connection.clone();
        Script::new(EXECUTION_SCRIPT)
            .key(&cleanup.waiting_key)
            .key(&cleanup.active_key)
            .key(&keys[1])
            .key(&keys[2])
            .key(&cleanup.cancelled_key)
            .key(&cleanup.priority_key)
            .arg(&cleanup.token)
            .arg(cleanup.wait_deadline)
            .arg(execution_millis)
            .arg(if request.is_quality_check() {
                u32::MAX
            } else {
                request.max_concurrent().get()
            })
            .arg(interval_millis)
            .arg(SIGNAL_TTL_MILLIS.max(interval_millis))
            .arg(u8::from(require_wait))
            .arg(u8::from(request.priority()))
            .invoke_async(&mut connection)
            .await
            .map_err(|_| unavailable("acquire account execution capacity"))
    }

    async fn release(mut self) -> Result<(), ProviderStoreError> {
        self.stop_renewal();
        if let Some(cleanup) = self.cleanup.as_ref() {
            cleanup_once(self.connection.clone(), cleanup).await?;
        }
        self.cleanup = None;
        Ok(())
    }
}

impl Drop for Ownership {
    fn drop(&mut self) {
        self.stop_renewal();
        if let Some(cleanup) = self.cleanup.take()
            && let Err(error) = self.sender.try_send(cleanup)
        {
            let reason = match error {
                mpsc::error::TrySendError::Full(_) => "full",
                mpsc::error::TrySendError::Closed(_) => "closed",
            };
            tracing::warn!(
                operation = "cancel_capacity_wait",
                reason,
                dropped = 1,
                "Capacity cleanup queue unavailable; original lease deadline bounds retention"
            );
        }
        self.lifecycle.owners.fetch_sub(1, Ordering::SeqCst);
        self.lifecycle.changed.notify_one();
    }
}

/// Store owns cleanup; Host bounds shutdown after its configured HTTP drain.
pub struct CapacityWaitCleanupWriter {
    connection: ConnectionManager,
    receiver: Mutex<mpsc::Receiver<Cleanup>>,
    lifecycle: Arc<CleanupLifecycle>,
}

impl DaemonTask for CapacityWaitCleanupWriter {
    fn run(&self, cancellation: CancellationToken) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let mut receiver = self.receiver.lock().await;
            let mut pending = FuturesUnordered::new();
            let mut input_closed = false;
            let mut stopping = false;
            let cancelled = cancellation.cancelled();
            tokio::pin!(cancelled);
            loop {
                if stopping && self.lifecycle.owners.load(Ordering::SeqCst) == 0 {
                    receiver.close();
                }
                if input_closed && pending.is_empty() {
                    return Ok(());
                }
                tokio::select! {
                    biased;
                    () = &mut cancelled, if !stopping => {
                        self.lifecycle.stopping.store(true, Ordering::SeqCst);
                        stopping = true;
                    }
                    () = self.lifecycle.changed.notified(), if stopping => {}
                    Some(()) = pending.next(), if !pending.is_empty() => {}
                    cleanup = receiver.recv(), if !input_closed && pending.len() < CLEANUP_CONCURRENCY => {
                        match cleanup {
                            Some(cleanup) => pending.push(cleanup_until_expired(
                                self.connection.clone(), cleanup,
                            )),
                            None => input_closed = true,
                        }
                    }
                }
            }
        })
    }
}

async fn cleanup_until_expired(connection: ConnectionManager, cleanup: Cleanup) {
    let remaining = cleanup
        .cleanup_deadline
        .duration_since(SystemTime::now())
        .unwrap_or_default();
    let deadline = Instant::now() + remaining;
    let mut failures = 0_u64;
    loop {
        let result =
            tokio::time::timeout_at(deadline, cleanup_once(connection.clone(), &cleanup)).await;
        if matches!(result, Ok(Ok(()))) {
            return;
        }
        failures = failures.saturating_add(1);
        if failures == 1 || Instant::now() >= deadline {
            tracing::warn!(
                operation = "cancel_capacity_wait",
                failures,
                "Capacity cleanup failed; retry is bounded by the original lease deadline"
            );
        }
        if Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep_until(deadline.min(Instant::now() + RETRY_DELAY)).await;
    }
}

async fn cleanup_once(
    mut connection: ConnectionManager,
    cleanup: &Cleanup,
) -> Result<(), ProviderStoreError> {
    tokio::time::timeout(IO_TIMEOUT, async {
        Script::new(CANCEL_SCRIPT)
            .key(&cleanup.waiting_key)
            .key(&cleanup.active_key)
            .key(&cleanup.cancelled_key)
            .key(&cleanup.priority_key)
            .arg(&cleanup.token)
            .arg(cleanup.wait_deadline)
            .invoke_async::<i64>(&mut connection)
            .await
    })
    .await
    .map_err(|_| unavailable("capacity wait cleanup timed out"))?
    .map_err(|_| unavailable("cancel account capacity wait"))?;
    Ok(())
}

fn timestamp(value: SystemTime) -> Result<u64, ProviderStoreError> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| u64::try_from(value.as_millis()).ok())
        .filter(|value| *value <= MAX_REDIS_EXACT_INTEGER)
        .ok_or_else(|| invalid("capacity wait deadline out of range"))
}

fn retry_interval(value: &str) -> Result<Duration, ProviderStoreError> {
    value
        .parse::<u64>()
        .map(Duration::from_millis)
        .map_err(|_| invalid("invalid capacity retry interval"))
}

fn unavailable(operation: &'static str) -> ProviderStoreError {
    ProviderStoreError::new(ProviderStoreErrorKind::Unavailable, operation)
}

fn invalid(operation: &'static str) -> ProviderStoreError {
    ProviderStoreError::new(ProviderStoreErrorKind::InvalidData, operation)
}
