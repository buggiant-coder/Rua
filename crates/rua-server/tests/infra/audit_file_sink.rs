use std::fs;

use rua_security::{FileAuditSink, GuardDecision, SecurityAuditEvent};
use tracing_subscriber::layer::SubscriberExt;

#[test]
fn audit_event_written_to_jsonl() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tmp.path().join("audit");

    let sink = FileAuditSink::new(&audit_dir);

    let subscriber = tracing_subscriber::registry().with(sink);
    tracing::subscriber::with_default(subscriber, || {
        let event = SecurityAuditEvent::from_evaluation(
            "agent-1",
            "session-1",
            "bash",
            GuardDecision::Allow,
            &[],
        );
        event.emit();
    });

    let files: Vec<_> = fs::read_dir(&audit_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("audit-"))
        .collect();
    assert_eq!(files.len(), 1, "expected exactly one audit JSONL file");

    let content = fs::read_to_string(files[0].path()).unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1, "expected exactly one audit line");

    let entry: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(entry["agent_id"], "agent-1");
    assert_eq!(entry["session_id"], "session-1");
    assert_eq!(entry["tool_name"], "bash");
    assert!(entry["timestamp"].as_str().is_some());
}

#[test]
fn non_audit_events_are_not_written() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tmp.path().join("audit");

    let sink = FileAuditSink::new(&audit_dir);
    let subscriber = tracing_subscriber::registry().with(sink);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("regular log message");
        tracing::warn!(tool = "bash", "some other event");
    });

    assert!(
        !audit_dir.exists(),
        "non-audit events must not create an empty audit directory"
    );
}

#[test]
fn multiple_events_produce_multiple_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tmp.path().join("audit");

    let sink = FileAuditSink::new(&audit_dir);
    let subscriber = tracing_subscriber::registry().with(sink);

    tracing::subscriber::with_default(subscriber, || {
        for i in 0..3 {
            let event = SecurityAuditEvent::from_evaluation(
                &format!("agent-{i}"),
                "session-1",
                "bash",
                GuardDecision::Deny,
                &[],
            );
            event.emit();
        }
    });

    let files: Vec<_> = fs::read_dir(&audit_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("audit-"))
        .collect();

    let content = fs::read_to_string(files[0].path()).unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 3);

    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(v["timestamp"].as_str().is_some());
        assert!(v["tool_name"].as_str().is_some());
    }
}
