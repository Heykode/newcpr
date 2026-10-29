//! 二进制与 Web 制品的交易式替换、跨文件系统移动与回滚。

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gateway_core::error::ErrorSource;

use super::archive::ExtractedRelease;
use super::{OperationError, SystemUpdateConfig, conflict, internal};

pub(crate) fn replace_release_files(
    executable: &Path,
    web_dist: &Path,
    extracted: ExtractedRelease,
) -> Result<(), OperationError> {
    let web_backup = backup_path_for(web_dist);
    let web_replaced = if let Some(new_web) = extracted.web_dist_dir {
        replace_dir(web_dist, &web_backup, &new_web)?;
        true
    } else {
        false
    };

    let binary_backup = backup_path_for(executable);
    if binary_backup.exists()
        && let Err(error) = fs::remove_file(&binary_backup)
    {
        let mut rollback = Vec::new();
        if web_replaced {
            collect_rollback_error(
                &mut rollback,
                "restore web assets",
                restore_dir(web_dist, &web_backup),
            );
        }
        return Err(error_with_rollback("无法移除旧的程序备份", error, rollback));
    }
    if let Err(error) = move_file(executable, &binary_backup) {
        let mut rollback = Vec::new();
        if web_replaced {
            collect_rollback_error(
                &mut rollback,
                "restore web assets",
                restore_dir(web_dist, &web_backup),
            );
        }
        return Err(error_with_rollback("无法备份程序文件", error, rollback));
    }
    if let Err(error) = move_file(&extracted.binary_path, executable) {
        let mut rollback = Vec::new();
        if executable.exists() {
            collect_rollback_error(
                &mut rollback,
                "remove partial replacement binary",
                fs::remove_file(executable),
            );
        }
        collect_rollback_error(
            &mut rollback,
            "restore previous binary",
            move_file(&binary_backup, executable),
        );
        if web_replaced {
            collect_rollback_error(
                &mut rollback,
                "restore web assets",
                restore_dir(web_dist, &web_backup),
            );
        }
        return Err(error_with_rollback("无法替换程序文件", error, rollback));
    }
    Ok(())
}

pub(crate) fn rollback_release(config: &SystemUpdateConfig) -> Result<(), OperationError> {
    let web_dist = config.web_dist_dir()?;
    let executable = config.executable_path()?;
    let binary_backup = backup_path_for(&executable);
    if !binary_backup.exists() {
        return Err(conflict("没有可用于回滚的程序备份"));
    }
    swap_file(&executable, &binary_backup)
        .map_err(|error| internal("程序文件回滚失败").with_source(error))?;

    let web_backup = backup_path_for(web_dist);
    if web_backup.exists()
        && let Err(error) = swap_dir(web_dist, &web_backup)
    {
        let mut rollback = Vec::new();
        collect_rollback_error(
            &mut rollback,
            "restore binary after web rollback failure",
            swap_file(&executable, &binary_backup),
        );
        return Err(error_with_rollback("管理页面文件回滚失败", error, rollback));
    }
    Ok(())
}

fn replace_dir(current: &Path, backup: &Path, replacement: &Path) -> Result<(), OperationError> {
    if backup.exists() {
        fs::remove_dir_all(backup)
            .map_err(|error| internal("无法移除旧的管理页面文件备份").with_source(error))?;
    }
    if current.exists() {
        move_dir(current, backup)
            .map_err(|error| internal("无法备份管理页面文件").with_source(error))?;
    }
    if let Err(error) = move_dir(replacement, current) {
        let mut rollback = Vec::new();
        if backup.exists() {
            collect_rollback_error(
                &mut rollback,
                "restore previous web assets",
                restore_dir(current, backup),
            );
        }
        return Err(error_with_rollback("无法替换管理页面文件", error, rollback));
    }
    Ok(())
}

fn swap_file(current: &Path, backup: &Path) -> io::Result<()> {
    if !current.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("current binary is missing: {}", current.display()),
        ));
    }
    let swap = swap_path_for(current);
    if swap.exists() {
        fs::remove_file(&swap)?;
    }
    super::swap_paths::swap_paths(current, backup, &swap, move_file)
}

fn swap_dir(current: &Path, backup: &Path) -> io::Result<()> {
    if !current.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("current web directory is missing: {}", current.display()),
        ));
    }
    let swap = swap_path_for(current);
    if swap.exists() {
        fs::remove_dir_all(&swap)?;
    }
    super::swap_paths::swap_paths(current, backup, &swap, move_dir)
}

fn restore_dir(current: &Path, backup: &Path) -> io::Result<()> {
    if current.exists() {
        fs::remove_dir_all(current)?;
    }
    if backup.exists() {
        move_dir(backup, current)?;
    }
    Ok(())
}

fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            if let Err(copy_error) = fs::copy(from, to) {
                let _ = fs::remove_file(to);
                return Err(copy_error);
            }
            fs::remove_file(from)
        }
        Err(error) => Err(error),
    }
}

fn move_dir(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            if let Err(copy_error) = copy_dir_all(from, to) {
                let _ = fs::remove_dir_all(to);
                return Err(copy_error);
            }
            fs::remove_dir_all(from)
        }
        Err(error) => Err(error),
    }
}

fn copy_dir_all(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = to.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target)?;
            fs::set_permissions(&target, entry.metadata()?.permissions())?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported file type in {}", entry.path().display()),
            ));
        }
    }
    Ok(())
}

fn backup_path_for(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_os_string();
    backup.push(".backup");
    PathBuf::from(backup)
}

fn swap_path_for(path: &Path) -> PathBuf {
    let mut swap = path.as_os_str().to_os_string();
    swap.push(".rollback-swap");
    PathBuf::from(swap)
}

fn collect_rollback_error(errors: &mut Vec<String>, action: &'static str, result: io::Result<()>) {
    if let Err(error) = result {
        errors.push(format!("{action}: {error}"));
    }
}

fn error_with_rollback(
    context: &'static str,
    error: impl Error + Send + Sync + 'static,
    rollback_errors: Vec<String>,
) -> OperationError {
    if rollback_errors.is_empty() {
        return internal(context).with_source(error);
    }
    internal(format!("{context}，且无法恢复原文件")).with_source(
        ErrorSource::new(error).with_cleanup(io::Error::other(rollback_errors.join("; "))),
    )
}
