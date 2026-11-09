use std::ffi::OsString;
use std::fs;
use std::sync::{Mutex, OnceLock};

use rua_doctor::windows_shell::select_windows_shell_for_path;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvRestore {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvRestore {
    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, previous }
    }

    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        unsafe {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
fn windows_shell_prefers_git_bash_on_path() {
    let _lock = env_lock().lock().expect("env lock");
    let _restore = EnvRestore::remove("RUA_WINDOWS_SHELL");
    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("Git").join("bin");
    fs::create_dir_all(&bin).expect("mkdir");
    let bash = bin.join("bash.exe");
    fs::write(&bash, b"").expect("write bash");
    let path = OsString::from(bin.to_string_lossy().into_owned());

    let (selected, kind) = select_windows_shell_for_path(Some(&path));

    assert_eq!(selected, bash.to_string_lossy());
    assert_eq!(kind, "git-bash");
}

#[test]
fn windows_shell_falls_back_to_cmd_without_git_bash() {
    let _lock = env_lock().lock().expect("env lock");
    let _restore = EnvRestore::remove("RUA_WINDOWS_SHELL");
    let (selected, kind) = select_windows_shell_for_path(Some(&OsString::new()));

    assert_eq!(selected, "cmd.exe");
    assert_eq!(kind, "cmd");
}

#[test]
fn windows_shell_uses_powershell_when_git_bash_missing() {
    let _lock = env_lock().lock().expect("env lock");
    let _restore = EnvRestore::remove("RUA_WINDOWS_SHELL");
    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("PowerShell").join("7");
    fs::create_dir_all(&bin).expect("mkdir");
    let pwsh = bin.join("pwsh.exe");
    fs::write(&pwsh, b"").expect("write pwsh");
    let path = OsString::from(bin.to_string_lossy().into_owned());

    let (selected, kind) = select_windows_shell_for_path(Some(&path));

    assert_eq!(selected, pwsh.to_string_lossy());
    assert_eq!(kind, "powershell");
}

#[test]
fn windows_shell_supports_explicit_wsl() {
    let _lock = env_lock().lock().expect("env lock");
    let temp = tempfile::tempdir().expect("tempdir");
    let wsl = temp.path().join("wsl.exe");
    fs::write(&wsl, b"").expect("write wsl");
    let _restore = EnvRestore::set("RUA_WINDOWS_SHELL", &wsl);

    let (selected, kind) = select_windows_shell_for_path(Some(&OsString::new()));

    assert_eq!(selected, wsl.to_string_lossy());
    assert_eq!(kind, "wsl");
}

#[test]
fn windows_shell_does_not_auto_select_wsl_from_path() {
    let _lock = env_lock().lock().expect("env lock");
    let _restore = EnvRestore::remove("RUA_WINDOWS_SHELL");
    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("Windows").join("System32");
    fs::create_dir_all(&bin).expect("mkdir");
    fs::write(bin.join("wsl.exe"), b"").expect("write wsl");
    let path = OsString::from(bin.to_string_lossy().into_owned());

    let (selected, kind) = select_windows_shell_for_path(Some(&path));

    assert_eq!(selected, "cmd.exe");
    assert_eq!(kind, "cmd");
}
