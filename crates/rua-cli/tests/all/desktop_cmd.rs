use std::sync::{Mutex, OnceLock};

use rua_cli::desktop::{candidate_paths_for, find_app_in};
use tempfile::TempDir;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct HomeGuard {
    old_home: Option<std::ffi::OsString>,
    _tmp: TempDir,
}

impl HomeGuard {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("temp home");
        let old_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        Self {
            old_home,
            _tmp: tmp,
        }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.old_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

#[test]
fn macos_desktop_candidate_paths_prefer_user_applications_before_system_applications() {
    let _guard = env_lock().lock().expect("HOME mutation lock");
    let home = HomeGuard::new();

    let paths = candidate_paths_for("macos", Some(home._tmp.path().to_path_buf()), None, None);

    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0], home._tmp.path().join("Applications/Rua.app"));
    assert_eq!(paths[1], std::path::PathBuf::from("/Applications/Rua.app"));
}

#[test]
fn macos_desktop_find_app_uses_first_existing_user_install_without_launching() {
    let _guard = env_lock().lock().expect("HOME mutation lock");
    let home = HomeGuard::new();
    let user_app = home._tmp.path().join("Applications/Rua.app");
    std::fs::create_dir_all(&user_app).expect("create fake app bundle dir");

    assert_eq!(
        find_app_in(
            "macos",
            [
                user_app.clone(),
                std::path::PathBuf::from("/Applications/Rua.app")
            ]
        ),
        Some(user_app)
    );
}

#[test]
fn windows_desktop_candidates_prefer_the_cli_install_directory() {
    let paths = candidate_paths_for(
        "windows",
        None,
        Some(std::path::PathBuf::from("/Users/test/AppData/Local")),
        Some(std::path::PathBuf::from("/Program Files/Rua/rua.exe")),
    );

    assert_eq!(
        paths,
        vec![
            std::path::PathBuf::from("/Program Files/Rua").join("Rua.exe"),
            std::path::PathBuf::from("/Users/test/AppData/Local")
                .join("Rua")
                .join("Rua.exe"),
            std::path::PathBuf::from("/Users/test/AppData/Local")
                .join("Programs")
                .join("Rua")
                .join("Rua.exe"),
        ]
    );
}

#[test]
fn windows_desktop_find_app_requires_an_executable_file() {
    let temp = tempfile::tempdir().expect("temp install");
    let missing = temp.path().join("missing/Rua.exe");
    let installed = temp.path().join("Rua.exe");
    std::fs::write(&installed, b"fixture").expect("write fake executable");

    assert_eq!(
        find_app_in("windows", [missing, installed.clone()]),
        Some(installed)
    );
}
