use rua_config::dirs::Paths;
use rua_config::types::security::FileGuardConfig;
use rua_security::file_guardian::{extract_bash_redirect_paths, extract_paths, looks_like_path};
use rua_security::{FileGuardian, Severity, ToolCallContext, ToolGuardFinding};
use serde_json::json;
use std::path::PathBuf;

fn guardian() -> FileGuardian {
    FileGuardian::new(&FileGuardConfig::default())
}

fn guardian_with_config(config: FileGuardConfig) -> FileGuardian {
    FileGuardian::new(&config)
}

fn assert_populated_finding(finding: &ToolGuardFinding) {
    assert!(!finding.rule_id.is_empty(), "rule_id must be populated");
    assert!(!finding.category.is_empty(), "category must be populated");
    assert!(!finding.title.is_empty(), "title must be populated");
    assert!(
        !finding.description.is_empty(),
        "description must be populated"
    );
    assert!(!finding.tool_name.is_empty(), "tool_name must be populated");
    assert!(!finding.guardian.is_empty(), "guardian must be populated");
    assert!(!finding.message.is_empty(), "message must be populated");
    assert!(
        !finding.remediation.is_empty(),
        "remediation must be populated"
    );
}

#[test]
fn evaluate_returns_vec_for_known_tool() {
    let ctx = ToolCallContext::new("agent", "session", "read", json!({"path": "/tmp/test.txt"}));

    let findings = guardian().evaluate(&ctx);

    assert!(findings.is_empty());
}

#[test]
fn unknown_and_empty_tool_names_do_not_crash() {
    for tool_name in ["nonexistent_tool_xyz", ""] {
        let ctx = ToolCallContext::new(
            "agent",
            "session",
            tool_name,
            json!({"data": "test", "path": "/tmp/test.txt"}),
        );

        let findings = guardian().evaluate(&ctx);

        assert!(
            findings.is_empty(),
            "unexpected finding for tool name {tool_name:?}: {findings:?}"
        );
    }
}

#[test]
fn empty_and_null_params_do_not_crash() {
    for tool_input in [
        json!({}),
        json!({"path": null}),
        json!({"path": "", "nested": [{"path": null}]}),
    ] {
        let ctx = ToolCallContext::new("agent", "session", "read", tool_input);

        let findings = guardian().evaluate(&ctx);

        assert!(
            findings.is_empty(),
            "empty/null path inputs should not produce findings: {findings:?}"
        );
    }
}

#[test]
fn sensitive_file_finding_populates_public_fields() {
    let ctx = ToolCallContext::new("agent", "session", "read", json!({"path": "/etc/passwd"}));

    let findings = guardian().evaluate(&ctx);
    let finding = findings
        .iter()
        .find(|f| f.rule_id == "FILE-SYSTEM")
        .expect("system-sensitive file access must produce FILE-SYSTEM finding");

    assert_populated_finding(finding);
    assert_eq!(finding.category, "sensitive_file");
    assert_eq!(finding.severity, Severity::High);
    assert_eq!(finding.matched_pattern.as_deref(), Some("/etc/passwd"));
    assert_eq!(
        finding.matched_value.as_deref(),
        Some(".../<redacted>/passwd")
    );
}

#[test]
fn finding_serializes_with_stable_rust_contract_fields() {
    let ctx = ToolCallContext::new("agent", "session", "read", json!({"path": "/etc/passwd"}));
    let findings = guardian().evaluate(&ctx);
    let finding = findings
        .iter()
        .find(|f| f.rule_id == "FILE-SYSTEM")
        .expect("system-sensitive file access must produce FILE-SYSTEM finding");

    let value = serde_json::to_value(finding).expect("finding must serialize");

    assert_eq!(value["rule_id"], "FILE-SYSTEM");
    assert_eq!(value["category"], "sensitive_file");
    assert_eq!(value["severity"], "HIGH");
    assert!(value["message"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(
        value["title"].as_str().is_some_and(|s| !s.is_empty()),
        "QwenPaw contract exposes a populated finding title"
    );
    assert!(
        value["description"].as_str().is_some_and(|s| !s.is_empty()),
        "QwenPaw contract exposes a populated finding description"
    );
    assert_eq!(value["tool_name"], "read");
    assert_eq!(value["guardian"], "file_guardian");
    assert!(value["remediation"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(value["matched_pattern"], "/etc/passwd");
    assert_eq!(value["matched_value"], ".../<redacted>/passwd");
}

#[test]
fn rust_canonical_bash_redirect_targets_are_guarded() {
    let paths = Paths {
        rua_root: PathBuf::from("/tmp/rua-work"),
        secret_dir: PathBuf::from("/tmp/rua-work.secret"),
        cache_dir: PathBuf::from("/tmp/rua-work.cache"),
    };
    let guardian = FileGuardian::with_paths(&FileGuardConfig::default(), &paths);
    let ctx = ToolCallContext::new(
        "agent",
        "session",
        "bash",
        json!({"command": "printf token > /tmp/rua-work.secret/token.json"}),
    );

    let findings = guardian.evaluate(&ctx);

    assert!(
        findings.iter().any(|f| f.rule_id == "FILE-SECRET-DIR"),
        "Rust canonical Bash redirect target should be guarded, got: {findings:?}"
    );
}

#[test]
fn secret_dir_direct_args_and_redirects_are_denied() {
    let paths = Paths {
        rua_root: PathBuf::from("/tmp/rua-work"),
        secret_dir: PathBuf::from("/tmp/rua-work.secret"),
        cache_dir: PathBuf::from("/tmp/rua-work.cache"),
    };
    let guardian = FileGuardian::with_paths(&FileGuardConfig::default(), &paths);

    let direct = ToolCallContext::new(
        "agent",
        "session",
        "edit",
        json!({"path": "/tmp/rua-work.secret/token.json"}),
    );
    let redirect = ToolCallContext::new(
        "agent",
        "session",
        "bash",
        json!({"command": "echo token > /tmp/rua-work.secret/token.json"}),
    );

    for ctx in [direct, redirect] {
        let findings = guardian.evaluate(&ctx);
        assert!(
            findings.iter().any(|f| f.rule_id == "FILE-SECRET-DIR"),
            "secret_dir access must be denied for {}: {findings:?}",
            ctx.tool_name
        );
    }
}

#[test]
fn configured_sensitive_file_and_directory_patterns_are_guarded() {
    let config = FileGuardConfig {
        sensitive_files: vec![
            "/tmp/rua-work/private-token.txt".to_string(),
            "/tmp/rua-work/secrets/**".to_string(),
        ],
        ..Default::default()
    };
    let guardian = guardian_with_config(config);

    for (path, label) in [
        ("/tmp/rua-work/private-token.txt", "exact file pattern"),
        (
            "/tmp/rua-work/secrets/nested/deep.key",
            "directory glob pattern",
        ),
    ] {
        let ctx = ToolCallContext::new("agent", "session", "read", json!({"path": path}));
        let findings = guardian.evaluate(&ctx);

        assert!(
            findings.iter().any(|f| f.rule_id == "FILE-SENSITIVE"),
            "{label} should be guarded for {path:?}: {findings:?}"
        );
    }
}

#[test]
fn engine_respects_disabled_file_guard_for_sensitive_paths() {
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.file_guard.enabled = false;
    let engine = rua_security::ToolGuardEngine::new(&config, None);
    let ctx = ToolCallContext::new("agent", "session", "read", json!({"path": "/etc/passwd"}));

    let result = engine.evaluate(&ctx);

    assert_eq!(result.decision, rua_security::GuardDecision::Allow);
    assert!(result.guarded);
    assert!(
        result.findings.is_empty(),
        "disabled File Guard must not emit always-run file findings: {:?}",
        result.findings
    );
}

#[test]
fn tmp_redirect_does_not_trigger_secret_dir_guard() {
    let paths = Paths {
        rua_root: PathBuf::from("/tmp/rua-work"),
        secret_dir: PathBuf::from("/tmp/rua-work.secret"),
        cache_dir: PathBuf::from("/tmp/rua-work.cache"),
    };
    let guardian = FileGuardian::with_paths(&FileGuardConfig::default(), &paths);
    let ctx = ToolCallContext::new(
        "agent",
        "session",
        "bash",
        json!({"command": "echo token >> /tmp/safe.log"}),
    );

    let findings = guardian.evaluate(&ctx);

    assert!(
        !findings.iter().any(|f| f.rule_id == "FILE-SECRET-DIR"),
        "tmp redirect must not be treated as secret_dir access: {findings:?}"
    );
}

#[test]
fn workspace_external_temp_write_is_not_file_guard_sensitive_by_default() {
    let paths = Paths {
        rua_root: PathBuf::from("/tmp/rua-work"),
        secret_dir: PathBuf::from("/tmp/rua-work.secret"),
        cache_dir: PathBuf::from("/tmp/rua-work.cache"),
    };
    let guardian = FileGuardian::with_paths(&FileGuardConfig::default(), &paths);
    let ctx = ToolCallContext::new(
        "agent",
        "session",
        "edit",
        json!({"file_path": "/tmp/read_docx.py", "content": "print('ok')"}),
    );

    let findings = guardian.evaluate(&ctx);

    assert!(
        findings.is_empty(),
        "file guard is a sensitive-path blacklist; workspace-external temp writes are not sensitive by default: {findings:?}"
    );
}

#[test]
fn redirect_operator_extraction_is_stable() {
    assert_eq!(
        extract_bash_redirect_paths("echo hello > /tmp/out.txt"),
        vec!["/tmp/out.txt"]
    );
    assert_eq!(
        extract_bash_redirect_paths("cmd >> /var/log/app.log"),
        vec!["/var/log/app.log"]
    );
    assert_eq!(
        extract_bash_redirect_paths("cmd &> /tmp/all.log"),
        vec!["/tmp/all.log"]
    );
    assert_eq!(
        extract_bash_redirect_paths("cmd 2> /tmp/err.log"),
        vec!["/tmp/err.log"]
    );
    assert_eq!(
        extract_bash_redirect_paths("cmd 2>>/tmp/err.log"),
        vec!["/tmp/err.log"]
    );
    assert_eq!(
        extract_bash_redirect_paths("cmd &>>/tmp/all.log"),
        vec!["/tmp/all.log"]
    );
    assert_eq!(
        extract_bash_redirect_paths("sort </tmp/input.txt"),
        vec!["/tmp/input.txt"]
    );
    assert!(
        extract_bash_redirect_paths("cat <<EOF").is_empty(),
        "heredoc delimiters are not file paths"
    );
}

#[test]
fn path_heuristic_rejects_uri_and_mime_tokens_without_losing_real_paths() {
    for token in [
        "",
        "-la",
        "--output=file",
        "http://example.com",
        "https://example.com/path",
        "ftp://files.example.com/archive",
        "data:text/plain;base64,AA==",
        "text/plain",
        "application/json",
        "image/png",
        "audio/mp3",
        "video/mp4",
        "echo",
        "cat",
    ] {
        assert!(
            !looks_like_path(token),
            "{token:?} should not be treated as a local path"
        );
    }

    for token in [
        "/etc/passwd",
        "~/docs",
        "./relative",
        "../parent",
        "src/main.rs",
        "/",
    ] {
        assert!(
            looks_like_path(token),
            "{token:?} should be treated as a local path"
        );
    }
}

#[test]
fn json_path_extraction_keeps_path_key_compat_but_ignores_non_path_media_values() {
    let paths = extract_paths(
        &json!({
            "url": "https://example.com/file.png",
            "media_type": "image/png",
            "nested": [{"mime_type": "application/json"}],
            "path": "safe.txt",
            "target_path": "../secrets/token.json",
            "source_path": "./src/main.rs"
        }),
        10,
    );

    assert_eq!(
        paths,
        vec![
            "safe.txt".to_string(),
            "./src/main.rs".to_string(),
            "../secrets/token.json".to_string()
        ]
    );
}

#[test]
fn traversal_findings_cover_plain_encoded_and_null_byte_paths() {
    let cases = [
        ("../secrets/token.json", "FILE-TRAVERSAL", Severity::High),
        (
            "/tmp/%2e%2e/secrets/token.json",
            "FILE-ENCODED-TRAVERSAL",
            Severity::High,
        ),
        ("safe.txt\0.json", "FILE-NULLBYTE", Severity::Critical),
    ];

    for (path, rule_id, severity) in cases {
        let ctx = ToolCallContext::new("agent", "session", "read", json!({"path": path}));

        let findings = guardian().evaluate(&ctx);
        let finding = findings
            .iter()
            .find(|f| f.rule_id == rule_id)
            .unwrap_or_else(|| panic!("{rule_id} finding missing for {path:?}: {findings:?}"));

        assert_eq!(finding.category, "path_traversal");
        assert_eq!(finding.severity, severity);
        assert_eq!(finding.tool_name, "read");
        assert_eq!(finding.guardian, "file_guardian");
        assert_populated_finding(finding);
    }
}

#[test]
fn disabled_patterns_can_suppress_default_globs_and_system_paths() {
    let config = FileGuardConfig {
        disabled_patterns: vec!["**/.env".to_string(), "/etc/passwd".to_string()],
        ..Default::default()
    };
    let guardian = guardian_with_config(config);

    for (path, suppressed_rule) in [
        ("/workspace/project/.env", "FILE-SENSITIVE"),
        ("/etc/passwd", "FILE-SYSTEM"),
    ] {
        let ctx = ToolCallContext::new("agent", "session", "read", json!({"path": path}));
        let findings = guardian.evaluate(&ctx);

        assert!(
            !findings.iter().any(|f| f.rule_id == suppressed_rule),
            "disabled pattern should suppress {suppressed_rule} for {path:?}: {findings:?}"
        );
    }
}

#[test]
fn invalid_custom_sensitive_globs_are_skipped_without_losing_valid_patterns() {
    let config = FileGuardConfig {
        sensitive_files: vec!["[".to_string(), "**/private-token.txt".to_string()],
        ..Default::default()
    };
    let guardian = guardian_with_config(config);

    let safe_ctx =
        ToolCallContext::new("agent", "session", "read", json!({"path": "/tmp/safe.txt"}));
    assert!(
        guardian.evaluate(&safe_ctx).is_empty(),
        "invalid glob must be skipped instead of poisoning the guardian"
    );

    let sensitive_ctx = ToolCallContext::new(
        "agent",
        "session",
        "read",
        json!({"path": "/tmp/private-token.txt"}),
    );
    let findings = guardian.evaluate(&sensitive_ctx);

    assert!(
        findings.iter().any(|f| f.rule_id == "FILE-SENSITIVE"),
        "valid custom glob should still match after an invalid glob is skipped: {findings:?}"
    );
}
