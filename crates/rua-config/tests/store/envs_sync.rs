use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rua_config::envs_sync::set_env_from_envs_json;
use serial_test::serial;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rua-{label}-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
#[serial]
fn envs_json_overrides_os_env() {
    let dir = TempDir::new("envs-sync");
    let envs_path = dir.path().join("envs.json");
    std::fs::write(&envs_path, r#"{"RUA_TEST_ENVS_SYNC":"bar"}"#).unwrap();

    std::env::set_var("RUA_TEST_ENVS_SYNC", "baz");
    assert_eq!(std::env::var("RUA_TEST_ENVS_SYNC").unwrap(), "baz");

    set_env_from_envs_json(dir.path());

    assert_eq!(
        std::env::var("RUA_TEST_ENVS_SYNC").unwrap(),
        "bar",
        "envs.json value must override existing OS env"
    );

    std::env::remove_var("RUA_TEST_ENVS_SYNC");
}

#[test]
fn missing_envs_json_silently_skips() {
    let dir = TempDir::new("envs-sync-missing");
    set_env_from_envs_json(dir.path());
}

#[test]
#[serial]
fn non_string_values_converted_to_json_strings() {
    let dir = TempDir::new("envs-sync-types");
    let envs_path = dir.path().join("envs.json");
    std::fs::write(&envs_path, r#"{"RUA_TEST_NUM":42,"RUA_TEST_BOOL":true}"#).unwrap();

    set_env_from_envs_json(dir.path());

    assert_eq!(std::env::var("RUA_TEST_NUM").unwrap(), "42");
    assert_eq!(std::env::var("RUA_TEST_BOOL").unwrap(), "true");

    std::env::remove_var("RUA_TEST_NUM");
    std::env::remove_var("RUA_TEST_BOOL");
}
