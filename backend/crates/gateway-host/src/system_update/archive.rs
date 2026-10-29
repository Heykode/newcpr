//! Release tar.gz 安全解包与制品归一化。

use std::fs;
use std::path::{Component, Path, PathBuf};

use super::{OperationError, internal, invalid};

const APP_BINARY_NAME: &str = "codex-proxy-rs";
const MAX_EXTRACTED_SIZE: u64 = 1024 * 1024 * 1024;
const MAX_ARCHIVE_FILES: usize = 20_000;

#[derive(Debug)]
pub(crate) struct ExtractedRelease {
    pub(crate) binary_path: PathBuf,
    pub(crate) web_dist_dir: Option<PathBuf>,
}

pub(crate) fn extract_release(
    archive_path: &Path,
    temp_dir: &Path,
) -> Result<ExtractedRelease, OperationError> {
    let file = fs::File::open(archive_path)
        .map_err(|error| internal("无法打开更新包").with_source(error))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let binary_path = temp_dir.join(APP_BINARY_NAME);
    let web_dist_dir = temp_dir.join("web-dist");
    let mut found_binary = false;
    let mut found_web = false;
    let mut extracted_size = 0_u64;
    let mut file_count = 0_usize;

    for entry in archive
        .entries()
        .map_err(|error| internal("无法读取更新包").with_source(error))?
    {
        let mut entry =
            entry.map_err(|error| internal("更新包内容损坏，请重新下载").with_source(error))?;
        let path = entry
            .path()
            .map_err(|error| internal("更新包中的文件路径无效").with_source(error))?
            .into_owned();
        if unsafe_archive_path(&path) {
            return Err(invalid("更新包包含不安全的文件路径，已停止更新"));
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }
        file_count = file_count.saturating_add(1);
        extracted_size = extracted_size.saturating_add(entry.header().size().unwrap_or(u64::MAX));
        if file_count > MAX_ARCHIVE_FILES || extracted_size > MAX_EXTRACTED_SIZE {
            return Err(invalid("更新包解压后的大小或文件数量超出限制"));
        }

        if path.file_name().is_some_and(|name| name == APP_BINARY_NAME) {
            if found_binary {
                return Err(invalid("更新包包含重复的程序文件"));
            }
            entry
                .unpack(&binary_path)
                .map_err(|error| internal("无法解压程序文件").with_source(error))?;
            found_binary = true;
            continue;
        }
        if let Some(relative) = web_dist_relative_path(&path) {
            if relative.as_os_str().is_empty() {
                continue;
            }
            let target = web_dist_dir.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| internal("无法创建管理页面目录").with_source(error))?;
            }
            entry
                .unpack(&target)
                .map_err(|error| internal("无法解压管理页面文件").with_source(error))?;
            found_web = true;
        }
    }
    if !found_binary {
        return Err(invalid("更新包缺少网关程序文件"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&binary_path, fs::Permissions::from_mode(0o755))
            .map_err(|error| internal("无法设置程序执行权限").with_source(error))?;
    }
    Ok(ExtractedRelease {
        binary_path,
        web_dist_dir: found_web.then_some(web_dist_dir),
    })
}

fn unsafe_archive_path(path: &Path) -> bool {
    path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
}

fn web_dist_relative_path(path: &Path) -> Option<PathBuf> {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for index in 0..components.len() {
        if components[index] == "web"
            && components
                .get(index + 1)
                .is_some_and(|value| value == "dist")
        {
            return Some(components[index + 2..].iter().collect());
        }
        if components[index] == "dist" {
            return Some(components[index + 1..].iter().collect());
        }
    }
    None
}
