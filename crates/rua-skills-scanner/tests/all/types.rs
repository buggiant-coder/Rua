use std::path::PathBuf;

use rua_skills_scanner::{
    AnalyzerFailure, Finding, ScanResult, Severity, SkillFile, ThreatCategory,
};
use serde_json::json;

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
        remediation: Some("avoid shell execution".to_string()),
        analyzer: Some("pattern".to_string()),
        metadata: serde_json::Map::new(),
    }
}

#[test]
fn severity_and_threat_category_use_stable_wire_names() {
    let severities = [
        Severity::Safe,
        Severity::Info,
        Severity::Low,
        Severity::Medium,
        Severity::High,
        Severity::Critical,
    ];
    assert_eq!(
        severities.map(Severity::as_str),
        ["SAFE", "INFO", "LOW", "MEDIUM", "HIGH", "CRITICAL"]
    );

    let categories = [
        ThreatCategory::PromptInjection,
        ThreatCategory::CommandInjection,
        ThreatCategory::DataExfiltration,
        ThreatCategory::UnauthorizedToolUse,
        ThreatCategory::Obfuscation,
        ThreatCategory::HardcodedSecrets,
        ThreatCategory::SocialEngineering,
        ThreatCategory::ResourceAbuse,
        ThreatCategory::PolicyViolation,
        ThreatCategory::Malware,
        ThreatCategory::HarmfulContent,
        ThreatCategory::SkillDiscoveryAbuse,
        ThreatCategory::TransitiveTrustAbuse,
        ThreatCategory::AutonomyAbuse,
        ThreatCategory::ToolChainingAbuse,
        ThreatCategory::UnicodeSteganography,
        ThreatCategory::SupplyChainAttack,
    ];
    assert_eq!(
        categories.map(ThreatCategory::as_str),
        [
            "prompt_injection",
            "command_injection",
            "data_exfiltration",
            "unauthorized_tool_use",
            "obfuscation",
            "hardcoded_secrets",
            "social_engineering",
            "resource_abuse",
            "policy_violation",
            "malware",
            "harmful_content",
            "skill_discovery_abuse",
            "transitive_trust_abuse",
            "autonomy_abuse",
            "tool_chaining_abuse",
            "unicode_steganography",
            "supply_chain_attack",
        ]
    );

    assert_eq!(Severity::Critical.as_str(), "CRITICAL");
    assert_eq!(Severity::Safe.as_str(), "SAFE");
    assert_eq!(
        ThreatCategory::CommandInjection.as_str(),
        "command_injection"
    );
    assert_eq!(
        ThreatCategory::HardcodedSecrets.as_str(),
        "hardcoded_secrets"
    );

    assert_eq!(serde_json::to_value(Severity::High).unwrap(), json!("HIGH"));
    assert_eq!(
        serde_json::to_value(ThreatCategory::SupplyChainAttack).unwrap(),
        json!("supply_chain_attack")
    );
}

#[test]
fn severity_max_uses_security_order_not_enum_name_order() {
    assert_eq!(Severity::Medium.max(Severity::Low), Severity::Medium);
    assert_eq!(Severity::Info.max(Severity::Critical), Severity::Critical);
    assert_eq!(Severity::Safe.max(Severity::Safe), Severity::Safe);
}

#[test]
fn skill_file_from_path_uses_forward_slash_relative_paths_and_file_types() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let file = nested.join("SKILL.md");
    std::fs::write(&file, "# hello").unwrap();

    let skill_file = SkillFile::from_path(&file, dir.path());

    assert_eq!(skill_file.path, file);
    assert_eq!(skill_file.relative_path, "nested/SKILL.md");
    assert_eq!(skill_file.file_type, "markdown");
    assert_eq!(skill_file.size_bytes, "# hello".len() as u64);
    assert!(skill_file.content.is_none());
}

#[test]
fn skill_file_read_content_is_cached_and_lossy_utf8_safe() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bad.py");
    std::fs::write(&file, b"hello \xFF").unwrap();
    let mut skill_file = SkillFile::from_path(&file, dir.path());

    assert_eq!(skill_file.read_content(), "hello \u{FFFD}");
    std::fs::write(&file, "changed").unwrap();
    assert_eq!(
        skill_file.read_content(),
        "hello \u{FFFD}",
        "second read should use cached content"
    );
}

#[test]
fn skill_file_read_content_returns_empty_for_missing_files() {
    let mut skill_file = SkillFile {
        path: PathBuf::from("/tmp/rua-missing-skill-file.py"),
        relative_path: "gone.py".into(),
        file_type: "python".into(),
        size_bytes: 0,
        content: None,
    };

    assert_eq!(skill_file.read_content(), "");
    assert_eq!(skill_file.content.as_deref(), Some(""));
}

#[test]
fn skill_file_falls_back_to_filename_when_base_is_not_a_parent() {
    let path = PathBuf::from("/tmp/rua-skill-demo/run.sh");
    let skill_file = SkillFile::from_path(&path, PathBuf::from("/elsewhere").as_path());

    assert_eq!(skill_file.relative_path, "run.sh");
    assert_eq!(skill_file.file_type, "bash");
}

#[test]
fn skill_file_hidden_detection_matches_qwenpaw_path_model() {
    let visible = SkillFile {
        path: PathBuf::from("/tmp/hello.py"),
        relative_path: "hello.py".into(),
        file_type: "python".into(),
        size_bytes: 0,
        content: None,
    };
    let dotfile = SkillFile {
        path: PathBuf::from("/tmp/.env"),
        relative_path: ".env".into(),
        file_type: "other".into(),
        size_bytes: 0,
        content: None,
    };
    let hidden_dir = SkillFile {
        path: PathBuf::from("/tmp/.git/config"),
        relative_path: ".git/config".into(),
        file_type: "other".into(),
        size_bytes: 0,
        content: None,
    };
    let current_dir = SkillFile {
        path: PathBuf::from("/tmp/hello.py"),
        relative_path: "./hello.py".into(),
        file_type: "python".into(),
        size_bytes: 0,
        content: None,
    };

    assert!(!visible.is_hidden());
    assert!(dotfile.is_hidden());
    assert!(hidden_dir.is_hidden());
    assert!(!current_dir.is_hidden());
}

#[test]
fn scan_result_compute_marks_high_or_critical_findings_unsafe() {
    let result = ScanResult::compute(
        "danger".to_string(),
        "/tmp/danger".to_string(),
        vec![
            finding("low", Severity::Low),
            finding("high", Severity::High),
        ],
        0.25,
        vec!["pattern".to_string()],
        vec![AnalyzerFailure {
            analyzer: "wasm".to_string(),
            error: "unsupported".to_string(),
        }],
    );

    assert!(!result.is_safe);
    assert_eq!(result.max_severity, Severity::High);
    assert_eq!(result.findings.len(), 2);
    assert_eq!(result.analyzers_used, vec!["pattern"]);
    assert_eq!(result.analyzers_failed[0].analyzer, "wasm");
    assert!(chrono::DateTime::parse_from_rfc3339(&result.timestamp).is_ok());
}

#[test]
fn scan_result_compute_treats_medium_and_lower_as_safe() {
    let result = ScanResult::compute(
        "review".to_string(),
        "/tmp/review".to_string(),
        vec![
            finding("info", Severity::Info),
            finding("medium", Severity::Medium),
        ],
        0.0,
        Vec::new(),
        Vec::new(),
    );

    assert!(result.is_safe);
    assert_eq!(result.max_severity, Severity::Medium);
}

#[test]
fn skill_scan_error_summary_includes_top_findings_and_truncation() {
    let result = ScanResult::compute(
        "blocked-skill".to_string(),
        "/tmp/blocked".to_string(),
        (0..7)
            .map(|index| finding(&index.to_string(), Severity::Critical))
            .collect(),
        0.0,
        Vec::new(),
        Vec::new(),
    );

    let err = rua_skills_scanner::SkillScanError::from_result(&result);

    assert!(err.message.contains("blocked-skill"));
    assert!(err.message.contains("7 issue(s)"));
    assert!(err.message.contains("max severity: CRITICAL"));
    assert!(err.message.contains("Finding 0"));
    assert!(err.message.contains("and 2 more"));
    assert_eq!(err.findings.len(), 7);
}
