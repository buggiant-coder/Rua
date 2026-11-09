use rua_config::types::security::{SecurityConfig, ToolGuardConfig};
use rua_security::SecurityConfigStore;
use std::path::PathBuf;

fn fresh_data_dir(label: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("rua-security-per-agent-{label}-{pid}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("create tempdir");
    dir
}

fn make_config(tool_guard: ToolGuardConfig) -> SecurityConfig {
    SecurityConfig {
        tool_guard,
        ..Default::default()
    }
}

#[test]
fn load_nonexistent_agent_returns_builtin() {
    let dir = fresh_data_dir("load-nonexist");
    let store = SecurityConfigStore::new(dir);

    let cfg = store.load("ghost-agent").expect("load");
    let builtin = SecurityConfigStore::builtin_template();

    assert_eq!(cfg.tool_guard.enabled, builtin.tool_guard.enabled);
    assert!(cfg.tool_guard.custom_rules.is_empty());
    assert!(cfg.file_guard.sensitive_files.is_empty());
    assert!(cfg.skill_scanner.whitelist.is_empty());
}

#[test]
fn save_then_load_round_trips() {
    let dir = fresh_data_dir("save-load");
    let store = SecurityConfigStore::new(dir);

    let cfg = make_config(ToolGuardConfig {
        enabled: false,
        disabled_rules: vec!["RULE_A".into(), "RULE_B".into()],
        ..Default::default()
    });

    store.save("agent-1", &cfg).expect("save");
    let loaded = store.load("agent-1").expect("load");

    assert!(!loaded.tool_guard.enabled);
    assert_eq!(loaded.tool_guard.disabled_rules, vec!["RULE_A", "RULE_B"]);
}

#[test]
fn delete_then_load_returns_builtin() {
    let dir = fresh_data_dir("delete-load");
    let store = SecurityConfigStore::new(dir);

    let cfg = make_config(ToolGuardConfig {
        enabled: false,
        ..Default::default()
    });
    store.save("agent-x", &cfg).expect("save");

    store.delete("agent-x").expect("delete");
    let loaded = store.load("agent-x").expect("load after delete");
    assert!(loaded.tool_guard.enabled);
}

#[test]
fn delete_nonexistent_is_noop() {
    let dir = fresh_data_dir("delete-noop");
    let store = SecurityConfigStore::new(dir);
    store
        .delete("never-existed")
        .expect("delete nonexistent should succeed");
}

#[test]
fn cross_agent_isolation() {
    let dir = fresh_data_dir("isolation");
    let store = SecurityConfigStore::new(dir);

    let cfg_a = make_config(ToolGuardConfig {
        enabled: false,
        ..Default::default()
    });
    let cfg_b = make_config(ToolGuardConfig {
        disabled_rules: vec!["ONLY_B".into()],
        ..Default::default()
    });

    store.save("agent-a", &cfg_a).expect("save A");
    store.save("agent-b", &cfg_b).expect("save B");

    let loaded_a = store.load("agent-a").expect("load A");
    let loaded_b = store.load("agent-b").expect("load B");

    assert!(!loaded_a.tool_guard.enabled);
    assert!(loaded_a.tool_guard.disabled_rules.is_empty());

    assert!(loaded_b.tool_guard.enabled);
    assert_eq!(loaded_b.tool_guard.disabled_rules, vec!["ONLY_B"]);
}

#[test]
fn atomic_write_no_partial_file() {
    let dir = fresh_data_dir("atomic");
    let store = SecurityConfigStore::new(dir.clone());

    let cfg = SecurityConfig::default();
    store.save("agent-atomic", &cfg).expect("save");

    let path = dir.join("workspaces/agent-atomic/security.json");
    assert!(path.exists());

    let content = std::fs::read_to_string(&path).expect("read");
    let parsed: SecurityConfig = serde_json::from_str(&content).expect("valid json");
    assert!(parsed.tool_guard.enabled);

    let parent = path.parent().unwrap();
    let tmp_files: Vec<_> = std::fs::read_dir(parent)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".security.json.tmp")
        })
        .collect();
    assert!(tmp_files.is_empty(), "no leftover tmp files");
}
