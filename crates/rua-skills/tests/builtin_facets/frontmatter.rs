use std::path::PathBuf;

use rua_skills::{
    extract_requirements, parse_skill_md, parse_skill_md_file, parse_skill_md_safe,
    REQUIREMENTS_METADATA_NAMESPACES,
};

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

#[test]
fn namespaces_in_priority_order() {
    assert_eq!(
        REQUIREMENTS_METADATA_NAMESPACES,
        &["openclaw", "rua", "clawdbot"]
    );
}

#[test]
fn parses_valid_skill_with_openclaw_namespace_requires() {
    let parsed = parse_skill_md_file(&fixture_dir("valid_skill")).unwrap();
    assert_eq!(parsed.name, "himalaya");
    assert_eq!(parsed.description, "CLI to manage emails via IMAP/SMTP.");
    assert_eq!(parsed.version, "1.1");
    assert_eq!(
        parsed.triggers,
        vec![
            "check email".to_string(),
            "send email".to_string(),
            "list inbox".to_string(),
        ]
    );
    assert_eq!(
        parsed.requirements.require_bins,
        vec!["himalaya".to_string()]
    );
    assert_eq!(
        parsed.requirements.require_envs,
        vec!["IMAP_PASSWORD".to_string()]
    );
    assert_eq!(parsed.emoji, "📧");
    assert!(parsed.body.starts_with("# Himalaya Email CLI"));
}

#[test]
fn parses_minimal_skill_with_no_metadata_block() {
    let parsed = parse_skill_md_file(&fixture_dir("minimal_skill")).unwrap();
    assert_eq!(parsed.name, "minimal");
    assert_eq!(parsed.description, "A minimal skill");
    assert_eq!(parsed.version, "");
    assert!(parsed.requirements.require_bins.is_empty());
    assert!(parsed.requirements.require_envs.is_empty());
    assert!(parsed.body.contains("Minimal Skill"));
}

#[test]
fn missing_name_falls_back_to_directory_name() {
    let parsed = parse_skill_md_file(&fixture_dir("missing_name_skill")).unwrap();
    assert_eq!(parsed.name, "missing_name_skill");
    assert_eq!(parsed.description, "A skill without a name field");
}

#[test]
fn invalid_yaml_returns_err_for_strict_parser() {
    let content =
        std::fs::read_to_string(fixture_dir("invalid_yaml_skill").join("SKILL.md")).unwrap();
    let res = parse_skill_md(&content);
    assert!(res.is_err(), "expected YAML error for invalid frontmatter");
}

#[test]
fn invalid_yaml_falls_back_with_safe_parser() {
    let content =
        std::fs::read_to_string(fixture_dir("invalid_yaml_skill").join("SKILL.md")).unwrap();
    let parsed = parse_skill_md_safe(&content, "invalid_yaml_skill");
    assert_eq!(parsed.name, "invalid_yaml_skill");
    assert_eq!(parsed.description, "");
    assert!(parsed.requirements.require_bins.is_empty());
}

#[test]
fn list_form_requires_at_top_level() {
    let content = "---\nname: demo\nrequires:\n  - git\n  - node\n---\n\nbody\n";
    let parsed = parse_skill_md(content).unwrap();
    assert_eq!(
        parsed.requirements.require_bins,
        vec!["git".to_string(), "node".to_string()]
    );
    assert!(parsed.requirements.require_envs.is_empty());
}

#[test]
fn dict_form_requires_at_top_level() {
    let content =
        "---\nname: demo\nrequires:\n  bins:\n    - git\n  env:\n    - GH_TOKEN\n---\n\nbody\n";
    let parsed = parse_skill_md(content).unwrap();
    assert_eq!(parsed.requirements.require_bins, vec!["git".to_string()]);
    assert_eq!(
        parsed.requirements.require_envs,
        vec!["GH_TOKEN".to_string()]
    );
}

#[test]
fn metadata_requires_takes_precedence_over_top_level() {
    let content = r#"---
name: demo
requires: [outer-only]
metadata:
  requires:
    bins: [inner-bin]
    env: [INNER_ENV]
---

body
"#;
    let parsed = parse_skill_md(content).unwrap();
    assert_eq!(
        parsed.requirements.require_bins,
        vec!["inner-bin".to_string()]
    );
    assert_eq!(
        parsed.requirements.require_envs,
        vec!["INNER_ENV".to_string()]
    );
}

#[test]
fn rua_namespace_takes_precedence_over_metadata_requires() {
    let content = r#"---
name: demo
metadata:
  rua:
    requires:
      bins: [from-rua]
  requires:
    bins: [from-metadata]
---

body
"#;
    let parsed = parse_skill_md(content).unwrap();
    assert_eq!(
        parsed.requirements.require_bins,
        vec!["from-rua".to_string()]
    );
}

#[test]
fn openclaw_namespace_wins_over_rua_and_clawdbot() {
    let content = r#"---
name: demo
metadata:
  clawdbot:
    requires:
      bins: [from-clawdbot]
  rua:
    requires:
      bins: [from-rua]
  openclaw:
    requires:
      bins: [from-openclaw]
---

body
"#;
    let parsed = parse_skill_md(content).unwrap();
    assert_eq!(
        parsed.requirements.require_bins,
        vec!["from-openclaw".to_string()]
    );
}

#[test]
fn clawdbot_namespace_wins_when_openclaw_and_rua_absent() {
    let content = r#"---
name: demo
metadata:
  clawdbot:
    requires:
      bins: [from-clawdbot]
---

body
"#;
    let parsed = parse_skill_md(content).unwrap();
    assert_eq!(
        parsed.requirements.require_bins,
        vec!["from-clawdbot".to_string()]
    );
}

#[test]
fn empty_frontmatter_body_only() {
    let content = "no frontmatter here\njust markdown body\n";
    let parsed = parse_skill_md(content).unwrap();
    assert!(parsed.name.is_empty());
    assert!(parsed.description.is_empty());
    assert_eq!(parsed.body, content);
}

#[test]
fn version_falls_back_to_metadata_builtin_skill_version() {
    let content = r#"---
name: demo
metadata:
  builtin_skill_version: "2.7"
---

body
"#;
    let parsed = parse_skill_md(content).unwrap();
    assert_eq!(parsed.version, "2.7");
}

#[test]
fn version_top_level_wins_over_metadata_version() {
    let content = r#"---
name: demo
version: "9.9"
metadata:
  version: "1.0"
  builtin_skill_version: "0.1"
---

body
"#;
    let parsed = parse_skill_md(content).unwrap();
    assert_eq!(parsed.version, "9.9");
}

#[test]
fn extract_requirements_helper_handles_empty_input() {
    let req = extract_requirements(&serde_json::Map::new());
    assert!(req.require_bins.is_empty());
    assert!(req.require_envs.is_empty());
}

#[test]
fn extract_requirements_helper_directly() {
    let raw: serde_json::Value = serde_json::from_str(
        r#"{
            "metadata": {
                "openclaw": {
                    "requires": {
                        "bins": ["b1"],
                        "env": ["E1"]
                    }
                }
            }
        }"#,
    )
    .unwrap();
    let req = extract_requirements(raw.as_object().unwrap());
    assert_eq!(req.require_bins, vec!["b1".to_string()]);
    assert_eq!(req.require_envs, vec!["E1".to_string()]);
}
