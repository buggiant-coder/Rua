use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use serde_json::{json, Value};

use rua_config::{atomic_write_json, SettingsStore};

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

fn read_settings(dir: &Path) -> Value {
    let bytes = std::fs::read(dir.join("settings.json")).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn empty_state_returns_empty_map() {
    let dir = TempDir::new("settings");
    let store = SettingsStore::new(dir.path().to_path_buf());
    assert!(store.load().unwrap().is_empty());
    assert_eq!(store.get("language").unwrap(), None);
}

#[test]
fn set_then_get_round_trips() {
    let dir = TempDir::new("settings");
    let store = SettingsStore::new(dir.path().to_path_buf());

    store.set("language", json!("zh")).unwrap();
    assert_eq!(store.get("language").unwrap(), Some(json!("zh")));

    let raw = read_settings(dir.path());
    assert_eq!(raw["language"], json!("zh"));
}

#[test]
fn set_preserves_unrelated_fields() {
    let dir = TempDir::new("settings");

    std::fs::write(
        dir.path().join("settings.json"),
        serde_json::to_vec_pretty(&json!({
            "telemetry": {"enabled": false},
            "language": "en"
        }))
        .unwrap(),
    )
    .unwrap();

    let store = SettingsStore::new(dir.path().to_path_buf());
    store.set("language", json!("ja")).unwrap();

    let raw = read_settings(dir.path());
    assert_eq!(raw["language"], json!("ja"));
    assert_eq!(raw["telemetry"], json!({"enabled": false}));
}

#[test]
fn delete_removes_key_and_reports_presence() {
    let dir = TempDir::new("settings");
    let store = SettingsStore::new(dir.path().to_path_buf());
    store.set("language", json!("en")).unwrap();

    assert!(store.delete("language").unwrap());
    assert_eq!(store.get("language").unwrap(), None);
    assert!(!store.delete("language").unwrap());
}

#[test]
fn replace_all_overwrites_document() {
    let dir = TempDir::new("settings");
    let store = SettingsStore::new(dir.path().to_path_buf());
    store.set("language", json!("zh")).unwrap();
    store.set("theme", json!("dark")).unwrap();

    let mut new_doc = serde_json::Map::new();
    new_doc.insert("language".into(), json!("en"));
    store.replace_all(new_doc).unwrap();

    let raw = read_settings(dir.path());
    assert_eq!(raw["language"], json!("en"));
    assert!(raw.get("theme").is_none());
}

#[test]
fn atomic_write_leaves_no_temp_sibling() {
    let dir = TempDir::new("settings");
    let store = SettingsStore::new(dir.path().to_path_buf());
    store.set("language", json!("en")).unwrap();

    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    for n in &names {
        assert!(!n.starts_with(".settings.json.tmp"), "leftover temp: {n}");
    }
    assert!(names.iter().any(|n| n == "settings.json"));
}

#[test]
fn atomic_write_supports_concurrent_writers_to_same_path() {
    let dir = TempDir::new("settings-atomic-concurrent");
    let path = dir.path().join("settings.json");
    let barrier = Arc::new(Barrier::new(16));
    let mut workers = Vec::new();

    for i in 0..16 {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            atomic_write_json(&path, &json!({ "writer": i }))
        }));
    }

    for worker in workers {
        worker.join().unwrap().unwrap();
    }

    let raw = read_settings(dir.path());
    assert!(raw.get("writer").and_then(|value| value.as_u64()).is_some());
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["settings.json".to_string()]);
}
