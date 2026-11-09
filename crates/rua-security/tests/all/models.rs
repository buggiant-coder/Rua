use rua_security::{GuardDecision, Severity, ToolGuardFinding, ToolGuardResult};
use serde_json::json;

fn finding(rule_id: &str, severity: Severity) -> ToolGuardFinding {
    ToolGuardFinding {
        rule_id: rule_id.to_string(),
        category: "dangerous_command".to_string(),
        severity,
        title: "Risky command".to_string(),
        description: "detected risky command".to_string(),
        tool_name: "bash".to_string(),
        guardian: "test_guardian".to_string(),
        message: "detected risky command".to_string(),
        remediation: String::new(),
        matched_pattern: None,
        matched_value: None,
    }
}

#[test]
fn severity_serializes_uppercase_and_parses_aliases() {
    let severities = [
        Severity::Info,
        Severity::Low,
        Severity::Medium,
        Severity::High,
        Severity::Critical,
    ];
    assert_eq!(
        severities.map(|severity| serde_json::to_value(severity).unwrap()),
        [
            json!("INFO"),
            json!("LOW"),
            json!("MEDIUM"),
            json!("HIGH"),
            json!("CRITICAL")
        ]
    );

    assert_eq!(
        serde_json::to_value(Severity::Critical).unwrap(),
        json!("CRITICAL")
    );
    assert_eq!(
        serde_json::to_value(Severity::Medium).unwrap(),
        json!("MEDIUM")
    );
    assert_eq!(Severity::parse("crit"), Some(Severity::Critical));
    assert_eq!(Severity::parse("MED"), Some(Severity::Medium));
    assert_eq!(Severity::parse("unknown"), None);
    assert_eq!(Severity::High.to_string(), "HIGH");
}

#[test]
fn guard_decision_uses_rust_wire_names_and_accepts_legacy_approval_alias() {
    let decisions = [
        GuardDecision::Allow,
        GuardDecision::Deny,
        GuardDecision::RequiresApproval,
        GuardDecision::AuditOnly,
    ];
    assert_eq!(
        decisions.map(|decision| serde_json::to_value(decision).unwrap()),
        [
            json!("allow"),
            json!("deny"),
            json!("requires_approval"),
            json!("audit_only")
        ]
    );

    assert_eq!(
        serde_json::to_value(GuardDecision::Allow).unwrap(),
        json!("allow")
    );
    assert_eq!(
        serde_json::to_value(GuardDecision::RequiresApproval).unwrap(),
        json!("requires_approval")
    );
    assert_eq!(
        serde_json::from_value::<GuardDecision>(json!("require_approval")).unwrap(),
        GuardDecision::RequiresApproval
    );
    assert_eq!(
        serde_json::from_value::<GuardDecision>(json!("ask_user")).unwrap(),
        GuardDecision::RequiresApproval
    );
}

#[test]
fn finding_serialization_preserves_required_fields_and_default_optional_fields() {
    let finding = finding("CMD-001", Severity::High);
    let value = serde_json::to_value(&finding).unwrap();

    assert_eq!(value["rule_id"], "CMD-001");
    assert_eq!(value["category"], "dangerous_command");
    assert_eq!(value["severity"], "HIGH");
    assert_eq!(value["title"], "Risky command");
    assert_eq!(value["description"], "detected risky command");
    assert_eq!(value["tool_name"], "bash");
    assert_eq!(value["guardian"], "test_guardian");
    assert_eq!(value["message"], "detected risky command");
    assert_eq!(value["remediation"], "");
    assert!(value["matched_pattern"].is_null());
    assert!(value["matched_value"].is_null());

    let roundtrip: ToolGuardFinding = serde_json::from_value(value).unwrap();
    assert_eq!(roundtrip.rule_id, "CMD-001");
    assert_eq!(roundtrip.title, "Risky command");
    assert_eq!(roundtrip.description, "detected risky command");
    assert_eq!(roundtrip.tool_name, "bash");
    assert_eq!(roundtrip.guardian, "test_guardian");
    assert_eq!(roundtrip.remediation, "");
    assert!(roundtrip.matched_pattern.is_none());
    assert!(roundtrip.matched_value.is_none());
}

#[test]
fn finding_deserializes_older_records_without_source_fields() {
    let finding: ToolGuardFinding = serde_json::from_value(json!({
        "rule_id": "LEGACY-001",
        "category": "dangerous_command",
        "severity": "HIGH",
        "message": "legacy finding",
        "remediation": "review manually"
    }))
    .unwrap();

    assert_eq!(finding.rule_id, "LEGACY-001");
    assert_eq!(finding.message, "legacy finding");
    assert_eq!(finding.title, "");
    assert_eq!(finding.description, "");
    assert_eq!(finding.tool_name, "");
    assert_eq!(finding.guardian, "");
}

#[test]
fn result_helpers_treat_high_and_critical_as_high_severity_only() {
    let info_result = ToolGuardResult::deny(vec![
        finding("INFO", Severity::Info),
        finding("MEDIUM", Severity::Medium),
    ]);
    assert!(!info_result.has_high_severity());

    let high_result = ToolGuardResult::deny(vec![finding("HIGH", Severity::High)]);
    assert!(high_result.has_high_severity());

    let critical_result = ToolGuardResult::deny(vec![finding("CRIT", Severity::Critical)]);
    assert!(critical_result.has_high_severity());
}

#[test]
fn allow_result_shape_is_stable() {
    let result = ToolGuardResult::allow();
    let value = serde_json::to_value(&result).unwrap();

    assert_eq!(value["decision"], "allow");
    assert_eq!(value["findings"], json!([]));
    assert_eq!(value["guarded"], false);
    assert!(value["evaluated_at"].is_string());
    assert!(!result.has_high_severity());
}
