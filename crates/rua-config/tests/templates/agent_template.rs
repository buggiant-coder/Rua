use std::path::{Path, PathBuf};

use rua_config::{AgentTemplate, AgentTemplateStore};

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

fn sample_template(name: &str) -> AgentTemplate {
    AgentTemplate {
        name: name.to_string(),
        description: format!("desc for {name}"),
        prompt_files: vec!["AGENT_SAFETY.md".into()],
        builtin: false,
        created_at: String::new(),
        updated_at: String::new(),
        extra: serde_json::Map::new(),
    }
}

#[test]
fn empty_state_returns_empty_list() {
    let dir = TempDir::new("agent-tpl");
    let store = AgentTemplateStore::new(dir.path().to_path_buf());
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn create_then_get_round_trips() {
    let dir = TempDir::new("agent-tpl");
    let store = AgentTemplateStore::new(dir.path().to_path_buf());
    assert!(store.create("custom", sample_template("Custom")).unwrap());

    let fetched = store.get("custom").unwrap().expect("present");
    assert_eq!(fetched.name, "Custom");
    assert_eq!(fetched.prompt_files, vec!["AGENT_SAFETY.md".to_string()]);
    assert!(!fetched.builtin);
}

#[test]
fn create_rejects_duplicate_id() {
    let dir = TempDir::new("agent-tpl");
    let store = AgentTemplateStore::new(dir.path().to_path_buf());
    assert!(store.create("dup", sample_template("First")).unwrap());

    assert!(!store.create("dup", sample_template("Second")).unwrap());
    assert_eq!(store.get("dup").unwrap().unwrap().name, "First");
}

#[test]
fn update_replaces_template_and_reports_presence() {
    let dir = TempDir::new("agent-tpl");
    let store = AgentTemplateStore::new(dir.path().to_path_buf());
    store.create("custom", sample_template("Old")).unwrap();
    assert!(store.update("custom", sample_template("New")).unwrap());
    assert_eq!(store.get("custom").unwrap().unwrap().name, "New");
    assert!(!store.update("missing", sample_template("X")).unwrap());
}

#[test]
fn delete_returns_presence_and_refuses_builtins() {
    let dir = TempDir::new("agent-tpl");
    let store = AgentTemplateStore::new(dir.path().to_path_buf());
    store
        .save_file(&rua_config::agent_template::builtin_templates())
        .unwrap();
    store.create("custom", sample_template("Custom")).unwrap();

    assert!(store.get("default").unwrap().is_some());
    assert!(store.get("blank").unwrap().is_some());
    assert!(store.get("qa").unwrap().is_some());

    assert!(store.delete("custom").unwrap());
    assert!(store.get("custom").unwrap().is_none());
    assert!(!store.delete("custom").unwrap());

    let err = store.delete("default").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn reads_preserve_customized_templates_and_unknown_fields() {
    let dir = TempDir::new("agent-tpl");
    let path = dir.path().join("agent_templates.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "version": 1,
            "templates": {
                "default": {
                    "name": "Existing Default",
                    "description": "user-owned default",
                    "prompt_files": ["CUSTOM.md"],
                    "builtin": true,
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-02T00:00:00Z",
                    "x-qwenpaw-import": {"source": "legacy-setup"}
                }
            },
            "x-template-store": {"schema": "future"}
        })
        .to_string(),
    )
    .unwrap();

    let store = AgentTemplateStore::new(dir.path().to_path_buf());
    let before = std::fs::read(dir.path().join("agent_templates.json")).unwrap();
    store.list().unwrap();
    let file = store.load().unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("agent_templates.json")).unwrap(),
        before
    );
    assert_eq!(
        file.extra["x-template-store"],
        serde_json::json!({"schema": "future"})
    );

    let default = file.templates.get("default").expect("default preserved");
    assert_eq!(default.name, "Existing Default");
    assert_eq!(
        default.extra["x-qwenpaw-import"],
        serde_json::json!({"source": "legacy-setup"})
    );

    assert!(
        !file.templates.contains_key("blank"),
        "reads must not backfill missing builtins"
    );
    assert!(
        !file.templates.contains_key("qa"),
        "reads must not backfill missing builtins"
    );
}

#[test]
fn atomic_write_leaves_no_temp_sibling() {
    let dir = TempDir::new("agent-tpl");
    let store = AgentTemplateStore::new(dir.path().to_path_buf());
    store.create("custom", sample_template("X")).unwrap();
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    for n in &names {
        assert!(
            !n.starts_with(".agent_templates.json.tmp"),
            "leftover temp: {n}"
        );
    }
    assert!(names.iter().any(|n| n == "agent_templates.json"));
}
