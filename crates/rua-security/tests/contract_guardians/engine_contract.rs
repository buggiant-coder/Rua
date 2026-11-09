use rua_config::types::security::{SecurityConfig, ToolGuardRuleConfig};
use rua_security::{GuardDecision, Severity, ToolCallContext, ToolGuardEngine};
use serde_json::json;

fn engine(config: &SecurityConfig) -> ToolGuardEngine {
    ToolGuardEngine::new(config, None)
}

#[test]
fn engine_result_shape_is_stable_for_unknown_tool() {
    let config = SecurityConfig::default();
    let ctx = ToolCallContext::new(
        "agent",
        "session",
        "nonexistent_tool_xyz",
        json!({"data": "test"}),
    );

    let result = engine(&config).evaluate(&ctx);

    assert_eq!(result.decision, GuardDecision::Allow);
    assert!(result.findings.is_empty());
    assert!(result.guarded, "default config guards every tool name");

    let value = serde_json::to_value(&result).expect("guard result must serialize");
    assert_eq!(value["decision"], "allow");
    assert_eq!(value["guarded"], true);
    assert!(value["findings"].as_array().is_some_and(Vec::is_empty));
    assert!(value["evaluated_at"].is_string());
}

#[test]
fn engine_handles_empty_and_null_params_without_findings() {
    let config = SecurityConfig::default();
    let engine = engine(&config);

    for tool_input in [json!({}), json!({"command": null})] {
        let ctx = ToolCallContext::new("agent", "session", "bash", tool_input);

        let result = engine.evaluate(&ctx);

        assert_eq!(result.decision, GuardDecision::Allow);
        assert!(
            result.findings.is_empty(),
            "empty/null command should not produce findings: {:?}",
            result.findings
        );
    }
}

#[test]
fn disabled_engine_allows_sensitive_inputs() {
    let mut config = SecurityConfig::default();
    config.tool_guard.enabled = false;
    let ctx = ToolCallContext::new("agent", "session", "read", json!({"path": "/etc/passwd"}));

    let result = engine(&config).evaluate(&ctx);

    assert_eq!(result.decision, GuardDecision::Allow);
    assert!(!result.guarded);
    assert!(result.findings.is_empty());
}

#[test]
fn every_tool_name_is_guarded_for_universal_safety_scanning() {
    let config = SecurityConfig::default();
    let all_guarded = engine(&config);
    assert!(all_guarded.is_guarded("read"));
    assert!(all_guarded.is_guarded("unknown_tool"));
}

#[test]
fn universal_guarding_does_not_depend_on_tool_list_config() {
    let config = SecurityConfig::default();
    let engine = engine(&config);

    assert!(engine.is_guarded("bash"));
    assert!(engine.is_guarded("read"));
    assert!(engine.is_guarded("edit"));
    assert!(engine.is_guarded("bash"));
}

#[test]
fn permission_rules_not_tool_guard_lists_own_explicit_denies() {
    let config = SecurityConfig::default();
    let ctx = ToolCallContext::new("agent", "session", "read", json!({"path": "/etc/passwd"}));

    let result = engine(&config).evaluate(&ctx);

    assert_eq!(result.decision, GuardDecision::RequiresApproval);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].rule_id, "FILE-SYSTEM");
}

#[test]
fn file_guardian_runs_even_when_tool_is_not_guarded() {
    let config = SecurityConfig::default();

    let ctx = ToolCallContext::new("agent", "session", "read", json!({"path": "/etc/passwd"}));

    let result = engine(&config).evaluate(&ctx);

    assert_eq!(result.decision, GuardDecision::RequiresApproval);
    assert!(
        result.guarded,
        "all tool calls enter the safety scan pipeline"
    );
    assert!(
        result.findings.iter().any(|f| f.rule_id == "FILE-SYSTEM"),
        "file guardian is an always-run guardian and must still flag sensitive paths"
    );
}

#[test]
fn custom_deny_rule_maps_python_auto_deny_intent_to_rust_engine_decision() {
    let mut config = SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_DENY_ECHO",
        vec!["bash"],
        vec!["echo"],
        "LOW",
        "Deny",
    )];
    let result = engine(&config).evaluate(&ToolCallContext::new(
        "agent",
        "session",
        "bash",
        json!({"command": "echo secret"}),
    ));

    assert_eq!(result.decision, GuardDecision::Deny);
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule_id == "CUSTOM_DENY_ECHO")
        .expect("custom deny rule should produce a finding");
    assert_eq!(
        finding.severity,
        Severity::Critical,
        "Deny action, not configured LOW severity, controls the engine decision"
    );
    assert_eq!(
        finding.matched_pattern.as_deref(),
        Some("ast:CUSTOM_DENY_ECHO")
    );
    assert_eq!(finding.matched_value.as_deref(), Some("echo"));
}

#[test]
fn custom_audit_rule_stays_audit_only_and_preserves_finding_metadata() {
    let mut config = SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_AUDIT_ECHO",
        Vec::new(),
        vec!["echo"],
        "HIGH",
        "Audit",
    )];
    let result = engine(&config).evaluate(&ToolCallContext::new(
        "agent",
        "session",
        "bash",
        json!({"command": "echo diagnostics"}),
    ));

    assert_eq!(result.decision, GuardDecision::AuditOnly);
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule_id == "CUSTOM_AUDIT_ECHO")
        .expect("custom audit rule should produce a finding");
    assert_eq!(finding.severity, Severity::Info);
    assert_eq!(finding.guardian, "ast_rule_guardian");
    assert_eq!(finding.tool_name, "bash");
    assert_eq!(finding.matched_value.as_deref(), Some("echo"));
}

fn custom_command_rule(
    id: &str,
    tools: Vec<&str>,
    command_set: Vec<&str>,
    severity: &str,
    action: &str,
) -> ToolGuardRuleConfig {
    serde_json::from_value(json!({
        "id": id,
        "tools": tools,
        "category": "command_injection",
        "severity": severity,
        "action": action,
        "command_set": command_set
    }))
    .unwrap()
}
