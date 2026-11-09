use rua_config::dirs::{home_dir, resolve_rua_root, Paths};
use serial_test::serial;
use std::ffi::OsString;

struct HomeEnvGuard {
    home: Option<OsString>,
    userprofile: Option<OsString>,
    homedrive: Option<OsString>,
    homepath: Option<OsString>,
    rua_root: Option<OsString>,
    rua_cache_dir: Option<OsString>,
}

impl HomeEnvGuard {
    fn capture() -> Self {
        Self {
            home: std::env::var_os("HOME"),
            userprofile: std::env::var_os("USERPROFILE"),
            homedrive: std::env::var_os("HOMEDRIVE"),
            homepath: std::env::var_os("HOMEPATH"),
            rua_root: std::env::var_os("RUA_ROOT"),
            rua_cache_dir: std::env::var_os("RUA_CACHE_DIR"),
        }
    }
}

impl Drop for HomeEnvGuard {
    fn drop(&mut self) {
        restore_env("HOME", &self.home);
        restore_env("USERPROFILE", &self.userprofile);
        restore_env("HOMEDRIVE", &self.homedrive);
        restore_env("HOMEPATH", &self.homepath);
        restore_env("RUA_ROOT", &self.rua_root);
        restore_env("RUA_CACHE_DIR", &self.rua_cache_dir);
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
#[serial]
fn explicit_arg_sets_rua_root() {
    let paths = Paths::new(Some("/tmp/test"));
    assert_eq!(paths.rua_root.as_os_str(), "/tmp/test");
}

#[test]
#[serial]
fn rua_root_arg_derives_secret_dir() {
    let paths = Paths::new(Some("/tmp/test"));
    assert_eq!(paths.secret_dir.as_os_str(), "/tmp/test.secret");
}

#[test]
#[serial]
fn rua_root_arg_derives_device_local_cache_dir_outside_root() {
    let _guard = HomeEnvGuard::capture();
    unsafe {
        std::env::remove_var("RUA_CACHE_DIR");
    }

    let paths = Paths::new(Some("/tmp/test"));

    assert_eq!(paths.cache_dir.as_os_str(), "/tmp/test.cache");
    assert!(!paths.cache_dir.starts_with(&paths.rua_root));
}

#[test]
#[serial]
fn distinct_rua_roots_derive_distinct_secret_dirs() {
    let alpha = Paths::new(Some("/tmp/rua-alpha"));
    let beta = Paths::new(Some("/tmp/rua-beta"));

    assert_eq!(alpha.secret_dir.as_os_str(), "/tmp/rua-alpha.secret");
    assert_eq!(beta.secret_dir.as_os_str(), "/tmp/rua-beta.secret");
    assert_ne!(
        alpha.secret_dir, beta.secret_dir,
        "relocated/dev installs must not share the same derived secret dir"
    );
}

#[test]
#[serial]
fn same_rua_root_derives_stable_secret_dir() {
    let first = Paths::new(Some("/tmp/rua-stable"));
    let second = Paths::new(Some("/tmp/rua-stable"));

    assert_eq!(first.secret_dir, second.secret_dir);
    assert_eq!(first.secret_dir.as_os_str(), "/tmp/rua-stable.secret");
}

#[test]
#[serial]
fn distinct_rua_roots_derive_distinct_cache_dirs() {
    let _guard = HomeEnvGuard::capture();
    unsafe {
        std::env::remove_var("RUA_CACHE_DIR");
    }

    let alpha = Paths::new(Some("/tmp/rua-alpha"));
    let beta = Paths::new(Some("/tmp/rua-beta"));

    assert_eq!(alpha.cache_dir.as_os_str(), "/tmp/rua-alpha.cache");
    assert_eq!(beta.cache_dir.as_os_str(), "/tmp/rua-beta.cache");
    assert_ne!(alpha.cache_dir, beta.cache_dir);
}

#[test]
#[serial]
fn explicit_rua_secret_dir_override_wins_over_derived_secret_dir() {
    unsafe {
        std::env::set_var("RUA_SECRET_DIR", "/tmp/rua-explicit-secret");
    }
    let paths = Paths::new(Some("/tmp/rua-data"));
    unsafe {
        std::env::remove_var("RUA_SECRET_DIR");
    }

    assert_eq!(paths.rua_root.as_os_str(), "/tmp/rua-data");
    assert_eq!(
        paths.secret_dir.as_os_str(),
        "/tmp/rua-explicit-secret",
        "explicit secret-dir relocation should isolate secret storage from the data dir"
    );
}

#[test]
#[serial]
fn explicit_rua_secret_dir_supports_tilde_expansion() {
    unsafe {
        std::env::set_var("RUA_SECRET_DIR", "~/rua-secret-relocated");
    }
    let paths = Paths::new(Some("/tmp/rua-data"));
    unsafe {
        std::env::remove_var("RUA_SECRET_DIR");
    }

    assert_eq!(paths.secret_dir, home_dir().join("rua-secret-relocated"));
}

#[test]
#[serial]
fn blank_rua_secret_dir_override_is_ignored() {
    unsafe {
        std::env::set_var("RUA_SECRET_DIR", "   ");
    }
    let paths = Paths::new(Some("/tmp/rua-data"));
    unsafe {
        std::env::remove_var("RUA_SECRET_DIR");
    }

    assert_eq!(paths.secret_dir.as_os_str(), "/tmp/rua-data.secret");
}

#[test]
#[serial]
fn explicit_rua_cache_dir_override_wins_over_derived_cache_dir() {
    let _guard = HomeEnvGuard::capture();
    unsafe {
        std::env::set_var("RUA_CACHE_DIR", "~/rua-runtime-cache");
    }

    let paths = Paths::new(Some("/tmp/rua-data"));

    assert_eq!(paths.cache_dir, home_dir().join("rua-runtime-cache"));
}

#[test]
#[serial]
fn blank_rua_cache_dir_override_is_ignored() {
    let _guard = HomeEnvGuard::capture();
    unsafe {
        std::env::set_var("RUA_CACHE_DIR", "   ");
    }

    let paths = Paths::new(Some("/tmp/rua-data"));

    assert_eq!(paths.cache_dir.as_os_str(), "/tmp/rua-data.cache");
}

#[test]
#[serial]
fn no_argument_or_env_uses_home_default() {
    unsafe {
        std::env::remove_var("RUA_ROOT");
    }
    let paths = Paths::new(None);
    let expected = home_dir().join(".rua");
    assert_eq!(paths.rua_root, expected);
}

#[test]
#[serial]
fn empty_argument_without_env_uses_home_default() {
    unsafe {
        std::env::remove_var("RUA_ROOT");
    }
    let paths = Paths::new(Some("  "));
    let expected = home_dir().join(".rua");
    assert_eq!(paths.rua_root, expected);
}

#[test]
#[serial]
fn tilde_expansion_in_rua_root() {
    let paths = Paths::new(Some("~/my-rua"));
    let expected = home_dir().join("my-rua");
    assert_eq!(paths.rua_root, expected);
}

#[test]
#[serial]
fn home_dir_falls_back_to_userprofile_when_home_is_missing() {
    let _guard = HomeEnvGuard::capture();
    unsafe {
        std::env::remove_var("HOME");
        std::env::remove_var("RUA_ROOT");
        std::env::set_var("USERPROFILE", "/tmp/rua-userprofile-home");
        std::env::remove_var("HOMEDRIVE");
        std::env::remove_var("HOMEPATH");
    }

    assert_eq!(home_dir().as_os_str(), "/tmp/rua-userprofile-home");
    assert_eq!(
        Paths::new(None).rua_root.as_os_str(),
        "/tmp/rua-userprofile-home/.rua"
    );
}

#[test]
#[serial]
fn tilde_expansion_accepts_windows_separator() {
    let paths = Paths::new(Some("~\\my-rua"));
    let expected = home_dir().join("my-rua");
    assert_eq!(paths.rua_root, expected);
}

#[test]
#[serial]
fn rua_root_env_var_used_as_fallback_when_no_cli_arg() {
    unsafe {
        std::env::set_var("RUA_ROOT", "/tmp/from-env");
    }
    let paths = Paths::new(None);
    assert_eq!(
        paths.rua_root.as_os_str(),
        "/tmp/from-env",
        "RUA_ROOT must be honored when no CLI arg given"
    );
    unsafe {
        std::env::remove_var("RUA_ROOT");
    }
}

#[test]
#[serial]
fn cli_arg_takes_priority_over_env_var() {
    unsafe {
        std::env::set_var("RUA_ROOT", "/tmp/from-env");
    }
    let paths = Paths::new(Some("/tmp/from-cli"));
    assert_eq!(
        paths.rua_root.as_os_str(),
        "/tmp/from-cli",
        "explicit CLI arg must beat RUA_ROOT"
    );
    unsafe {
        std::env::remove_var("RUA_ROOT");
    }
}

#[test]
#[serial]
fn resolve_rua_root_funnel_consistent_with_paths_new() {
    unsafe {
        std::env::set_var("RUA_ROOT", "/tmp/funnel-test");
    }
    assert_eq!(resolve_rua_root(None), Paths::new(None).rua_root);
    assert_eq!(
        resolve_rua_root(Some("/tmp/cli-funnel")),
        Paths::new(Some("/tmp/cli-funnel")).rua_root,
    );
    unsafe {
        std::env::remove_var("RUA_ROOT");
    }
}
