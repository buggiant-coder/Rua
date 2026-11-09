#![cfg(unix)]

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serial_test::serial;
use tempfile::TempDir;

fn set_env(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    unsafe {
        std::env::set_var(key, value);
    }
}

fn remove_env(key: &str) {
    unsafe {
        std::env::remove_var(key);
    }
}

struct EnvRestore {
    path: Option<std::ffi::OsString>,
    shell: Option<std::ffi::OsString>,
    rua_user_shell: Option<std::ffi::OsString>,
}

impl EnvRestore {
    fn capture() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            shell: std::env::var_os("SHELL"),
            rua_user_shell: std::env::var_os("RUA_USER_SHELL"),
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.path {
            Some(value) => set_env("PATH", value),
            None => remove_env("PATH"),
        }
        match &self.shell {
            Some(value) => set_env("SHELL", value),
            None => remove_env("SHELL"),
        }
        match &self.rua_user_shell {
            Some(value) => set_env("RUA_USER_SHELL", value),
            None => remove_env("RUA_USER_SHELL"),
        }
    }
}

fn write_fake_shell(dir: &Path, body: &str) -> io::Result<PathBuf> {
    let path = dir.join("fake-shell");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    let mut perms = std::fs::metadata(&path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms)?;
    Ok(path)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn path_augment_prepends_missing_entries_from_noisy_shell_path() {
    let _restore = EnvRestore::capture();
    let tmp = TempDir::new().expect("tempdir");
    let shell = write_fake_shell(
        tmp.path(),
        r#"printf 'rc noise before\n'
printf '__RUA_PATH_DELIM__%s__RUA_PATH_DELIM__' '/custom/bin:/usr/bin:/custom/bin:/new/bin:'
printf '\nrc noise after\n'"#,
    )
    .expect("fake shell");

    set_env("SHELL", &shell);
    set_env("PATH", "/usr/bin:/bin");

    rua_server::path_augment::augment_from_user_shell().await;

    assert_eq!(
        std::env::var("PATH").expect("PATH"),
        "/custom/bin:/new/bin:/usr/bin:/bin",
        "PATH augmentation should prepend only missing user-shell entries, preserving current PATH"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn path_augment_shell_cannot_control_the_callers_terminal_session() {
    let _restore = EnvRestore::capture();
    let tmp = TempDir::new().expect("tempdir");
    let shell = write_fake_shell(
        tmp.path(),
        r#"session_id=$(ps -o sess= -p $$ | tr -d ' ')
printf '__RUA_PATH_DELIM__/session/%s__RUA_PATH_DELIM__' "$session_id""#,
    )
    .expect("fake shell");

    set_env("SHELL", &shell);
    set_env("PATH", "/usr/bin:/bin");

    rua_server::path_augment::augment_from_user_shell().await;

    let path = std::env::var("PATH").expect("PATH");
    let captured_session = path
        .split(':')
        .find_map(|entry| entry.strip_prefix("/session/"))
        .expect("fake shell session should be captured");
    let caller_session = unsafe { libc::getsid(0) };
    assert_ne!(
        captured_session,
        caller_session.to_string(),
        "PATH discovery shell must run outside the caller's terminal session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn path_augment_timeout_does_not_leave_an_orphaned_shell() {
    let _restore = EnvRestore::capture();
    let tmp = TempDir::new().expect("tempdir");
    let pid_file = tmp.path().join("shell.pid");
    let shell = write_fake_shell(
        tmp.path(),
        &format!("printf '%s' $$ > '{}'; exec sleep 30", pid_file.display()),
    )
    .expect("fake shell");

    set_env("SHELL", &shell);
    set_env("PATH", "/usr/bin:/bin");

    rua_server::path_augment::augment_from_user_shell().await;

    let pid: i32 = std::fs::read_to_string(&pid_file)
        .expect("fake shell should record its pid")
        .parse()
        .expect("recorded pid should be numeric");
    for _ in 0..20 {
        if unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("timed-out PATH discovery shell {pid} was left running");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn path_augment_keeps_path_when_shell_output_has_no_delimiters() {
    let _restore = EnvRestore::capture();
    let tmp = TempDir::new().expect("tempdir");
    let shell = write_fake_shell(tmp.path(), "printf '/custom/bin:/new/bin'").expect("fake shell");

    set_env("SHELL", &shell);
    set_env("PATH", "/usr/bin:/bin");

    rua_server::path_augment::augment_from_user_shell().await;

    let path = std::env::var("PATH").expect("PATH");
    assert!(
        !path.split(':').any(|entry| entry == "/custom/bin"),
        "malformed shell output must not be trusted, got {path}"
    );
    assert!(
        !path.split(':').any(|entry| entry == "/new/bin"),
        "malformed shell output must not be trusted, got {path}"
    );
    assert!(
        path.split(':').any(|entry| entry == "/usr/bin"),
        "PATH augmentation must preserve /usr/bin, got {path}"
    );
    assert!(
        path.split(':').any(|entry| entry == "/bin"),
        "PATH augmentation must preserve /bin, got {path}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn path_augment_skips_bad_shell_candidates_without_losing_current_path() {
    let _restore = EnvRestore::capture();
    set_env("SHELL", "/definitely/missing/rua-shell");
    set_env("PATH", "/usr/bin:/bin");

    rua_server::path_augment::augment_from_user_shell().await;
    let path = std::env::var("PATH").expect("PATH");
    assert!(
        path.split(':').any(|entry| entry == "/usr/bin"),
        "PATH augmentation must preserve /usr/bin, got {path}"
    );
    assert!(
        path.split(':').any(|entry| entry == "/bin"),
        "PATH augmentation must preserve /bin, got {path}"
    );

    let tmp = TempDir::new().expect("tempdir");
    let shell = write_fake_shell(tmp.path(), "exit 7").expect("fake shell");
    set_env("SHELL", &shell);

    rua_server::path_augment::augment_from_user_shell().await;
    let path = std::env::var("PATH").expect("PATH");
    assert!(
        path.split(':').any(|entry| entry == "/usr/bin"),
        "PATH augmentation must preserve /usr/bin after a failing candidate, got {path}"
    );
    assert!(
        path.split(':').any(|entry| entry == "/bin"),
        "PATH augmentation must preserve /bin after a failing candidate, got {path}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn path_augment_without_shell_is_a_noop() {
    let _restore = EnvRestore::capture();
    remove_env("SHELL");
    remove_env("RUA_USER_SHELL");
    set_env("PATH", "/usr/bin:/bin");

    rua_server::path_augment::augment_from_user_shell().await;

    let path = std::env::var("PATH").expect("PATH");
    assert!(
        path.split(':').any(|entry| entry == "/usr/bin"),
        "PATH augmentation must preserve /usr/bin, got {path}"
    );
    assert!(
        path.split(':').any(|entry| entry == "/bin"),
        "PATH augmentation must preserve /bin, got {path}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn path_augment_uses_explicit_user_shell_when_shell_is_missing() {
    let _restore = EnvRestore::capture();
    let tmp = TempDir::new().expect("tempdir");
    let shell = write_fake_shell(
        tmp.path(),
        r#"printf '__RUA_PATH_DELIM__%s__RUA_PATH_DELIM__' '/nvm/bin:/usr/bin:/bin'"#,
    )
    .expect("fake shell");

    remove_env("SHELL");
    set_env("RUA_USER_SHELL", &shell);
    set_env("PATH", "/usr/bin:/bin");

    rua_server::path_augment::augment_from_user_shell().await;

    assert_eq!(
        std::env::var("PATH").expect("PATH"),
        "/nvm/bin:/usr/bin:/bin",
        "explicit fallback shell should recover user-managed tool paths when SHELL is absent"
    );
}
