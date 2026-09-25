use bytes::Bytes;

/// A short-lived capability download; implementations never expose local paths.
pub struct TemporaryImage {
    pub content_type: &'static str,
    pub bytes: Bytes,
}

pub trait TemporaryImageSource: Send + Sync {
    fn read(&self, capability: &str) -> Option<TemporaryImage>;
}
