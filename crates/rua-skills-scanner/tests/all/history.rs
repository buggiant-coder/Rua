use std::path::Path;

use rua_skills_scanner::history::{
    clear_blocked_history_at, get_blocked_history_at, record_blocked_skill_at,
    remove_blocked_entry_at, BlockedSkillRecord,
};
use rua_skills_scanner::{Finding, ScanResult, Severity, ThreatCategory};

fn finding(id: &str, severity: Severity) -> Finding {
    Finding {
        id: id.to_string(),
        rule_id: format!("RULE-{id}"),
        category: ThreatCategory::CommandInjection,
        severity,
        title: format!("Finding {id}"),
        description: "detected risk".to_string(),
        file_path: Some("main.py".to_string()),
        line_number: Some(7),
        snippet: Some("os.system('rm -rf /')".to_string()),
        remediation: None,
        analyzer: Some("pattern".to_string()),
        metadata: serde_json::Map::new(),
    }
}

fn result(skill_name: &str) -> ScanResult {
    ScanResult::compute(
        skill_name.to_string(),
        "/tmp/skill".to_string(),
        vec![finding("danger", Severity::High)],
        0.0,
        vec!["pattern".to_string()],
        Vec::new(),
    )
}

fn write_skill(dir: &Path, content: &str) {
    std::fs::write(dir.join("SKILL.md"), content).unwrap();
}

#[test]
fn blocked_record_deserializes_optional_fields_with_python_defaults() {
    let record: BlockedSkillRecord = serde_json::from_value(serde_json::json!({
        "skill_name": "demo",
        "blocked_at": "2024-01-01T00:00:00Z",
        "max_severity": "HIGH"
    }))
    .unwrap();

    assert_eq!(record.skill_name, "demo");
    assert_eq!(record.max_severity, "HIGH");
    assert!(record.findings.is_empty());
    assert_eq!(record.content_hash, "");
    assert_eq!(record.action, "blocked");
}

#[test]
fn history_read_returns_empty_for_missing_or_invalid_file() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.json");
    assert!(get_blocked_history_at(&missing).is_empty());

    let invalid = dir.path().join("bad.json");
    std::fs::write(&invalid, "not json").unwrap();
    assert!(get_blocked_history_at(&invalid).is_empty());
}

#[test]
fn record_remove_and_clear_blocked_history_by_skill_and_hash() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = tempfile::tempdir().unwrap();
    write_skill(skill_dir.path(), "hello");

    let path = dir.path().join("skill_scan_blocked.json");
    record_blocked_skill_at(&path, &result("demo"), skill_dir.path(), "warned");

    let records = get_blocked_history_at(&path);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].skill_name, "demo");
    assert_eq!(records[0].max_severity, "HIGH");
    assert_eq!(records[0].action, "warned");
    assert_eq!(records[0].findings.len(), 1);
    assert_eq!(records[0].findings[0]["rule_id"], "RULE-danger");
    assert_eq!(records[0].content_hash.len(), 64);

    assert!(!remove_blocked_entry_at(&path, "demo", "wrong-hash"));
    assert!(remove_blocked_entry_at(
        &path,
        "demo",
        &records[0].content_hash
    ));
    assert!(get_blocked_history_at(&path).is_empty());

    record_blocked_skill_at(&path, &result("demo"), skill_dir.path(), "blocked");
    assert_eq!(get_blocked_history_at(&path).len(), 1);
    clear_blocked_history_at(&path);
    assert!(get_blocked_history_at(&path).is_empty());
}
