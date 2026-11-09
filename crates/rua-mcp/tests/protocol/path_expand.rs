use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct HomeEnvGuard {
    home: Option<OsString>,
    userprofile: Option<OsString>,
}

impl HomeEnvGuard {
    fn capture() -> Self {
        Self {
            home: std::env::var_os("HOME"),
            userprofile: std::env::var_os("USERPROFILE"),
        }
    }
}

impl Drop for HomeEnvGuard {
    fn drop(&mut self) {
        restore_env("HOME", &self.home);
        restore_env("USERPROFILE", &self.userprofile);
    }
}

fn restore_env(key: &str, value: &Option<OsString>) {
    unsafe {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

#[test]
fn expand_tilde_accepts_windows_separator() {
    let _lock = env_lock().lock().expect("env lock");
    let _guard = HomeEnvGuard::capture();
    let home = tempfile::tempdir().expect("temp home");
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::remove_var("USERPROFILE");
    }

    let expanded = rua_mcp::path_expand::expand_tilde("~\\tools\\node.exe");

    assert_eq!(
        std::path::PathBuf::from(expanded),
        home.path().join("tools\\node.exe")
    );
}
