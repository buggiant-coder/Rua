use rua_security::engine::aggregate_decision;
use rua_security::{GuardDecision, Severity, ToolGuardFinding};

fn make_finding(severity: Severity) -> ToolGuardFinding {
    ToolGuardFinding {
        rule_id: "TEST-001".into(),
        category: "test".into(),
        severity,
        title: "Test finding".into(),
        description: "test finding".into(),
        tool_name: "bash".into(),
        guardian: "test_guardian".into(),
        message: "test finding".into(),
        remediation: String::new(),
        matched_pattern: None,
        matched_value: None,
    }
}

#[test]
fn critical_severity_requires_approval() {
    let findings = vec![make_finding(Severity::Critical)];
    assert_eq!(
        aggregate_decision(&findings),
        GuardDecision::RequiresApproval
    );
}

#[test]
fn non_critical_findings_require_approval() {
    let findings = vec![make_finding(Severity::High)];
    assert_eq!(
        aggregate_decision(&findings),
        GuardDecision::RequiresApproval
    );

    let findings = vec![make_finding(Severity::Medium)];
    assert_eq!(
        aggregate_decision(&findings),
        GuardDecision::RequiresApproval
    );

    let findings = vec![make_finding(Severity::Low)];
    assert_eq!(
        aggregate_decision(&findings),
        GuardDecision::RequiresApproval
    );
}

#[test]
fn info_only_findings_return_audit_only() {
    let findings = vec![make_finding(Severity::Info), make_finding(Severity::Info)];
    assert_eq!(aggregate_decision(&findings), GuardDecision::AuditOnly);
}

#[test]
fn info_mixed_with_actionable_findings_still_requires_approval() {
    let findings = vec![make_finding(Severity::Info), make_finding(Severity::Low)];
    assert_eq!(
        aggregate_decision(&findings),
        GuardDecision::RequiresApproval
    );
}
