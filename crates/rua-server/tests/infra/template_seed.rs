use rua_core::seed_manifest::is_entry_materialized;
use rua_server::seed::templates::ensure_builtin_templates_seeded;

#[test]
fn templates_seed_once_and_never_reconcile_user_changes_or_deletion() {
    let tmp = tempfile::tempdir().unwrap();
    ensure_builtin_templates_seeded(tmp.path());
    let path = tmp.path().join("agent_templates.json");
    let file: rua_config::AgentTemplatesFile =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(file.templates.len(), 3);
    assert!(file.templates["blank"].prompt_files.is_empty());
    assert_eq!(file.templates["default"].prompt_files.len(), 7);
    assert_eq!(file.templates["qa"].prompt_files.len(), 8);
    assert!(is_entry_materialized(tmp.path(), "agent_templates:builtin").unwrap());
    let custom = b"{\"templates\":{},\"custom\":true}";
    std::fs::write(&path, custom).unwrap();
    ensure_builtin_templates_seeded(tmp.path());
    assert_eq!(std::fs::read(&path).unwrap(), custom);
    std::fs::remove_file(&path).unwrap();
    ensure_builtin_templates_seeded(tmp.path());
    assert!(!path.exists());
}

#[test]
fn preexisting_template_file_is_not_claimed_or_rewritten() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("agent_templates.json");
    let custom = b"{\"templates\":{\"default\":{\"prompt_files\":[\"AGENT_SAFETY.md\",\"AGENT_zOther.md\"]}}}";
    std::fs::write(&path, custom).unwrap();
    ensure_builtin_templates_seeded(tmp.path());
    assert_eq!(std::fs::read(&path).unwrap(), custom);
    assert!(!is_entry_materialized(tmp.path(), "agent_templates:builtin").unwrap());
}
