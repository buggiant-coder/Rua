use rua_config::dirs::Paths;
use rua_doctor::registry::{run_all_checks, CheckContext, CheckStatus};

#[test]
fn log_writable_pass_when_dir_exists_and_writable() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path().to_path_buf();
    std::fs::create_dir_all(wd.join("logs")).unwrap();

    let paths = Paths {
        rua_root: wd.clone(),
        secret_dir: wd.join(".secret"),
        cache_dir: wd.join(".cache"),
    };
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let check = results
        .iter()
        .find(|r| r.name == "app_log_writable")
        .expect("app_log_writable check must be registered");
    assert_eq!(check.status, CheckStatus::Pass);

    let leftover: Vec<_> = std::fs::read_dir(wd.join("logs"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with(".rua-writable-test-"))
                .unwrap_or(false)
        })
        .collect();
    assert!(leftover.is_empty(), "temp probe files must be cleaned up");
}

#[cfg(unix)]
#[test]
fn log_writable_fail_when_readonly() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path().to_path_buf();
    let log_dir = wd.join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::set_permissions(&log_dir, std::fs::Permissions::from_mode(0o444)).unwrap();

    let paths = Paths {
        rua_root: wd.clone(),
        secret_dir: wd.join(".secret"),
        cache_dir: wd.join(".cache"),
    };
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let check = results
        .iter()
        .find(|r| r.name == "app_log_writable")
        .expect("app_log_writable check must be registered");
    assert_eq!(check.status, CheckStatus::Fail);
    assert!(
        check.message.contains("not writable"),
        "fail message must mention 'not writable', got: {}",
        check.message
    );

    std::fs::set_permissions(&log_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn log_writable_fail_when_dir_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path().to_path_buf();

    let paths = Paths {
        rua_root: wd.clone(),
        secret_dir: wd.join(".secret"),
        cache_dir: wd.join(".cache"),
    };
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let check = results
        .iter()
        .find(|r| r.name == "app_log_writable")
        .expect("app_log_writable check must be registered");
    assert_eq!(check.status, CheckStatus::Fail);
}
