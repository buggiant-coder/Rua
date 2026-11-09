use std::fs;
use std::path::Path;

use rua_skills::{channel_matches, resolve_effective_skills, ALL_SKILL_ROUTING_CHANNELS};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn all_routing_channels_lists_ten_canonical_channels() {
    let expected = [
        "console",
        "discord",
        "telegram",
        "dingtalk",
        "feishu",
        "imessage",
        "qq",
        "mattermost",
        "wecom",
        "mqtt",
    ];
    assert_eq!(
        ALL_SKILL_ROUTING_CHANNELS.len(),
        expected.len(),
        "channel set must be exactly 10 entries"
    );
    for c in expected.iter() {
        assert!(
            ALL_SKILL_ROUTING_CHANNELS.contains(c),
            "missing channel {c}",
        );
    }
}

#[test]
fn channel_matches_all_keyword_visible_everywhere() {
    let channels = vec!["all".to_string()];
    assert!(channel_matches(&channels, "telegram"));
    assert!(channel_matches(&channels, "console"));
    assert!(channel_matches(&channels, "anything-else"));
}

#[test]
fn channel_matches_specific_names_only_match_listed() {
    let channels = vec!["telegram".to_string(), "discord".to_string()];
    assert!(channel_matches(&channels, "telegram"));
    assert!(channel_matches(&channels, "discord"));
    assert!(!channel_matches(&channels, "console"));
    assert!(!channel_matches(&channels, "dingtalk"));
}

#[test]
fn channel_matches_empty_list_treated_as_all() {
    let channels: Vec<String> = Vec::new();
    assert!(channel_matches(&channels, "console"));
}

#[test]
fn resolve_effective_skills_empty_when_no_manifest() {
    let dir = TempDir::new().unwrap();
    let resolved = resolve_effective_skills(dir.path(), "console");
    assert!(resolved.is_empty());
}

#[test]
fn resolve_effective_skills_filters_by_enabled_channel_and_disk() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path();
    let skills_root = workspace.join("skills");
    write_skill_md(&skills_root.join("alpha"), "alpha", "Alpha");
    write_skill_md(&skills_root.join("beta"), "beta", "Beta");
    write_skill_md(&skills_root.join("gamma"), "gamma", "Gamma");

    let manifest = json!({
        "schema_version": "workspace-skill-manifest.v1",
        "version": 1,
        "skills": {
            "alpha": {
                "enabled": true,
                "channels": ["all"],
                "source": "customized",
                "metadata": {"name": "alpha", "source": "customized"}
            },
            "beta": {
                "enabled": true,
                "channels": ["telegram"],
                "source": "customized",
                "metadata": {"name": "beta", "source": "customized"}
            },
            "gamma": {
                "enabled": false,
                "channels": ["all"],
                "source": "customized",
                "metadata": {"name": "gamma", "source": "customized"}
            },
            "orphan": {
                "enabled": true,
                "channels": ["all"],
                "source": "customized",
                "metadata": {"name": "orphan", "source": "customized"}
            }
        }
    });
    fs::write(
        workspace.join("skill.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let console = resolve_effective_skills(workspace, "console");
    assert_eq!(console, vec!["alpha".to_string()]);

    let telegram = resolve_effective_skills(workspace, "telegram");
    assert_eq!(telegram, vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn resolve_effective_skills_returns_lexicographic_order() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path();
    let skills_root = workspace.join("skills");

    for name in ["zeta", "alpha", "mu"].iter() {
        write_skill_md(&skills_root.join(name), name, name);
    }
    let manifest = json!({
        "schema_version": "workspace-skill-manifest.v1",
        "version": 1,
        "skills": {
            "zeta": {
                "enabled": true,
                "channels": ["all"],
                "source": "customized",
                "metadata": {"name": "zeta", "source": "customized"}
            },
            "alpha": {
                "enabled": true,
                "channels": ["all"],
                "source": "customized",
                "metadata": {"name": "alpha", "source": "customized"}
            },
            "mu": {
                "enabled": true,
                "channels": ["all"],
                "source": "customized",
                "metadata": {"name": "mu", "source": "customized"}
            }
        }
    });
    fs::write(
        workspace.join("skill.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let resolved = resolve_effective_skills(workspace, "console");
    assert_eq!(
        resolved,
        vec!["alpha".to_string(), "mu".to_string(), "zeta".to_string()]
    );
}

fn write_skill_md(dir: &Path, name: &str, description: &str) {
    fs::create_dir_all(dir).unwrap();
    let body = format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n");
    fs::write(dir.join("SKILL.md"), body).unwrap();
}
