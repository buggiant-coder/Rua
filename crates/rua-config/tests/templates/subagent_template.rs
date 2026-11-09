use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use rua_config::SubagentTemplateStore;

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

fn template(id: &str, name: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "description": "",
        "system_prompt": "",
        "tools": [],
        "visible_to": [],
        "builtin": false
    })
}

#[test]
fn empty_state_returns_empty_list() {
    let dir = TempDir::new("subagent-tpl");
    let store = SubagentTemplateStore::new(dir.path().to_path_buf());
    assert!(store.list().unwrap().is_empty());
    assert!(store.get("missing").unwrap().is_none());
}

#[test]
fn create_then_get_round_trips() {
    let dir = TempDir::new("subagent-tpl");
    let store = SubagentTemplateStore::new(dir.path().to_path_buf());
    assert!(store.create(template("tpl-1", "First")).unwrap());

    let fetched = store.get("tpl-1").unwrap().expect("present");
    assert_eq!(fetched["name"], json!("First"));

    assert_eq!(store.list().unwrap().len(), 1);
}

#[test]
fn create_rejects_missing_id_field() {
    let dir = TempDir::new("subagent-tpl");
    let store = SubagentTemplateStore::new(dir.path().to_path_buf());
    let err = store.create(json!({"name": "no-id"})).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn create_rejects_duplicate_id() {
    let dir = TempDir::new("subagent-tpl");
    let store = SubagentTemplateStore::new(dir.path().to_path_buf());
    assert!(store.create(template("dup", "first")).unwrap());
    assert!(!store.create(template("dup", "second")).unwrap());
}

#[test]
fn update_replaces_template_by_id() {
    let dir = TempDir::new("subagent-tpl");
    let store = SubagentTemplateStore::new(dir.path().to_path_buf());
    store.create(template("tpl-1", "Old")).unwrap();

    assert!(store
        .update("tpl-1", json!({"name": "New", "tools": ["echo"]}))
        .unwrap());
    let fetched = store.get("tpl-1").unwrap().unwrap();
    assert_eq!(fetched["name"], json!("New"));
    assert_eq!(fetched["id"], json!("tpl-1"));
    assert_eq!(fetched["tools"], json!(["echo"]));

    assert!(!store.update("missing", template("missing", "x")).unwrap());
}

#[test]
fn delete_returns_presence_flag() {
    let dir = TempDir::new("subagent-tpl");
    let store = SubagentTemplateStore::new(dir.path().to_path_buf());
    store.create(template("tpl-1", "First")).unwrap();
    assert!(store.delete("tpl-1").unwrap());
    assert!(!store.delete("tpl-1").unwrap());
}

#[test]
fn atomic_write_leaves_no_temp_sibling() {
    let dir = TempDir::new("subagent-tpl");
    let store = SubagentTemplateStore::new(dir.path().to_path_buf());
    store.create(template("tpl-1", "x")).unwrap();
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    for n in &names {
        assert!(
            !n.starts_with(".subagent_templates.json.tmp"),
            "leftover temp: {n}"
        );
    }
    assert!(names.iter().any(|n| n == "subagent_templates.json"));
}
