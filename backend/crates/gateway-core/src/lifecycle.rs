//! Host 与 API 之间的连接注册、drain 与取消契约。

use std::fmt;
use std::sync::{Arc, OnceLock};

use std::time::{Duration, SystemTime};

use event_listener::Event;
use futures::future::{BoxFuture, pending};
use futures_timer::Delay;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestLeaseKind {
    ClientAdmission,
    Credential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseFailure {
    Lost,
    RenewalTimedOut,
    Expired,
}

/// Stable causes only; never include credentials or infrastructure error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellationReason {
    Requested,
    Lease {
        kind: RequestLeaseKind,
        failure: LeaseFailure,
    },
}

impl CancellationReason {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Requested => "request was cancelled",
            Self::Lease {
                kind: RequestLeaseKind::ClientAdmission,
                failure,
            } => match failure {
                LeaseFailure::Lost => "request was cancelled: client admission lease was lost",
                LeaseFailure::RenewalTimedOut => {
                    "request was cancelled: client admission lease renewal timed out"
                }
                LeaseFailure::Expired => "request was cancelled: client admission lease expired",
            },
            Self::Lease {
                kind: RequestLeaseKind::Credential,
                failure,
            } => match failure {
                LeaseFailure::Lost => "request was cancelled: credential lease was lost",
                LeaseFailure::RenewalTimedOut => {
                    "request was cancelled: credential lease renewal timed out"
                }
                LeaseFailure::Expired => "request was cancelled: credential lease expired",
            },
        }
    }
}

struct CancellationState {
    reason: OnceLock<CancellationReason>,
    event: Event,
}

/// 可克隆的请求、任务与连接取消信号。
#[derive(Clone)]
pub struct CancellationToken(Arc<CancellationState>);

impl fmt::Debug for CancellationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CancellationToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(CancellationState {
            reason: OnceLock::new(),
            event: Event::new(),
        }))
    }

    pub fn cancel(&self) {
        self.cancel_with_reason(CancellationReason::Requested);
    }

    pub fn cancel_with_reason(&self, reason: CancellationReason) {
        if self.0.reason.set(reason).is_err() {
            return;
        }
        self.0.event.notify(usize::MAX);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.reason().is_some()
    }

    #[must_use]
    pub fn reason(&self) -> Option<CancellationReason> {
        self.0.reason.get().copied()
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        // Dropping a losing select branch also unregisters its listener.
        let listener = self.0.event.listen();
        if self.is_cancelled() {
            return;
        }
        listener.await;
    }
}

/// 进程已进入 drain，新连接不得再注册。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionDraining;

impl fmt::Display for ConnectionDraining {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("connection lifecycle is draining")
    }
}

impl std::error::Error for ConnectionDraining {}

/// 一次成功的活跃连接注册。
///
/// 实现必须在 guard `Drop` 时原子减少活跃连接计数。
pub trait ConnectionGuard: Send + 'static {}

/// API 消费、Host 实现的进程连接生命周期。
pub trait ConnectionLifecycle: Send + Sync {
    /// 原子地检查 drain 状态并注册一个活跃连接。
    ///
    /// 当本方法成功时，drain 必须等待返回的 guard 被释放；
    /// 当 drain 已经线性化生效时，本方法必须返回 [`ConnectionDraining`]。
    fn try_register(&self) -> Result<Box<dyn ConnectionGuard>, ConnectionDraining>;

    fn cancellation(&self) -> CancellationToken;

    fn is_draining(&self) -> bool;
}
/// 请求默认没有总时长限制；显式截止与用于异常回收的可续期租约分开。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Deadline(Option<SystemTime>);

/// 租约失去续期后有界回收，不作为模型请求的总执行预算。
pub const REQUEST_LEASE_TTL: Duration = Duration::from_secs(10 * 60);

impl Deadline {
    pub fn from_timeout(started_at: SystemTime, timeout: Option<Duration>) -> Option<Self> {
        match timeout {
            Some(timeout) => started_at.checked_add(timeout).map(|at| Self(Some(at))),
            None => Some(Self(None)),
        }
    }

    #[must_use]
    pub const fn at(self) -> Option<SystemTime> {
        self.0
    }

    #[must_use]
    pub fn remaining(self) -> Option<Duration> {
        self.0
            .map(|at| at.duration_since(SystemTime::now()).unwrap_or_default())
    }

    #[must_use]
    pub fn bounded(self, maximum: Duration) -> Duration {
        self.remaining()
            .map_or(maximum, |remaining| remaining.min(maximum))
    }

    #[must_use]
    pub fn is_elapsed(self) -> bool {
        self.remaining()
            .is_some_and(|remaining| remaining.is_zero())
    }

    #[must_use]
    pub fn min(self, other: SystemTime) -> Self {
        Self(Some(self.0.map_or(other, |at| at.min(other))))
    }

    #[must_use]
    pub fn lease_deadline(self) -> SystemTime {
        let expires = SystemTime::now() + REQUEST_LEASE_TTL;
        self.0.map_or(expires, |at| at.min(expires))
    }

    pub fn wait(self) -> BoxFuture<'static, ()> {
        match self.remaining() {
            Some(remaining) => Box::pin(Delay::new(remaining)),
            None => Box::pin(pending()),
        }
    }
}

impl From<SystemTime> for Deadline {
    fn from(at: SystemTime) -> Self {
        Self(Some(at))
    }
}

/// 持有者释放即停止续期，具体执行器由 Store 提供。
pub trait LeaseGuard: Send + Sync {}
impl<T: Send + Sync> LeaseGuard for T {}
