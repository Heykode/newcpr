use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use bytes::Bytes;
use gateway_core::provider_ports::{TemporaryImage, TemporaryImageSource};
use serde_json::{Map, Value};
use url::Url;

use super::{ExcelRequestError, images};

const MEMORY_BUDGET: usize = 1024 * 1024 * 1024;
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const ORPHAN_GRACE: Duration = Duration::from_secs(31 * 60);
#[cfg(test)]
const TTL: Duration = Duration::from_secs(30 * 60);

pub(crate) struct ImageRelay {
    origin: Option<String>,
    entries: Mutex<BTreeMap<String, Entry>>,
    byte_budget: Arc<AtomicUsize>,
    entry_budget: Arc<AtomicUsize>,
    memory_budget: Arc<AtomicUsize>,
    downloads: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    tuning: gateway_core::runtime::RequestTuningHandle,
    secret: Option<[u8; 32]>,
    directory: Option<RelayDirectory>,
}

struct RelayDirectory {
    _owner: std::fs::File,
    directory: tempfile::TempDir,
}

impl RelayDirectory {
    fn create(root: &Path) -> std::io::Result<Self> {
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = root.symlink_metadata()?;
        if !metadata.is_dir() {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
        }
        let directory = tempfile::Builder::new()
            .prefix("session-")
            .tempdir_in(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        let owner = std::fs::File::create_new(directory.path().join(".owner"))?;
        owner.try_lock().map_err(std::io::Error::from)?;
        let storage = Self {
            _owner: owner,
            directory,
        };
        storage.cleanup_orphans();
        Ok(storage)
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    fn cleanup_orphans(&self) {
        let Some(root) = self.path().parent() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == self.path()
                || !entry.file_name().to_string_lossy().starts_with("session-")
                || !entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                continue;
            }
            let marker = path.join(".owner");
            let Ok(metadata) = marker.symlink_metadata() else {
                continue;
            };
            if !metadata.is_file()
                || !metadata
                    .modified()
                    .ok()
                    .and_then(|time| SystemTime::now().duration_since(time).ok())
                    .is_some_and(|age| age > ORPHAN_GRACE)
            {
                continue;
            }
            let Ok(owner) = std::fs::File::options().read(true).write(true).open(marker) else {
                continue;
            };
            // An idle but live process still owns the lock; age alone never permits deletion.
            if owner.try_lock().is_ok() {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
}

struct DiskImage {
    path: tempfile::TempPath,
    size: usize,
    _capacity: CapacityPermit,
    _entry: CapacityPermit,
}

struct DownloadBytes {
    bytes: Vec<u8>,
    _image: Arc<DiskImage>,
    _memory: CapacityPermit,
    _slot: CapacityPermit,
}

struct CapacityPermit {
    used: Arc<AtomicUsize>,
    amount: usize,
}

impl CapacityPermit {
    fn acquire(used: &Arc<AtomicUsize>, amount: usize, limit: usize) -> Option<Self> {
        used.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            value.checked_add(amount).filter(|next| *next <= limit)
        })
        .ok()?;
        Some(Self {
            used: Arc::clone(used),
            amount,
        })
    }

    fn split(&mut self, amount: usize) -> Self {
        self.amount = self
            .amount
            .checked_sub(amount)
            .expect("reserved image capacity");
        Self {
            used: Arc::clone(&self.used),
            amount,
        }
    }
}

impl Drop for CapacityPermit {
    fn drop(&mut self) {
        self.used.fetch_sub(self.amount, Ordering::AcqRel);
    }
}

impl AsRef<[u8]> for DownloadBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

struct Entry {
    image: Arc<DiskImage>,
    content_type: &'static str,
    expires: Instant,
}

pub(crate) struct ImageLease {
    #[cfg(test)]
    tokens: Vec<String>,
    _request: CapacityPermit,
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
        let mut secret = [0u8; 32];
        let secret = getrandom::fill(&mut secret).ok().map(|()| secret);
        let directory = origin.as_ref().and_then(|_| {
            RelayDirectory::create(&std::env::temp_dir().join("cpr-excel-images")).ok()
        });
        Self {
            origin,
            entries: Mutex::new(BTreeMap::new()),
            byte_budget: Arc::new(AtomicUsize::new(0)),
            entry_budget: Arc::new(AtomicUsize::new(0)),
            memory_budget: Arc::new(AtomicUsize::new(0)),
            downloads: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(AtomicUsize::new(0)),
            tuning: Default::default(),
            secret,
            directory,
        }
    }

    pub(crate) fn start_cleanup(self: &Arc<Self>) {
        if self.origin.is_none() {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CLEANUP_INTERVAL).await;
                let Some(relay) = weak.upgrade() else { return };
                let _ = tokio::task::spawn_blocking(move || {
                    relay.cleanup();
                    if let Some(directory) = &relay.directory {
                        directory.cleanup_orphans();
                    }
                })
                .await;
            }
        });
    }

    fn cleanup(&self) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, entry| entry.expires > Instant::now());
    }

    pub(crate) fn with_request_tuning(
        mut self,
        tuning: gateway_core::runtime::RequestTuningHandle,
    ) -> Self {
        self.tuning = tuning;
        self
    }

    pub(crate) fn request_tuning(&self) -> gateway_core::routing::RequestTuning {
        self.tuning.load()
    }

    pub(crate) fn enabled(&self) -> bool {
        self.origin.is_some()
    }

    pub(crate) fn stage_with_tuning(
        self: &Arc<Self>,
        body: &mut Map<String, Value>,
        limits: gateway_core::routing::RequestTuning,
        scope: &str,
    ) -> Result<Option<Arc<ImageLease>>, ExcelRequestError> {
        let Some(origin) = &self.origin else {
            return Ok(None);
        };
        let input = body.get("input").unwrap_or(&Value::Null);
        let budget = images::decoded_budget_user_with_limits(input, limits.into())
            .map_err(|_| ExcelRequestError::Input)?;
        if budget == 0 {
            return Ok(None);
        }
        self.cleanup();
        let directory = self
            .directory
            .as_ref()
            .ok_or(ExcelRequestError::ImageRelay)?;
        let request = CapacityPermit::acquire(
            &self.requests,
            1,
            limits.excel_image_relay_requests as usize,
        )
        .ok_or(ExcelRequestError::ImageRelay)?;
        let _memory = CapacityPermit::acquire(&self.memory_budget, budget, MEMORY_BUDGET)
            .ok_or(ExcelRequestError::ImageRelay)?;
        let mut pictures = Vec::new();
        images::collect_user_with_limits(input, &mut pictures, limits.into())
            .map_err(|_| ExcelRequestError::Input)?;
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
                || dimensions.width.saturating_mul(dimensions.height) > 64 * 1024 * 1024
            {
                return Err(ExcelRequestError::Input);
            }
            let secret = self.secret.as_ref().ok_or(ExcelRequestError::ImageRelay)?;
            let token = crate::transport::session::hmac_sha256(
                secret,
                &[
                    scope.as_bytes(),
                    b"\0",
                    picture.media.as_bytes(),
                    b"\0",
                    &picture.bytes,
                ],
            );
            candidates.push((hex::encode(token), picture));
        }
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| entry.expires > now);
        let new_tokens: std::collections::BTreeSet<_> = candidates
            .iter()
            .map(|(token, _)| token)
            .filter(|token| !entries.contains_key(*token))
            .collect();
        if entries.len() + new_tokens.len() > limits.excel_image_relay_entries as usize {
            return Err(ExcelRequestError::ImageRelay);
        }
        let mut reserved_entries = CapacityPermit::acquire(
            &self.entry_budget,
            new_tokens.len(),
            limits.excel_image_relay_entries as usize,
        )
        .ok_or(ExcelRequestError::ImageRelay)?;
        let mut bytes_by_token = BTreeMap::new();
        for (token, picture) in &candidates {
            if !entries.contains_key(token) {
                bytes_by_token.insert(token, picture.bytes.len());
            }
        }
        let new_bytes = bytes_by_token.values().sum();
        let mut reserved = CapacityPermit::acquire(
            &self.byte_budget,
            new_bytes,
            limits.excel_image_relay_bytes as usize,
        )
        .ok_or(ExcelRequestError::ImageRelay)?;
        let ttl = Duration::from_secs(
            u64::from(limits.excel_image_relay_ttl_minutes.clamp(1, 1440)) * 60,
        );
        // Write all new files before publishing any links; a failed batch owns no entries.
        let mut staged = BTreeMap::new();
        for (token, picture) in &candidates {
            if entries.contains_key(token) || staged.contains_key(token) {
                continue;
            }
            let mut file = tempfile::NamedTempFile::new_in(directory.path())
                .map_err(|_| ExcelRequestError::ImageRelay)?;
            file.write_all(&picture.bytes)
                .map_err(|_| ExcelRequestError::ImageRelay)?;
            staged.insert(
                token.clone(),
                Entry {
                    image: Arc::new(DiskImage {
                        path: file.into_temp_path(),
                        size: picture.bytes.len(),
                        _capacity: reserved.split(picture.bytes.len()),
                        _entry: reserved_entries.split(1),
                    }),
                    content_type: picture.media,
                    expires: now + ttl,
                },
            );
        }
        entries.append(&mut staged);
        let mut replacements = BTreeMap::new();
        #[cfg(test)]
        let mut tokens = Vec::with_capacity(candidates.len());
        for (token, picture) in candidates {
            replacements.insert(
                picture.url,
                format!("{}/_cpr/excel-images/{token}", origin.trim_end_matches('/')),
            );
            entries.get_mut(&token).expect("published image").expires = now + ttl;
            #[cfg(test)]
            tokens.push(token);
        }
        if let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) {
            for item in items
                .iter_mut()
                .filter(|item| images::is_user_message(item))
            {
                if let Some(content) = item.get_mut("content") {
                    rewrite_user_urls(content, &replacements);
                }
            }
        }
        Ok(Some(Arc::new(ImageLease {
            #[cfg(test)]
            tokens,
            _request: request,
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
        let entry = entries.get(capability)?;
        let image = Arc::clone(&entry.image);
        let content_type = entry.content_type;
        drop(entries);
        let slot = CapacityPermit::acquire(
            &self.downloads,
            1,
            self.tuning.load().excel_image_relay_downloads as usize,
        )?;
        let memory = CapacityPermit::acquire(&self.memory_budget, image.size, MEMORY_BUDGET)?;
        let mut file = std::fs::File::open(&image.path).ok()?;
        if file.metadata().ok()?.len() != image.size as u64 {
            return None;
        }
        let mut bytes = vec![0; image.size];
        file.read_exact(&mut bytes).ok()?;
        if file.read(&mut [0]).ok()? != 0 {
            return None;
        }
        if bytes.len() != image.size {
            return None;
        }
        Some(TemporaryImage {
            content_type,
            // Both permits live until the last HTTP-body clone is released.
            bytes: Bytes::from_owner(DownloadBytes {
                bytes,
                _image: image,
                _memory: memory,
                _slot: slot,
            }),
        })
    }
}

fn rewrite_user_urls(value: &mut Value, replacements: &BTreeMap<String, String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                rewrite_user_urls(value, replacements);
            }
        }
        Value::Object(fields) => {
            if matches!(
                fields.get("type").and_then(Value::as_str),
                Some("function_call_output" | "custom_tool_call_output")
            ) {
                return;
            }
            if fields.get("type").and_then(Value::as_str) == Some("input_image")
                && let Some(url) = fields
                    .get("image_url")
                    .and_then(Value::as_str)
                    .and_then(|url| replacements.get(url))
            {
                fields.insert("image_url".into(), url.clone().into());
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    const MAX_BYTES: usize = 1024 * 1024 * 1024;
    const MAX_REQUESTS: usize = 128;
    use super::*;
    use serde_json::json;

    #[test]
    fn excel_image_orphan_cleanup_preserves_live_and_unmanaged_directories() {
        let root = tempfile::tempdir().unwrap();
        let storage_root = root.path().join("private");
        let live = RelayDirectory::create(&storage_root).unwrap();
        let crashed = RelayDirectory::create(&storage_root).unwrap();
        let live_path = live.path().to_path_buf();
        let crashed_path = crashed.path().to_path_buf();
        let old = SystemTime::now() - ORPHAN_GRACE - Duration::from_secs(60);
        for directory in [&live, &crashed] {
            directory
                ._owner
                .set_times(std::fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let RelayDirectory { _owner, directory } = crashed;
        drop(_owner);
        assert_eq!(directory.keep(), crashed_path);
        let unmanaged = storage_root.join("session-unmanaged");
        std::fs::create_dir(&unmanaged).unwrap();
        live.cleanup_orphans();
        assert!(live_path.exists());
        assert!(unmanaged.exists());
        assert!(!crashed_path.exists());
        drop(live);
        assert!(!live_path.exists());
    }

    impl ImageRelay {
        pub(crate) fn stage(
            self: &Arc<Self>,
            body: &mut Map<String, Value>,
        ) -> Result<Option<Arc<ImageLease>>, ExcelRequestError> {
            self.stage_with_tuning(body, self.tuning.load(), &uuid::Uuid::new_v4().to_string())
        }
    }

    fn input() -> Map<String, Value> {
        json!({"input":[{"role":"user","content":[{"type":"input_image",
            "image_url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg=="}]}]})
            .as_object().unwrap().clone()
    }

    #[test]
    fn excel_image_relay_is_opt_in_bounded_and_survives_request_drop() {
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
        for _ in 0..MAX_REQUESTS {
            leases.push(relay.stage(&mut input()).unwrap().unwrap());
        }
        assert!(relay.stage(&mut input()).is_err());
        let token = leases[0].tokens[0].clone();
        let image = relay.read(&token).unwrap();
        assert_eq!(image.content_type, "image/png");
        assert!(image.bytes.starts_with(b"\x89PNG"));
        assert!(relay.read("../config.yaml").is_none());
        drop(leases);
        assert!(relay.read(&token).is_some());
        assert!(relay.stage(&mut input()).is_ok());
    }

    #[test]
    fn excel_image_request_slots_survive_clones_and_release_on_error_or_drop() {
        let tuning =
            gateway_core::runtime::RequestTuningHandle::new(gateway_core::routing::RequestTuning {
                excel_image_relay_requests: 1,
                ..Default::default()
            });
        let relay = Arc::new(
            ImageRelay::new(Some("https://images.example.com".into()))
                .with_request_tuning(tuning.clone()),
        );
        let first = relay.stage(&mut input()).unwrap().unwrap();
        let clone = first.clone();
        assert!(relay.stage(&mut input()).is_err());
        assert!(
            relay
                .stage(json!({"input":"text"}).as_object_mut().unwrap())
                .unwrap()
                .is_none()
        );
        drop(first);
        assert_eq!(relay.requests.load(Ordering::Acquire), 1);
        tuning.publish(gateway_core::routing::RequestTuning {
            excel_image_relay_requests: 2,
            ..Default::default()
        });
        let second = relay.stage(&mut input()).unwrap().unwrap();
        assert_eq!(relay.requests.load(Ordering::Acquire), 2);
        drop((clone, second));
        assert_eq!(relay.requests.load(Ordering::Acquire), 0);
        let mut wrong = input();
        wrong["input"][0]["content"][0]["image_url"] = "data:image/png;base64,AQID".into();
        assert!(relay.stage(&mut wrong).is_err());
        assert_eq!(relay.requests.load(Ordering::Acquire), 0);
        relay.entries.lock().unwrap().clear();
        assert_eq!(relay.byte_budget.load(Ordering::Acquire), 0);
    }

    #[test]
    fn excel_image_relay_expiry_download_limits_and_media_are_checked() {
        let relay = Arc::new(ImageRelay::new(Some("https://images.example.com".into())));
        let lease = relay.stage(&mut input()).unwrap().unwrap();
        let token = &lease.tokens[0];
        for _ in 0..16 {
            assert!(relay.read(token).is_some());
        }
        assert!(relay.read(token).is_some());
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
            "https://user:$pass@example.com",
            "https://localhost",
            "https://example.com/path?secret=yes",
        ] {
            assert!(!validate_origin(origin));
        }
    }

    #[test]
    fn excel_image_limits_change_without_resetting_outstanding_owners() {
        let tuning = gateway_core::runtime::RequestTuningHandle::default();
        let relay = Arc::new(
            ImageRelay::new(Some("https://images.example.com".into()))
                .with_request_tuning(tuning.clone()),
        );
        let first = relay.stage(&mut input()).unwrap().unwrap();
        let image = relay.read(&first.tokens[0]).unwrap();
        let used = relay.byte_budget.load(Ordering::Acquire);
        tuning.publish(gateway_core::routing::RequestTuning {
            excel_image_relay_bytes: used as u64,
            excel_image_relay_entries: 1,
            excel_image_relay_downloads: 1,
            ..Default::default()
        });
        assert!(relay.stage(&mut input()).is_err());
        assert!(relay.read(&first.tokens[0]).is_none());
        drop(first);
        assert!(relay.stage(&mut input()).is_err());
        assert_eq!(relay.byte_budget.load(Ordering::Acquire), used);
        assert!(
            relay
                .stage(json!({"input":"text"}).as_object_mut().unwrap())
                .unwrap()
                .is_none()
        );
        tuning.publish(Default::default());
        let second = relay.stage(&mut input()).unwrap().unwrap();
        assert!(relay.byte_budget.load(Ordering::Acquire) > used);
        drop(second);
        relay.entries.lock().unwrap().clear();
        assert_eq!(relay.byte_budget.load(Ordering::Acquire), used);
        drop(image);
        assert_eq!(relay.byte_budget.load(Ordering::Acquire), 0);
        assert_eq!(relay.downloads.load(Ordering::Acquire), 0);
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
            .image
            .size;
        let mut downloads = Vec::new();
        for index in 0..32 {
            downloads.push(relay.read(&leases[index % 3].tokens[0]).unwrap());
        }
        assert!(relay.read(&leases[0].tokens[0]).is_none());
        let token = leases[0].tokens[0].clone();
        drop(leases);
        relay.entries.lock().unwrap().clear();
        assert!(relay.read(&token).is_none());
        assert_eq!(
            relay.byte_budget.load(Ordering::Acquire),
            3 * bytes_per_image
        );
        let copy = downloads[0].bytes.clone();
        drop(downloads);
        assert_eq!(relay.downloads.load(Ordering::Acquire), 1);
        assert_eq!(relay.byte_budget.load(Ordering::Acquire), bytes_per_image);
        drop(copy);
        assert_eq!(relay.byte_budget.load(Ordering::Acquire), 0);
        assert_eq!(relay.downloads.load(Ordering::Acquire), 0);
        let all = CapacityPermit::acquire(&relay.memory_budget, MAX_BYTES, MAX_BYTES).unwrap();
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
        assert_eq!(relay.byte_budget.load(Ordering::Acquire), 0);
    }

    #[test]
    fn excel_image_urls_reuse_within_scope_without_cross_scope_sharing() {
        let relay = Arc::new(ImageRelay::new(Some("https://images.example.com".into())));
        let mut first = input();
        let lease = relay
            .stage_with_tuning(&mut first, relay.request_tuning(), "scope-a")
            .unwrap()
            .unwrap();
        let bytes = relay.byte_budget.load(Ordering::Acquire);
        let token = lease.tokens[0].clone();
        drop(lease);
        let mut again = input();
        relay
            .stage_with_tuning(&mut again, relay.request_tuning(), "scope-a")
            .unwrap();
        assert_eq!(first, again);
        assert_eq!(relay.byte_budget.load(Ordering::Acquire), bytes);
        assert!(relay.read(&token).is_some());
        let mut other = input();
        relay
            .stage_with_tuning(&mut other, relay.request_tuning(), "scope-b")
            .unwrap();
        assert_ne!(first, other);
        assert_eq!(relay.entries.lock().unwrap().len(), 2);
    }

    #[test]
    fn excel_image_disk_expiry_and_ttl_updates_preserve_live_downloads() {
        let relay = Arc::new(ImageRelay::new(Some("https://images.example.com".into())));
        let limits = gateway_core::routing::RequestTuning {
            excel_image_relay_ttl_minutes: 1,
            excel_image_relay_entries: 1,
            ..Default::default()
        };
        let lease = relay
            .stage_with_tuning(&mut input(), limits, "scope")
            .unwrap()
            .unwrap();
        let token = lease.tokens[0].clone();
        let (path, expiry) = {
            let entries = relay.entries.lock().unwrap();
            let entry = entries.get(&token).unwrap();
            (entry.image.path.to_path_buf(), entry.expires)
        };
        assert!(path.is_file());
        assert!(expiry <= Instant::now() + Duration::from_secs(60));
        let image = relay.read(&token).unwrap();
        let data = image.bytes.clone();
        drop(lease);
        relay
            .entries
            .lock()
            .unwrap()
            .values_mut()
            .for_each(|entry| entry.expires = Instant::now());
        relay.cleanup();
        assert!(relay.read(&token).is_none());
        assert_eq!(relay.entry_budget.load(Ordering::Acquire), 1);
        assert!(
            relay
                .stage_with_tuning(&mut input(), limits, "new-scope")
                .is_err()
        );
        assert!(path.exists());
        assert!(data.starts_with(b"\x89PNG"));
        drop(image);
        assert!(path.exists());
        drop(data);
        assert!(!path.exists());
        assert_eq!(relay.byte_budget.load(Ordering::Acquire), 0);
        assert_eq!(relay.memory_budget.load(Ordering::Acquire), 0);
        assert_eq!(relay.entry_budget.load(Ordering::Acquire), 0);
        assert!(
            relay
                .stage_with_tuning(&mut input(), limits, "new-scope")
                .is_ok()
        );
    }

    #[test]
    fn excel_image_modified_disk_file_is_rejected_without_leaking_download_permits() {
        let relay = Arc::new(ImageRelay::new(Some("https://images.example.com".into())));
        let lease = relay
            .stage_with_tuning(&mut input(), relay.request_tuning(), "scope")
            .unwrap()
            .unwrap();
        let path = relay.entries.lock().unwrap()[&lease.tokens[0]]
            .image
            .path
            .to_path_buf();
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"unexpected")
            .unwrap();
        assert!(relay.read(&lease.tokens[0]).is_none());
        assert_eq!(relay.downloads.load(Ordering::Acquire), 0);
        assert_eq!(relay.memory_budget.load(Ordering::Acquire), 0);
    }
}
