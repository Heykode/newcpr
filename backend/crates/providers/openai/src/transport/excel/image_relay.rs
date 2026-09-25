use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use bytes::Bytes;
use gateway_core::provider_ports::{TemporaryImage, TemporaryImageSource};
use serde_json::{Map, Value};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use url::Url;

use super::{ExcelRequestError, images};

const TTL: Duration = Duration::from_secs(300);
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 128;

pub(crate) struct ImageRelay {
    origin: Option<String>,
    entries: Mutex<BTreeMap<String, Entry>>,
    byte_budget: Arc<Semaphore>,
    downloads: Arc<Semaphore>,
}

struct ImageBytes {
    bytes: Vec<u8>,
    _capacity: OwnedSemaphorePermit,
}

impl AsRef<[u8]> for ImageBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

struct DownloadBytes {
    bytes: Bytes,
    _slot: OwnedSemaphorePermit,
}

impl AsRef<[u8]> for DownloadBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

struct Entry {
    bytes: Bytes,
    content_type: &'static str,
    expires: Instant,
    downloads: u8,
}

pub(crate) struct ImageLease {
    relay: Weak<ImageRelay>,
    tokens: Vec<String>,
}

impl Drop for ImageLease {
    fn drop(&mut self) {
        if let Some(relay) = self.relay.upgrade() {
            let mut entries = relay
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for token in &self.tokens {
                entries.remove(token);
            }
        }
    }
}

pub(crate) fn validate_origin(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/"
            && url.host_str().is_some_and(|host| {
                matches!(url.host(), Some(url::Host::Domain(_)))
                    && host.contains('.')
                    && host != "localhost"
                    && !host.ends_with(".localhost")
                    && !host.ends_with(".local")
            })
    })
}

impl ImageRelay {
    pub(crate) fn new(origin: Option<String>) -> Self {
        Self {
            origin,
            entries: Mutex::new(BTreeMap::new()),
            byte_budget: Arc::new(Semaphore::new(MAX_BYTES)),
            downloads: Arc::new(Semaphore::new(32)),
        }
    }

    pub(crate) fn stage(
        self: &Arc<Self>,
        body: &mut Map<String, Value>,
    ) -> Result<Option<Arc<ImageLease>>, ExcelRequestError> {
        let Some(origin) = &self.origin else {
            return Ok(None);
        };
        let input = body.get("input").unwrap_or(&Value::Null);
        let budget = images::decoded_budget(input).map_err(|_| ExcelRequestError::Input)?;
        if budget == 0 {
            return Ok(None);
        }
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, entry| entry.expires > Instant::now());
        let mut reserved = Arc::clone(&self.byte_budget)
            .try_acquire_many_owned(budget as u32)
            .map_err(|_| ExcelRequestError::ImageRelay)?;
        let mut pictures = Vec::new();
        images::collect(input, &mut pictures, &mut 0).map_err(|_| ExcelRequestError::Input)?;
        if pictures.is_empty() {
            return Ok(None);
        }
        let mut candidates = Vec::with_capacity(pictures.len());
        for picture in pictures {
            let media = match imagesize::image_type(&picture.bytes)
                .map_err(|_| ExcelRequestError::Input)?
            {
                imagesize::ImageType::Png => "image/png",
                imagesize::ImageType::Jpeg => "image/jpeg",
                imagesize::ImageType::Gif => "image/gif",
                imagesize::ImageType::Webp => "image/webp",
                _ => return Err(ExcelRequestError::Input),
            };
            let dimensions =
                imagesize::blob_size(&picture.bytes).map_err(|_| ExcelRequestError::Input)?;
            if media != picture.media
                || dimensions.width == 0
                || dimensions.height == 0
                || dimensions.width > 16_384
                || dimensions.height > 16_384
                || dimensions.width.saturating_mul(dimensions.height) > 40_000_000
            {
                return Err(ExcelRequestError::Input);
            }
            let mut secret = [0_u8; 32];
            getrandom::fill(&mut secret).map_err(|_| ExcelRequestError::ImageRelay)?;
            candidates.push((hex::encode(secret), picture));
        }
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| entry.expires > now);
        if entries.len() + candidates.len() > MAX_ENTRIES {
            return Err(ExcelRequestError::ImageRelay);
        }
        let mut replacements = BTreeMap::new();
        let mut tokens = Vec::with_capacity(candidates.len());
        for (token, picture) in candidates {
            replacements.insert(
                picture.url,
                format!("{}/_cpr/excel-images/{token}", origin.trim_end_matches('/')),
            );
            entries.insert(
                token.clone(),
                Entry {
                    bytes: Bytes::from_owner(ImageBytes {
                        _capacity: reserved
                            .split(picture.bytes.len())
                            .expect("decoded size is bounded by the base64 estimate"),
                        bytes: picture.bytes,
                    }),
                    content_type: picture.media,
                    expires: now + TTL,
                    downloads: 0,
                },
            );
            tokens.push(token);
        }
        if let Some(input) = body.get_mut("input") {
            rewrite_urls(input, &replacements);
        }
        Ok(Some(Arc::new(ImageLease {
            relay: Arc::downgrade(self),
            tokens,
        })))
    }
}

impl TemporaryImageSource for ImageRelay {
    fn read(&self, capability: &str) -> Option<TemporaryImage> {
        if capability.len() != 64 || !capability.bytes().all(|ch| ch.is_ascii_hexdigit()) {
            return None;
        }
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| entry.expires > now);
        let entry = entries.get_mut(capability)?;
        if entry.downloads >= 16 {
            return None;
        }
        let slot = Arc::clone(&self.downloads).try_acquire_owned().ok()?;
        entry.downloads += 1;
        Some(TemporaryImage {
            content_type: entry.content_type,
            // Both permits live until the last HTTP-body clone is released.
            bytes: Bytes::from_owner(DownloadBytes {
                bytes: entry.bytes.clone(),
                _slot: slot,
            }),
        })
    }
}

fn rewrite_urls(value: &mut Value, replacements: &BTreeMap<String, String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                rewrite_urls(value, replacements);
            }
        }
        Value::Object(fields) => {
            if fields.get("type").and_then(Value::as_str) == Some("input_image")
                && let Some(url) = fields
                    .get("image_url")
                    .and_then(Value::as_str)
                    .and_then(|url| replacements.get(url))
            {
                fields.insert("image_url".into(), url.clone().into());
            } else {
                for value in fields.values_mut() {
                    rewrite_urls(value, replacements);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input() -> Map<String, Value> {
        json!({"input":[{"role":"user","content":[{"type":"input_image",
            "image_url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg=="}]}]})
            .as_object().unwrap().clone()
    }

    #[test]
    fn excel_image_relay_is_opt_in_bounded_and_released_on_drop() {
        let mut body = input();
        let original = body.clone();
        assert!(
            Arc::new(ImageRelay::new(None))
                .stage(&mut body)
                .unwrap()
                .is_none()
        );
        assert_eq!(original, body);
        let relay = Arc::new(ImageRelay::new(Some("https://images.example.com".into())));
        let mut leases = Vec::new();
        for _ in 0..MAX_ENTRIES {
            leases.push(relay.stage(&mut input()).unwrap().unwrap());
        }
        assert!(relay.stage(&mut input()).is_err());
        let token = leases[0].tokens[0].clone();
        let image = relay.read(&token).unwrap();
        assert_eq!(image.content_type, "image/png");
        assert!(image.bytes.starts_with(b"\x89PNG"));
        assert!(relay.read("../config.yaml").is_none());
        drop(leases);
        assert!(relay.read(&token).is_none());
        assert!(relay.stage(&mut input()).is_ok());
    }

    #[test]
    fn excel_image_relay_expiry_download_limits_and_media_are_checked() {
        let relay = Arc::new(ImageRelay::new(Some("https://images.example.com".into())));
        let lease = relay.stage(&mut input()).unwrap().unwrap();
        let token = &lease.tokens[0];
        for _ in 0..16 {
            assert!(relay.read(token).is_some());
        }
        assert!(relay.read(token).is_none());
        relay
            .entries
            .lock()
            .unwrap()
            .get_mut(token)
            .unwrap()
            .expires = Instant::now() - TTL;
        assert!(relay.read(token).is_none());
        let mut wrong = input();
        wrong["input"][0]["content"][0]["image_url"] = "data:image/png;base64,AQID".into();
        assert!(relay.stage(&mut wrong).is_err());
        for origin in [
            "http://example.com",
            "https://user:pass@example.com",
            "https://localhost",
            "https://example.com/path?secret=yes",
        ] {
            assert!(!validate_origin(origin));
        }
    }

    #[test]
    fn excel_image_capacity_and_download_slots_follow_body_lifetime() {
        let relay = Arc::new(ImageRelay::new(Some("https://images.example.com".into())));
        let leases: Vec<_> = (0..3)
            .map(|_| relay.stage(&mut input()).unwrap().unwrap())
            .collect();
        let bytes_per_image = relay
            .entries
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .bytes
            .len();
        let mut downloads = Vec::new();
        for index in 0..32 {
            downloads.push(relay.read(&leases[index % 3].tokens[0]).unwrap());
        }
        assert!(relay.read(&leases[0].tokens[0]).is_none());
        let token = leases[0].tokens[0].clone();
        drop(leases);
        assert!(relay.read(&token).is_none());
        assert_eq!(
            relay.byte_budget.available_permits(),
            MAX_BYTES - 3 * bytes_per_image
        );
        let copy = downloads[0].bytes.clone();
        drop(downloads);
        assert_eq!(relay.downloads.available_permits(), 31);
        assert_eq!(
            relay.byte_budget.available_permits(),
            MAX_BYTES - bytes_per_image
        );
        drop(copy);
        assert_eq!(relay.byte_budget.available_permits(), MAX_BYTES);
        assert_eq!(relay.downloads.available_permits(), 32);
        let all = Arc::clone(&relay.byte_budget)
            .try_acquire_many_owned(MAX_BYTES as u32)
            .unwrap();
        let mut bad = input();
        bad["input"][0]["content"][0]["image_url"] = "data:image/png;base64,!!!!".into();
        assert!(matches!(
            relay.stage(&mut bad),
            Err(ExcelRequestError::ImageRelay)
        ));
        assert!(
            relay
                .stage(json!({"input":"text"}).as_object_mut().unwrap())
                .unwrap()
                .is_none()
        );
        drop(all);
        assert!(matches!(
            relay.stage(&mut bad),
            Err(ExcelRequestError::Input)
        ));
        assert_eq!(relay.byte_budget.available_permits(), MAX_BYTES);
    }
}
