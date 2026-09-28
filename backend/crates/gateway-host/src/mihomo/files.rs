//! Private, atomic on-disk state. Never expose OS errors containing source URLs.

use gateway_admin::model::AdminError;
use std::{
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn prepare_dir(dir: &Path) -> Result<(), AdminError> {
    std::fs::create_dir_all(dir).map_err(|_| AdminError::internal("代理运行目录不可写"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| AdminError::internal("不能保护代理运行目录"))?;
    }
    Ok(())
}

pub(super) fn secret() -> Result<String, AdminError> {
    let mut bytes = [0_u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|_| AdminError::internal("无法生成代理控制器密钥"))?;
    Ok(hex::encode(bytes))
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8], executable: bool) -> Result<(), AdminError> {
    let parent = path
        .parent()
        .ok_or_else(|| AdminError::internal("代理配置路径无效"))?;
    let temp = parent.join(format!(
        ".pending-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(if executable { 0o700 } else { 0o600 });
        }
        #[cfg(not(unix))]
        let _ = executable;
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result.map_err(|_| AdminError::internal("代理配置原子写入失败"))
}
