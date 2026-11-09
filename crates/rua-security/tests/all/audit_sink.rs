use chrono::Utc;
use rua_security::read_audit_entries;

#[test]
fn read_entries_empty_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let entries = read_audit_entries(tmp.path(), None, 100);
    assert!(entries.is_empty());
}

#[test]
fn read_entries_with_limit_and_since() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("audit-2025-05-13.jsonl");

    let mut lines = Vec::new();
    for i in 0..5 {
        let ts = format!("2025-05-13T10:0{i}:00+00:00");
        let entry = serde_json::json!({
            "timestamp": ts,
            "kind": "ToolGuardEvaluation",
            "agent_id": "default",
            "tool_name": format!("tool_{i}"),
        });
        lines.push(serde_json::to_string(&entry).unwrap());
    }
    std::fs::write(&file, lines.join("\n") + "\n").unwrap();

    let entries = read_audit_entries(tmp.path(), None, 3);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["tool_name"], "tool_4");

    let since = chrono::DateTime::parse_from_rfc3339("2025-05-13T10:03:00+00:00")
        .unwrap()
        .with_timezone(&Utc);
    let entries = read_audit_entries(tmp.path(), Some(since), 100);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["tool_name"], "tool_4");
    assert_eq!(entries[1]["tool_name"], "tool_3");
}
