use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use gateway_admin::{
    model::log_cleanup::CleanupBatch,
    ports::{
        log_cleanup::LogFileMaintenance,
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
    },
};
use gateway_core::time::DeploymentTimeZone;
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

pub(crate) struct FileMaintenance {
    directory: PathBuf,
    timezone: DeploymentTimeZone,
}

impl FileMaintenance {
    pub(crate) fn new(directory: PathBuf, timezone: DeploymentTimeZone) -> Self {
        Self {
            directory,
            timezone,
        }
    }
}

fn unavailable(_: impl std::fmt::Display) -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "log files",
        "运行日志目录暂不可用",
    )
}

fn managed_name(name: &str) -> Option<(NaiveDate, bool)> {
    let body = [
        super::APPLICATION_LOG_FILE_PREFIX,
        super::OAUTH_RECOVERY_LOG_FILE_PREFIX,
        super::REQUEST_DUMP_LOG_FILE_PREFIX,
    ]
    .into_iter()
    .find_map(|prefix| {
        name.strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('.'))
    })?;
    let (body, closed) = body
        .strip_suffix(".log.gz")
        .map(|s| (s, true))
        .or_else(|| body.strip_suffix(".log").map(|s| (s, false)))?;
    let (date, segment) = body
        .split_once('.')
        .map_or((body, None), |(d, s)| (d, Some(s)));
    if segment.is_some_and(|s| s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit())) {
        return None;
    }
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .map(|date| (date, closed))
}

async fn directory(path: &Path) -> AdminStoreResult<Option<tokio::fs::ReadDir>> {
    // Refuse symlinks anywhere in the configured path, not just matching filenames.
    for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        match tokio::fs::symlink_metadata(ancestor).await {
            Ok(meta) if meta.file_type().is_symlink() => return Err(unavailable("symlink")),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(unavailable(e)),
        }
    }
    tokio::fs::read_dir(path)
        .await
        .map(Some)
        .map_err(unavailable)
}

#[async_trait]
impl LogFileMaintenance for FileMaintenance {
    async fn bytes(&self) -> AdminStoreResult<u64> {
        let Some(mut entries) = directory(&self.directory).await? else {
            return Ok(0);
        };
        let mut total = 0_u64;
        let started = Instant::now();
        while let Some(entry) = entries.next_entry().await.map_err(unavailable)? {
            if started.elapsed().as_secs() >= 5 {
                return Err(unavailable("scan budget"));
            }
            if managed_name(&entry.file_name().to_string_lossy()).is_none() {
                continue;
            }
            let metadata = match tokio::fs::symlink_metadata(entry.path()).await {
                Ok(metadata) => metadata,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(unavailable(e)),
            };
            if metadata.file_type().is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    total = total.saturating_add(metadata.blocks().saturating_mul(512));
                }
                #[cfg(not(unix))]
                {
                    total = total.saturating_add(metadata.len());
                }
            }
        }
        Ok(total)
    }

    async fn clean(&self, cutoff: DateTime<Utc>, limit: usize) -> AdminStoreResult<CleanupBatch> {
        let Some(mut entries) = directory(&self.directory).await? else {
            return Ok(CleanupBatch {
                removed: 0,
                complete: true,
            });
        };
        let mut removed = 0;
        let started = Instant::now();
        while let Some(entry) = entries.next_entry().await.map_err(unavailable)? {
            if started.elapsed().as_secs() >= 3 {
                return Err(unavailable("scan budget"));
            }
            let Some((date, true)) = managed_name(&entry.file_name().to_string_lossy()) else {
                continue;
            };
            if date > self.timezone.local(cutoff).date_naive() {
                continue;
            }
            let metadata = match tokio::fs::symlink_metadata(entry.path()).await {
                Ok(metadata) => metadata,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(unavailable(e)),
            };
            if !metadata.file_type().is_file() {
                continue;
            }
            let modified: DateTime<Utc> = metadata.modified().map_err(unavailable)?.into();
            if modified >= cutoff {
                continue;
            }
            match tokio::fs::remove_file(entry.path()).await {
                Ok(()) => removed += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(unavailable(e)),
            }
            if removed >= limit as u64 {
                return Ok(CleanupBatch {
                    removed,
                    complete: false,
                });
            }
        }
        Ok(CleanupBatch {
            removed,
            complete: true,
        })
    }
}
