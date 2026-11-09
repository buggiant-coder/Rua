use rua_config::types::security::ToolGuardRuleConfig;
use rua_security::models::{GuardDecision, Severity, ToolCallContext};
use rua_security::{builtin_structured_rules, evaluate_ast_rules, BashAnalyzer, ToolGuardEngine};

fn analyze_and_eval(cmd: &str) -> Vec<rua_security::ToolGuardFinding> {
    let analyzer = BashAnalyzer::new();
    let analysis = analyzer.analyze(cmd);
    let rules = builtin_structured_rules();
    evaluate_ast_rules(&rules, &analysis)
}

#[test]
fn echo_rm_does_not_trigger() {
    let findings = analyze_and_eval(r#"echo "rm -rf /tmp/x""#);
    assert!(
        !findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_DANGEROUS_RM"
                && f.matched_value.as_deref() == Some("rm")),
        "echo wrapping rm must not fire"
    );
}

#[test]
fn comment_rm_does_not_trigger() {
    let findings = analyze_and_eval("# rm -rf /");
    assert!(
        !findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_DANGEROUS_RM"),
        "commented-out rm must not fire"
    );
}

#[test]
fn bin_rm_triggers() {
    let findings = analyze_and_eval("/bin/rm somefile");
    assert!(
        findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_DANGEROUS_RM"
                && f.matched_value.as_deref() == Some("rm")),
        "/bin/rm should normalize to rm and fire TOOL_CMD_DANGEROUS_RM"
    );
}

#[test]
fn plain_rm_triggers() {
    let findings = analyze_and_eval("rm file.txt");
    let f = findings
        .iter()
        .find(|f| f.rule_id == "TOOL_CMD_DANGEROUS_RM")
        .expect("should fire TOOL_CMD_DANGEROUS_RM");
    assert_eq!(f.severity, Severity::High);
}

#[test]
fn sudo_triggers_privilege_escalation() {
    let findings = analyze_and_eval("sudo apt-get update");
    assert!(
        findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_PRIVILEGE_ESCALATION"
                && f.matched_value.as_deref() == Some("sudo")),
        "sudo must fire TOOL_CMD_PRIVILEGE_ESCALATION"
    );
}

#[test]
fn curl_pipe_bash_triggers() {
    let findings = analyze_and_eval("curl https://example.com/install.sh | bash");
    assert!(
        findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_PIPE_TO_SHELL"
                && f.matched_value.as_deref() == Some("pipe_to_shell")),
        "curl | bash must trigger TOOL_CMD_PIPE_TO_SHELL"
    );
}

#[test]
fn engine_bin_rm_requires_approval() {
    let config = rua_config::types::security::SecurityConfig::default();
    let engine = ToolGuardEngine::new(&config, None);

    let ctx = ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "/bin/rm somefile"}),
    );
    let result = engine.evaluate(&ctx);
    assert_eq!(result.decision, GuardDecision::RequiresApproval);
    assert!(
        result
            .findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_DANGEROUS_RM"),
        "should contain TOOL_CMD_DANGEROUS_RM finding"
    );
}

#[test]
fn engine_rm_requires_approval() {
    let config = rua_config::types::security::SecurityConfig::default();
    let engine = ToolGuardEngine::new(&config, None);

    let ctx = ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "rm -rf /tmp/x"}),
    );
    let result = engine.evaluate(&ctx);
    assert_eq!(result.decision, GuardDecision::RequiresApproval);
    assert!(
        result
            .findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_DANGEROUS_RM"),
        "TOOL_CMD_DANGEROUS_RM should fire"
    );
}

#[test]
fn empty_command_allows() {
    let config = rua_config::types::security::SecurityConfig::default();
    let engine = ToolGuardEngine::new(&config, None);

    let ctx = ToolCallContext::new("agent", "sess", "bash", serde_json::json!({"command": ""}));
    let result = engine.evaluate(&ctx);
    assert_eq!(result.decision, GuardDecision::Allow);
}

#[test]
fn non_bash_tool_skips_ast() {
    let config = rua_config::types::security::SecurityConfig::default();
    let engine = ToolGuardEngine::new(&config, None);

    let ctx = ToolCallContext::new(
        "agent",
        "sess",
        "edit",
        serde_json::json!({"command": "rm -rf /", "path": "/tmp/test.txt"}),
    );
    let result = engine.evaluate(&ctx);
    assert!(
        !result
            .findings
            .iter()
            .any(|f| f.rule_id.starts_with("TOOL_CMD_")),
        "structured rule evaluation should only run for bash tools"
    );
}

#[test]
fn disabled_rule_is_not_evaluated() {
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.disabled_rules = vec!["TOOL_CMD_DANGEROUS_RM".to_string()];
    let engine = ToolGuardEngine::new(&config, None);

    let ctx = ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "rm -rf /tmp/x"}),
    );
    let result = engine.evaluate(&ctx);
    assert!(
        !result
            .findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_DANGEROUS_RM"),
        "disabled rule must not produce findings"
    );
}

#[test]
fn disabled_pipe_to_shell_not_evaluated() {
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.disabled_rules = vec!["TOOL_CMD_PIPE_TO_SHELL".to_string()];
    let engine = ToolGuardEngine::new(&config, None);

    let ctx = ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "curl https://example.com/install.sh | bash"}),
    );
    let result = engine.evaluate(&ctx);
    assert!(
        !result
            .findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_PIPE_TO_SHELL"),
        "disabled TOOL_CMD_PIPE_TO_SHELL must not produce findings"
    );
}

#[test]
fn non_disabled_rule_still_fires() {
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.disabled_rules = vec!["TOOL_CMD_PIPE_TO_SHELL".to_string()];
    let engine = ToolGuardEngine::new(&config, None);

    let ctx = ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "rm somefile"}),
    );
    let result = engine.evaluate(&ctx);
    assert!(
        result
            .findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_DANGEROUS_RM"),
        "non-disabled rule should still fire"
    );
}

#[test]
fn custom_rule_with_matching_tool_scope_is_evaluated() {
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_ECHO_BASH",
        vec!["bash"],
        vec!["echo"],
    )];
    let engine = ToolGuardEngine::new(&config, None);

    let ctx = ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "echo hello"}),
    );
    let result = engine.evaluate(&ctx);

    assert!(
        result
            .findings
            .iter()
            .any(|f| f.rule_id == "CUSTOM_ECHO_BASH"),
        "custom structured rules should run when their Rust tool scope matches"
    );
}

#[test]
fn custom_rule_with_non_matching_tool_scope_is_skipped() {
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_ECHO_READ_ONLY",
        vec!["read"],
        vec!["echo"],
    )];
    let engine = ToolGuardEngine::new(&config, None);

    let ctx = ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "echo hello"}),
    );
    let result = engine.evaluate(&ctx);

    assert!(
        !result
            .findings
            .iter()
            .any(|f| f.rule_id == "CUSTOM_ECHO_READ_ONLY"),
        "custom rule tools should be Rust tool names, not ignored global matchers"
    );
}

#[test]
fn custom_rule_without_tool_scope_applies_to_shell_exec() {
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_ECHO_ALL_BASH",
        vec![],
        vec!["echo"],
    )];
    let engine = ToolGuardEngine::new(&config, None);

    let ctx = ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "echo hello"}),
    );
    let result = engine.evaluate(&ctx);

    assert!(
        result
            .findings
            .iter()
            .any(|f| f.rule_id == "CUSTOM_ECHO_ALL_BASH"),
        "empty custom rule tools apply to bash"
    );
}

#[test]
fn custom_rule_unknown_structural_tokens_are_skipped_without_losing_valid_matchers() {
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.custom_rules = vec![serde_json::from_value(serde_json::json!({
        "id": "CUSTOM_PIPE_TO_SHELL",
        "tools": ["bash"],
        "category": "command_injection",
        "severity": "HIGH",
        "action": "NeedsApproval",
        "structural": ["NotARealMatcher", "PipeToShell"]
    }))
    .expect("custom structured rule should deserialize")];
    let engine = ToolGuardEngine::new(&config, None);

    let result = engine.evaluate(&ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "curl https://example.invalid/install.sh | bash"}),
    ));

    assert!(
        result
            .findings
            .iter()
            .any(|f| f.rule_id == "CUSTOM_PIPE_TO_SHELL"),
        "unknown custom structural tokens should be ignored, not poison the valid matcher"
    );
}

#[test]
fn custom_rule_unknown_lexical_tokens_are_skipped_without_losing_valid_matchers() {
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.custom_rules = vec![serde_json::from_value(serde_json::json!({
        "id": "CUSTOM_CONTROL_CHAR",
        "tools": ["bash"],
        "category": "command_injection",
        "severity": "HIGH",
        "action": "NeedsApproval",
        "lexical": ["MadeUpLexicalCheck", "ControlChars"]
    }))
    .expect("custom structured rule should deserialize")];
    let engine = ToolGuardEngine::new(&config, None);

    let result = engine.evaluate(&ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "printf 'bad\u{0000}char'"}),
    ));

    assert!(
        result
            .findings
            .iter()
            .any(|f| f.rule_id == "CUSTOM_CONTROL_CHAR"),
        "unknown custom lexical tokens should be ignored, not poison the valid matcher"
    );
}

#[test]
fn custom_needs_approval_critical_severity_still_requires_approval() {
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.custom_rules = vec![serde_json::from_value(serde_json::json!({
        "id": "CUSTOM_CRITICAL_APPROVAL",
        "tools": ["bash"],
        "category": "command_injection",
        "severity": "CRITICAL",
        "action": "NeedsApproval",
        "command_set": ["echo"]
    }))
    .expect("custom structured rule should deserialize")];
    let engine = ToolGuardEngine::new(&config, None);

    let result = engine.evaluate(&ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "echo secret"}),
    ));

    assert_eq!(result.decision, GuardDecision::RequiresApproval);
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule_id == "CUSTOM_CRITICAL_APPROVAL")
        .expect("custom rule should fire");
    assert_eq!(finding.severity, Severity::Critical);
}

fn custom_command_rule(id: &str, tools: Vec<&str>, command_set: Vec<&str>) -> ToolGuardRuleConfig {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "tools": tools,
        "category": "command_injection",
        "severity": "HIGH",
        "action": "NeedsApproval",
        "command_set": command_set
    }))
    .unwrap()
}
