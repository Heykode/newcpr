//! Request-local control of the response owned by the current upstream exchange.

use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use crate::lifecycle::CancellationToken;

#[derive(Clone, Default)]
pub struct ResponseControl {
    active: Arc<Mutex<Weak<InterruptTarget>>>,
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
