//! Request-local control of the response owned by the current upstream exchange.

use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use crate::lifecycle::CancellationToken;
use async_trait::async_trait;

#[async_trait]
pub trait ResponseControlTransport: Send + Sync {
    async fn send(&self, payload: &str) -> Result<(), ResponseControlUnavailable>;
    async fn receive(&self) -> Result<String, ResponseControlUnavailable>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseControlUnavailable;

#[derive(Clone)]
pub struct ResponseControlBinding(Weak<dyn ResponseControlTransport>);

impl ResponseControlBinding {
    pub fn same_binding(&self, other: &Self) -> bool {
        self.0.ptr_eq(&other.0)
    }

    pub async fn send(&self, payload: &str) -> Result<(), ResponseControlUnavailable> {
        self.0
            .upgrade()
            .ok_or(ResponseControlUnavailable)?
            .send(payload)
            .await
    }
}

#[derive(Clone, Default)]
pub struct ResponseControl {
    connection_cancellation: Arc<Mutex<Option<CancellationToken>>>,
    active: Arc<Mutex<Weak<InterruptTarget>>>,
    target: Arc<Mutex<Option<Weak<dyn ResponseControlTransport>>>>,
}

impl fmt::Debug for ResponseControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResponseControl([REDACTED])")
    }
}

struct InterruptTarget {
    response_id: String,
    requested: CancellationToken,
}

pub struct ActiveResponseInterrupt(Arc<InterruptTarget>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseInterruptError {
    Unavailable,
    ResponseMismatch,
}

impl ResponseControl {
    /// Set before the cold execution starts; shared by this response's attempts.
    pub fn set_connection_cancellation(&self, cancellation: CancellationToken) {
        *self
            .connection_cancellation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cancellation);
    }

    pub fn connection_cancellation(&self) -> Option<CancellationToken> {
        self.connection_cancellation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn binding(&self) -> Option<ResponseControlBinding> {
        self.target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .cloned()
            .map(ResponseControlBinding)
    }

    pub fn bind(&self, transport: &Arc<dyn ResponseControlTransport>) {
        *self
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(transport));
    }

    pub fn clear(&self) {
        *self
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn transport(&self) -> Option<Arc<dyn ResponseControlTransport>> {
        self.target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
    }

    pub async fn send(&self, payload: &str) -> Result<(), ResponseControlUnavailable> {
        self.transport()
            .ok_or(ResponseControlUnavailable)?
            .send(payload)
            .await
    }

    pub async fn receive(&self) -> Result<String, ResponseControlUnavailable> {
        match self.transport() {
            Some(transport) => transport.receive().await,
            None => std::future::pending().await,
        }
    }

    pub fn activate(&self, response_id: String) -> Option<ActiveResponseInterrupt> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active.upgrade().is_some() || response_id.is_empty() {
            return None;
        }
        let target = Arc::new(InterruptTarget {
            response_id,
            requested: CancellationToken::new(),
        });
        *active = Arc::downgrade(&target);
        Some(ActiveResponseInterrupt(target))
    }

    pub fn interrupt(&self, response_id: &str) -> Result<(), ResponseInterruptError> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let target = active
            .upgrade()
            .ok_or(ResponseInterruptError::Unavailable)?;
        if target.response_id != response_id {
            return Err(ResponseInterruptError::ResponseMismatch);
        }
        target.requested.cancel();
        Ok(())
    }
}

impl ActiveResponseInterrupt {
    #[must_use]
    pub fn response_id(&self) -> &str {
        &self.0.response_id
    }

    pub fn requested(&self) -> impl Future<Output = ()> + Send + 'static {
        // A waiter must not extend the active response owner's lifetime.
        let requested = self.0.requested.clone();
        async move { requested.cancelled().await }
    }
}
