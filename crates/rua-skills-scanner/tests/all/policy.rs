use std::io::Write;

use rua_skills_scanner::{ScanMode, ScanPolicy, Severity};

#[test]
fn default_policy_has_documentation_indicators() {
    let policy = ScanPolicy::default();
    assert!(policy.is_doc_path("docs/SKILL.md"));
    assert!(policy.is_doc_path("examples/example.py"));
    assert!(policy.is_doc_path("DOCS/Guide.md"));
    assert!(!policy.is_doc_path("src/main.py"));
}

#[test]
fn default_policy_skips_known_inert_extensions() {
    let policy = ScanPolicy::default();
    assert!(policy.should_skip_extension(".png"));
    assert!(policy.should_skip_extension(".woff2"));
    assert!(policy.should_skip_extension(".zip"));
    assert!(!policy.should_skip_extension(".py"));
}

#[test]
fn default_policy_dedupes_findings() {
    let policy = ScanPolicy::default();
    assert!(policy.rule_scoping.dedupe_duplicate_findings);
}

#[test]
fn policy_helpers_expose_disabled_rules_and_severity_overrides() {
    let mut policy = ScanPolicy::default();
    policy
        .disabled_rules
        .insert("COMMAND_INJECTION_EVAL".to_string());
    policy
        .severity_overrides
        .insert("HARDCODED_SECRET_HIGH_ENTROPY".to_string(), Severity::Low);

    assert!(policy.is_rule_disabled("COMMAND_INJECTION_EVAL"));
    assert!(!policy.is_rule_disabled("COMMAND_INJECTION_OS_SYSTEM"));
    assert_eq!(
        policy.get_severity_override("HARDCODED_SECRET_HIGH_ENTROPY"),
        Some(Severity::Low)
    );
    assert_eq!(policy.get_severity_override("UNKNOWN_RULE"), None);
}

#[test]
fn scan_mode_parses_known_strings() {
    assert_eq!(ScanMode::parse("off"), Some(ScanMode::Off));
    assert_eq!(ScanMode::parse("WARN"), Some(ScanMode::Warn));
    assert_eq!(ScanMode::parse(" Block "), Some(ScanMode::Block));
    assert_eq!(ScanMode::parse("foo"), None);
}

#[test]
fn preset_balanced_loads() {
    let policy = ScanPolicy::from_preset("balanced").unwrap();
    assert_eq!(policy.file_limits.max_file_count, 500);
    assert_eq!(policy.file_limits.max_file_size_bytes, 10_485_760);
    assert!(policy.severity_overrides.is_empty());
}

#[test]
fn preset_paranoid_loads_with_strict_limits() {
    let policy = ScanPolicy::from_preset("paranoid").unwrap();
    assert_eq!(policy.file_limits.max_file_count, 50);
    assert_eq!(policy.file_limits.max_file_size_bytes, 1_048_576);
    assert!(!policy.severity_overrides.is_empty());
    assert_eq!(
        policy.severity_overrides.get("COMMAND_INJECTION_EVAL"),
        Some(&Severity::Critical)
    );
}

#[test]
fn preset_permissive_loads_with_relaxed_limits() {
    let policy = ScanPolicy::from_preset("permissive").unwrap();
    assert_eq!(policy.file_limits.max_file_count, 2000);
    assert_eq!(policy.file_limits.max_file_size_bytes, 52_428_800);
    assert_eq!(
        policy.severity_overrides.get("COMMAND_INJECTION_EVAL"),
        Some(&Severity::Medium)
    );
    assert_eq!(
        policy.severity_overrides.get("OBFUSCATION_BASE64_DECODE"),
        Some(&Severity::Info)
    );
}

#[test]
fn presets_have_severity_differences() {
    let paranoid = ScanPolicy::from_preset("paranoid").unwrap();
    let balanced = ScanPolicy::from_preset("balanced").unwrap();
    let permissive = ScanPolicy::from_preset("permissive").unwrap();

    assert!(paranoid.file_limits.max_file_count < balanced.file_limits.max_file_count);
    assert!(balanced.file_limits.max_file_count < permissive.file_limits.max_file_count);

    assert!(!paranoid.severity_overrides.is_empty());
    assert!(balanced.severity_overrides.is_empty());
    assert!(!permissive.severity_overrides.is_empty());
}

#[test]
fn preset_case_insensitive() {
    assert!(ScanPolicy::from_preset("PARANOID").is_ok());
    assert!(ScanPolicy::from_preset("Balanced").is_ok());
    assert!(ScanPolicy::from_preset("PERMISSIVE").is_ok());
}

#[test]
fn preset_unknown_returns_err() {
    let result = ScanPolicy::from_preset("nonexistent");
    assert!(result.is_err());
}

#[test]
fn preset_names_lists_all() {
    let names = ScanPolicy::preset_names();
    assert_eq!(names, &["paranoid", "balanced", "permissive"]);
}

#[test]
fn from_file_loads_valid_yaml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_policy.yaml");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(
        f,
        r#"
rule_scoping:
  skip_in_docs: []
  code_only: []
  doc_path_indicators:
    - "docs"
  dedupe_duplicate_findings: false
credentials:
  known_test_values: []
  placeholder_markers: []
file_classification:
  inert_extensions: []
  archive_extensions: []
file_limits:
  max_file_count: 100
  max_file_size_bytes: 999
severity_overrides: {{}}
disabled_rules: []
"#
    )
    .unwrap();

    let policy = ScanPolicy::from_file(&path).unwrap();
    assert_eq!(policy.file_limits.max_file_size_bytes, 999);
    assert_eq!(policy.file_limits.max_file_count, 100);
    assert!(!policy.rule_scoping.dedupe_duplicate_findings);
}

#[test]
fn from_file_loads_security_switches_from_yaml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("security_policy.yaml");
    std::fs::write(
        &path,
        r#"
rule_scoping:
  skip_in_docs:
    - PROMPT_INJECTION_IGNORE_INSTRUCTIONS
  code_only:
    - COMMAND_INJECTION_EVAL
  doc_path_indicators:
    - docs
  dedupe_duplicate_findings: false
severity_overrides:
  COMMAND_INJECTION_EVAL: LOW
disabled_rules:
  - SECRET_STRIPE_KEY
"#,
    )
    .unwrap();

    let policy = ScanPolicy::from_file(&path).unwrap();

    assert!(policy
        .rule_scoping
        .skip_in_docs
        .contains("PROMPT_INJECTION_IGNORE_INSTRUCTIONS"));
    assert!(policy
        .rule_scoping
        .code_only
        .contains("COMMAND_INJECTION_EVAL"));
    assert!(!policy.rule_scoping.dedupe_duplicate_findings);
    assert_eq!(
        policy.get_severity_override("COMMAND_INJECTION_EVAL"),
        Some(Severity::Low)
    );
    assert!(policy.is_rule_disabled("SECRET_STRIPE_KEY"));
}

#[test]
fn from_file_missing_returns_err() {
    let result = ScanPolicy::from_file(std::path::Path::new("/tmp/no_such_policy_1234.yaml"));
    assert!(result.is_err());
}

#[test]
fn from_file_invalid_yaml_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.yaml");
    std::fs::write(&path, "not: [valid: yaml: {{{{").unwrap();
    let result = ScanPolicy::from_file(&path);
    assert!(result.is_err());
}

#[test]
fn from_file_partial_yaml_keeps_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.yaml");
    std::fs::write(&path, "file_limits:\n  max_file_count: 42\n").unwrap();

    let policy = ScanPolicy::from_file(&path).unwrap();
    let defaults = ScanPolicy::default();

    assert_eq!(policy.file_limits.max_file_count, 42);
    assert_eq!(
        policy.file_limits.max_file_size_bytes,
        defaults.file_limits.max_file_size_bytes
    );
    assert_eq!(
        policy.rule_scoping.dedupe_duplicate_findings,
        defaults.rule_scoping.dedupe_duplicate_findings
    );
    assert_eq!(
        policy.credentials.known_test_values,
        defaults.credentials.known_test_values
    );
    assert_eq!(
        policy.file_classification.inert_extensions,
        defaults.file_classification.inert_extensions
    );
}

#[test]
fn from_file_user_array_replaces_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("custom_rules.yaml");
    std::fs::write(
        &path,
        r#"
credentials:
  known_test_values:
    - "CUSTOM_KEY_1"
"#,
    )
    .unwrap();

    let policy = ScanPolicy::from_file(&path).unwrap();
    assert_eq!(policy.credentials.known_test_values.len(), 1);
    assert!(policy
        .credentials
        .known_test_values
        .contains("CUSTOM_KEY_1"));
    let defaults = ScanPolicy::default();
    assert!(!policy
        .credentials
        .known_test_values
        .contains("AKIAIOSFODNN7EXAMPLE"));
    assert_eq!(
        policy.credentials.placeholder_markers,
        defaults.credentials.placeholder_markers
    );
}

#[test]
fn from_file_empty_yaml_returns_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.yaml");
    std::fs::write(&path, "").unwrap();

    let policy = ScanPolicy::from_file(&path).unwrap();
    let defaults = ScanPolicy::default();
    assert_eq!(
        policy.file_limits.max_file_count,
        defaults.file_limits.max_file_count
    );
    assert_eq!(
        policy.credentials.known_test_values,
        defaults.credentials.known_test_values
    );
    assert_eq!(
        policy.rule_scoping.dedupe_duplicate_findings,
        defaults.rule_scoping.dedupe_duplicate_findings
    );
}

#[test]
fn from_file_loads_disabled_rules_and_severity_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rule_policy.yaml");
    std::fs::write(
        &path,
        r#"
rule_scoping:
  skip_in_docs:
    - "COMMAND_INJECTION_EVAL"
  code_only:
    - "COMMAND_INJECTION_OS_SYSTEM"
severity_overrides:
  COMMAND_INJECTION_EVAL: LOW
  HARDCODED_SECRET_HIGH_ENTROPY: CRITICAL
disabled_rules:
  - "OBFUSCATION_BASE64_DECODE"
"#,
    )
    .unwrap();

    let policy = ScanPolicy::from_file(&path).unwrap();

    assert!(policy
        .rule_scoping
        .skip_in_docs
        .contains("COMMAND_INJECTION_EVAL"));
    assert!(policy
        .rule_scoping
        .code_only
        .contains("COMMAND_INJECTION_OS_SYSTEM"));
    assert!(policy.is_rule_disabled("OBFUSCATION_BASE64_DECODE"));
    assert_eq!(
        policy.get_severity_override("COMMAND_INJECTION_EVAL"),
        Some(Severity::Low)
    );
    assert_eq!(
        policy.get_severity_override("HARDCODED_SECRET_HIGH_ENTROPY"),
        Some(Severity::Critical)
    );
}
