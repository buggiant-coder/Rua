use std::collections::HashSet;

use rua_security::builtin_structured_rules;

#[test]
fn builtin_structured_rule_count_is_27() {
    assert_eq!(
        builtin_structured_rules().len(),
        27,
        "expected 27 structured built-in rules (20 original + 7 SHELL_EVASION)"
    );
}

#[test]
fn every_rule_has_unique_id_and_valid_prefix() {
    let rules = builtin_structured_rules();
    let mut seen_ids: HashSet<&str> = HashSet::with_capacity(rules.len());

    for rule in &rules {
        assert!(!rule.id.is_empty(), "rule id must not be empty");
        assert!(
            rule.id.starts_with("TOOL_CMD_") || rule.id.starts_with("SHELL_EVASION_"),
            "rule id {} should follow the TOOL_CMD_* or SHELL_EVASION_* convention",
            rule.id
        );
        assert!(
            seen_ids.insert(&rule.id),
            "duplicate rule id detected: {}",
            rule.id
        );
    }
}

#[test]
fn important_builtin_rules_are_present() {
    let rules = builtin_structured_rules();
    let ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();

    for must_have in [
        "TOOL_CMD_DANGEROUS_RM",
        "TOOL_CMD_FS_DESTRUCTION",
        "TOOL_CMD_REVERSE_SHELL",
        "TOOL_CMD_PRIVILEGE_ESCALATION",
        "TOOL_CMD_SYSTEM_REBOOT",
    ] {
        assert!(
            ids.contains(&must_have),
            "{must_have} should be present in builtin_structured_rules()"
        );
    }
}
