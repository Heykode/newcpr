//! Optional, nonblocking diagnostic observer. It never decides routing or delivery.

use std::sync::Arc;

use serde_json::Value;

pub trait RequestCaptureFactory: Send + Sync {
    fn start(
        &self,
        request_id: &str,
        key_id: &str,
        group_ids: &[&str],
    ) -> Option<Arc<dyn RequestCaptureObserver>>;
}

pub trait RequestCaptureObserver: Send + Sync {
    fn body(&self, stage: &'static str, attempt: u32, exchange: Option<u64>, bytes: &[u8]);
    fn fact(&self, stage: &'static str, attempt: u32, data: &Value);
}
