use rua_security::models::ToolCallContext;
use rua_security::{builtin_structured_rules, evaluate_ast_rules, BashAnalyzer, ToolGuardEngine};

fn ast_findings(cmd: &str) -> Vec<rua_security::ToolGuardFinding> {
    let analyzer = BashAnalyzer::new();
    let analysis = analyzer.analyze(cmd);
    let rules = builtin_structured_rules();
    evaluate_ast_rules(&rules, &analysis)
}

#[test]
fn ls_absolute_path_outside_cwd_no_finding() {
    let findings = ast_findings("ls -la /Users/example/.rua/workspaces/for-test/sessions/x.json");
    assert!(
        findings.is_empty(),
        "ls with absolute path must produce zero AST findings, got: {:?}",
        findings
    );
}

#[test]
fn cat_etc_passwd_no_finding() {
    let findings = ast_findings("cat /etc/passwd");
    assert!(
        findings.is_empty(),
        "cat /etc/passwd must produce zero AST findings (path control is FileGuardian's job), got: {:?}",
        findings
    );
}

#[test]
fn stat_arbitrary_path_no_finding() {
    let findings = ast_findings("stat /var/log/syslog");
    assert!(
        findings.is_empty(),
        "stat with arbitrary path must produce zero AST findings, got: {:?}",
        findings
    );
}

#[test]
fn rm_still_triggers_structured_rule() {
    let findings = ast_findings("rm -f /any/path/file.txt");
    assert!(
        findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_DANGEROUS_RM"),
        "rm must trigger TOOL_CMD_DANGEROUS_RM"
    );
}

#[test]
fn dd_still_triggers_structured_rule() {
    let findings = ast_findings("dd if=/dev/zero of=/dev/sda");
    assert!(
        findings
            .iter()
            .any(|f| f.rule_id == "TOOL_CMD_FS_DESTRUCTION"),
        "dd must trigger TOOL_CMD_FS_DESTRUCTION"
    );
}

#[test]
fn no_path_boundary_or_sensitive_path_rule_ids_exist() {
    let rules = builtin_structured_rules();
    let ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
    assert!(
        !ids.contains(&"AST_PATH_BOUNDARY"),
        "AST_PATH_BOUNDARY must not exist in builtin AST rules"
    );
    assert!(
        !ids.contains(&"AST_SENSITIVE_PATH"),
        "AST_SENSITIVE_PATH must not exist in builtin AST rules"
    );
}

#[test]
fn full_engine_ls_outside_workspace_no_approval() {
    let config = rua_config::types::security::SecurityConfig::default();
    let engine = ToolGuardEngine::new(&config, None);

    let mut ctx = ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "ls -la /tmp/outside/workspace"}),
    );
    ctx.working_dir = "/home/user/project".to_string();
    let result = engine.evaluate(&ctx);
    assert_eq!(
        result.decision,
        rua_security::GuardDecision::Allow,
        "ls outside workspace must not trigger approval via full engine"
    );
}

#[test]
fn full_engine_rm_still_requires_approval() {
    let config = rua_config::types::security::SecurityConfig::default();
    let engine = ToolGuardEngine::new(&config, None);

    let ctx = ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "rm -rf /tmp/x"}),
    );
    let result = engine.evaluate(&ctx);
    assert_eq!(
        result.decision,
        rua_security::GuardDecision::RequiresApproval,
        "rm must still require approval"
    );
}

#[test]
fn full_engine_rm_outside_workspace_still_requires_approval() {
    let config = rua_config::types::security::SecurityConfig::default();
    let engine = ToolGuardEngine::new(&config, None);

    let mut ctx = ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": "rm -rf /tmp/outside-workspace"}),
    );
    ctx.working_dir = "/home/user/project".to_string();
    let result = engine.evaluate(&ctx);
    assert_eq!(
        result.decision,
        rua_security::GuardDecision::RequiresApproval,
        "rm outside workspace must still trigger approval instead of hard deny"
    );
    assert!(
        result
            .findings
            .iter()
            .any(|finding| finding.matched_pattern.as_deref() == Some("ast:rm_workspace_boundary")),
        "rm outside workspace must keep the boundary finding visible"
    );
}
