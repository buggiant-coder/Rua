use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rua_config::encrypted_io::EncryptedFileIO;
use rua_config::EnvStore;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
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

fn make_envs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[test]
fn empty_state_returns_empty_map() {
    let dir = TempDir::new("env-store");
    let store = EnvStore::new(dir.path().to_path_buf());
    assert!(store.list().unwrap().is_empty());
    assert_eq!(store.get("ANYKEY").unwrap(), None);
}

#[test]
fn save_then_list_round_trips() {
    let dir = TempDir::new("env-store");
    let store = EnvStore::new(dir.path().to_path_buf());

    let envs = make_envs(&[("OPENAI_API_KEY", "sk-deadbeef-12345"), ("PORT", "8080")]);
    store.save(envs.clone()).unwrap();

    let listed = store.list().unwrap();
    assert_eq!(listed, envs);
}

#[test]
fn encrypted_env_store_round_trips_without_plaintext_leakage() {
    let dir = TempDir::new("env-store-encrypted");
    let key = [7_u8; 32];
    let store = EnvStore::with_key(dir.path().to_path_buf(), key);

    store
        .save(make_envs(&[
            ("OPENAI_API_KEY", "sk-encrypted-secret-12345"),
            ("UNICODE_SECRET", "你好世界"),
        ]))
        .unwrap();

    let path = dir.path().join("envs.json");
    assert!(
        EncryptedFileIO::is_encrypted_file(&path).unwrap(),
        "EnvStore::with_key must write envs.json in the encrypted-at-rest format"
    );
    let raw = std::fs::read(&path).unwrap();
    let raw_text = String::from_utf8_lossy(&raw);
    assert!(!raw_text.contains("sk-encrypted-secret-12345"));
    assert!(!raw_text.contains("你好世界"));

    let same_key_store = EnvStore::with_key(dir.path().to_path_buf(), key);
    let listed = same_key_store.list().unwrap();
    assert_eq!(
        listed.get("OPENAI_API_KEY"),
        Some(&"sk-encrypted-secret-12345".to_string())
    );
    assert_eq!(listed.get("UNICODE_SECRET"), Some(&"你好世界".to_string()));

    let wrong_key_store = EnvStore::with_key(dir.path().to_path_buf(), [8_u8; 32]);
    wrong_key_store
        .list()
        .expect_err("wrong key must not silently decrypt env secrets");
}

#[test]
fn save_is_full_replacement() {
    let dir = TempDir::new("env-store");
    let store = EnvStore::new(dir.path().to_path_buf());
    store.save(make_envs(&[("A", "1"), ("B", "2")])).unwrap();
    store.save(make_envs(&[("C", "3")])).unwrap();

    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed.get("C"), Some(&"3".to_string()));
}

#[test]
fn delete_returns_presence_flag() {
    let dir = TempDir::new("env-store");
    let store = EnvStore::new(dir.path().to_path_buf());
    store.save(make_envs(&[("A", "1"), ("B", "2")])).unwrap();

    assert!(store.delete("A").unwrap());
    assert!(!store.delete("A").unwrap());
    assert_eq!(store.list().unwrap().len(), 1);
}

#[test]
fn list_masked_redacts_values() {
    let dir = TempDir::new("env-store");
    let store = EnvStore::new(dir.path().to_path_buf());
    store
        .save(make_envs(&[(
            "OPENAI_API_KEY",
            "sk-thisisasecretkey-12345",
        )]))
        .unwrap();

    let masked = store.list_masked().unwrap();
    let value = masked.get("OPENAI_API_KEY").unwrap();
    assert_ne!(value, "sk-thisisasecretkey-12345");
    assert!(value.contains("****"), "expected mask, got {value}");

    let raw = store.list().unwrap();
    assert_eq!(
        raw.get("OPENAI_API_KEY"),
        Some(&"sk-thisisasecretkey-12345".to_string())
    );
}

#[test]
fn atomic_write_leaves_no_temp_sibling() {
    let dir = TempDir::new("env-store");
    let store = EnvStore::new(dir.path().to_path_buf());
    store.save(make_envs(&[("FOO", "bar")])).unwrap();

    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    for n in &names {
        assert!(!n.starts_with(".envs.json.tmp"), "leftover temp: {n}");
    }
    assert!(names.iter().any(|n| n == "envs.json"));
}

#[cfg(unix)]
#[test]
fn save_applies_chmod_600() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("env-store");
    let store = EnvStore::new(dir.path().to_path_buf());
    store
        .save(make_envs(&[("OPENAI_API_KEY", "sk-secret")]))
        .unwrap();

    let path = dir.path().join("envs.json");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();

    assert_eq!(
        mode & 0o777,
        0o600,
        "expected chmod 600 on envs.json, got 0o{:o}",
        mode & 0o777
    );

    store.delete("OPENAI_API_KEY").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "chmod 600 not preserved after delete");
}
