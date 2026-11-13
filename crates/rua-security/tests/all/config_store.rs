use rua_config::types::security::{
    FileGuardConfig, SkillScannerConfig, SkillScannerWhitelistEntry,
};
use rua_security::{SecurityConfigStore, StoreError};
use serde_json::json;
use std::path::PathBuf;

fn fresh_data_dir(label: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("rua-security-store-{label}-{pid}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("create tempdir");
    dir
}

#[test]
fn missing_config_returns_defaults() {
    let dir = fresh_data_dir("missing-config");
    let store = SecurityConfigStore::new(dir.clone());

    let file = store.load("default").unwrap().file_guard;
    let skill = store.get_skill_scanner().expect("get_skill_scanner");

    assert!(file.sensitive_files.is_empty());
    assert!(skill.whitelist.is_empty());
    assert_eq!(skill.timeout, 30);
}

#[test]
fn fresh_root_with_unset_security_uses_process_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecurityConfigStore::new(dir.path().to_path_buf());
    rua_config::save_config(&rua_config::RootConfig::default(), &store.config_path()).unwrap();
    assert_eq!(
        store.get_allow_no_auth_hosts().unwrap(),
        vec!["127.0.0.1", "::1"]
    );
    assert!(store.get_skill_scanner().unwrap().whitelist.is_empty());
    rua_config::atomic_write_json(&store.config_path(), &json!({"security": null})).unwrap();
    assert_eq!(
        store.get_allow_no_auth_hosts().unwrap(),
        vec!["127.0.0.1", "::1"]
    );
    store
        .update_allow_no_auth_hosts(vec!["::1".into()])
        .unwrap();
    assert_eq!(store.get_allow_no_auth_hosts().unwrap(), vec!["::1"]);
}

#[test]
fn update_file_guard_persists() {
    let dir = fresh_data_dir("update-file-guard");
    let store = SecurityConfigStore::new(dir);

    let cfg = FileGuardConfig {
        enabled: true,
        sensitive_files: vec!["/etc/passwd".into(), "/root/.ssh".into()],
        ..Default::default()
    };

    let mut security = store.load("default").unwrap();
    security.file_guard = cfg;
    store.save("default", &security).unwrap();
    let saved = store.load("default").unwrap().file_guard;
    assert!(saved.enabled);
    assert_eq!(saved.sensitive_files.len(), 2);
}

#[test]
fn process_settings_discard_retired_guard_policy_and_preserve_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecurityConfigStore::new(dir.path().to_path_buf());
    rua_config::atomic_write_json(
        &store.config_path(),
        &json!({
            "future_root": 9,
            "security": {"tool_guard": {"enabled": false}, "file_guard": {"enabled": false},
                "skill_scanner": {"mode": "warn"}, "future_security": 7}
        }),
    )
    .unwrap();
    store
        .update_allow_no_auth_hosts(vec!["::1".into()])
        .unwrap();
    let root: serde_json::Value =
        serde_json::from_slice(&std::fs::read(store.config_path()).unwrap()).unwrap();
    assert!(root["security"].get("tool_guard").is_none());
    assert!(root["security"].get("file_guard").is_none());
    assert_eq!(root["security"]["future_security"], 7);
    assert_eq!(root["future_root"], 9);
    assert!(store.load("default").unwrap().tool_guard.enabled);
    assert!(
        !dir.path().join("workspaces").exists(),
        "no implicit Agent migration"
    );
}

#[test]
fn skill_scanner_update_persists() {
    let dir = fresh_data_dir("update-skill-scanner");
    let store = SecurityConfigStore::new(dir);

    let cfg = SkillScannerConfig {
        mode: "block".into(),
        timeout: 30,
        ..Default::default()
    };

    let saved = store
        .update_skill_scanner(cfg)
        .expect("update_skill_scanner");
    assert_eq!(saved.mode, "block");
    assert_eq!(saved.timeout, 30);
}

#[test]
fn whitelist_add_and_remove() {
    let dir = fresh_data_dir("whitelist");
    let store = SecurityConfigStore::new(dir);

    let entry = SkillScannerWhitelistEntry {
        skill_name: "trusted-tool".into(),
        content_hash: "deadbeef".into(),
        added_at: String::new(),
        extra: serde_json::Map::new(),
    };

    let added = store.add_whitelist(entry).expect("add_whitelist");
    assert_eq!(added.skill_name, "trusted-tool");

    let cfg = store.get_skill_scanner().expect("get_skill_scanner");
    assert_eq!(cfg.whitelist.len(), 1);

    store
        .remove_whitelist("trusted-tool")
        .expect("remove existing entry");

    let cfg = store.get_skill_scanner().expect("get_skill_scanner");
    assert!(cfg.whitelist.is_empty());

    let err = store
        .remove_whitelist("trusted-tool")
        .expect_err("removing twice should be NotFound");
    assert!(matches!(err, StoreError::NotFound { .. }));
}

#[test]
fn blocked_history_lifecycle() {
    let dir = fresh_data_dir("blocked-history");
    let store = SecurityConfigStore::new(dir.clone());

    assert!(store.get_blocked_history().expect("empty list").is_empty());

    let sidecar = dir.join("skill_scanner_blocked.json");
    let entries = json!([
        {"skill_name": "evil-1", "blocked_at": "2026-05-10T07:00:00Z"},
        {"skill_name": "evil-2", "blocked_at": "2026-05-10T07:01:00Z"},
    ]);
    std::fs::write(&sidecar, serde_json::to_vec(&entries).unwrap()).unwrap();

    let history = store.get_blocked_history().expect("read history");
    assert_eq!(history.len(), 2);

    store
        .delete_blocked_history_entry(0)
        .expect("delete index 0");
    let after = store.get_blocked_history().expect("read history");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0]["skill_name"], "evil-2");

    let err = store
        .delete_blocked_history_entry(99)
        .expect_err("out-of-range");
    assert!(matches!(err, StoreError::NotFound { .. }));

    store.clear_blocked_history().expect("clear");
    assert!(!sidecar.exists(), "clearing should remove the sidecar");
    assert!(store
        .get_blocked_history()
        .expect("read history")
        .is_empty());
}
