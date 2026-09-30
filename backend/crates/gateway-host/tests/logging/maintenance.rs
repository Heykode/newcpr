use super::*;

#[test]
fn log_cleanup_deletes_only_closed_old_managed_files() {
    if env::var_os(CHILD_PROCESS_ENV).is_none() {
        let output = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "logging::maintenance::log_cleanup_deletes_only_closed_old_managed_files",
            ])
            .env(CHILD_PROCESS_ENV, "1")
            .env("RUST_LOG", "off")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut config = logging_config(directory.clone(), false);
    config.runtime_data_dir = directory.join("runtime");
    let bundle = runtime.block_on(gateway_host::initialize(config)).unwrap();
    let maintenance = bundle.log_file_maintenance();
    let names = [
        "codex-proxy-rs-application.2020-01-01.log.gz",
        "codex-proxy-rs-application.2020-01-01.log",
        "database.dump",
        "other.2020-01-01.log.gz",
        "codex-proxy-rs-application.2020-01-01.log.gz.tmp",
        "codex-proxy-rs-application.2020-01-01.bad.log.gz",
    ];
    for name in names {
        let path = directory.join(name);
        fs::write(&path, "fixture").unwrap();
        filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(1_577_836_800, 0))
            .unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        directory.join("database.dump"),
        directory.join("codex-proxy-rs-application.2020-01-02.log.gz"),
    )
    .unwrap();
    assert!(runtime.block_on(maintenance.bytes()).unwrap() > 0);
    let result = runtime
        .block_on(maintenance.clean(chrono::Utc::now() - chrono::Duration::days(7), 100))
        .unwrap();
    assert_eq!(result.removed, 1);
    assert!(result.complete);
    assert!(!directory.join(names[0]).exists());
    for name in &names[1..] {
        assert!(directory.join(name).exists());
    }
    let today = chrono::Utc::now().date_naive();
    assert!(
        directory
            .join(format!("{APPLICATION_LOG_FILE_PREFIX}{today}.log"))
            .exists()
    );
}
