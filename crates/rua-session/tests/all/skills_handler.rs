use std::fs;
use std::path::Path;

use rua_session::{handle_skills_command, SkillsContext};
use tempfile::tempdir;

fn write_skill(skill_root: &Path, folder: &str, frontmatter: &str) {
    let dir = skill_root.join(folder);
    fs::create_dir_all(&dir).unwrap();
    let mut content = String::from("---\n");
    content.push_str(frontmatter);
    content.push_str("\n---\n");
    content.push_str("body");
    fs::write(dir.join("SKILL.md"), content).unwrap();
}

fn write_manifest(workspace_dir: &Path, manifest: &str) {
    fs::write(workspace_dir.join("skill.json"), manifest).unwrap();
}

#[test]
fn missing_workspace_reports_error() {
    let ctx = SkillsContext {
        workspace_dir: None,
        channel_id: "console".into(),
        locale: "en".into(),
    };
    let out = handle_skills_command(&ctx);
    assert!(out.contains("Workspace not initialized"), "got: {out}");
}

#[test]
fn empty_manifest_reports_no_skills() {
    let dir = tempdir().unwrap();
    let ctx = SkillsContext {
        workspace_dir: Some(dir.path().to_path_buf()),
        channel_id: "console".into(),
        locale: "en".into(),
    };
    let out = handle_skills_command(&ctx);
    assert!(
        out.contains("No skills are currently enabled"),
        "got: {out}"
    );
}

#[test]
fn lists_enabled_skill_with_frontmatter_description() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    let skills_root = ws.join("skills");
    fs::create_dir_all(&skills_root).unwrap();

    write_skill(
        &skills_root,
        "weekly_report",
        "name: weekly_report\ndescription: Generate the weekly status report from all team channels, including project updates, blockers, completed tasks, and upcoming milestones for review",
    );

    let manifest = r#"{
        "schema_version": "workspace-skill-manifest.v1",
        "version": 1,
        "skills": {
            "weekly_report": {
                "enabled": true,
                "channels": ["all"],
                "source": {"type": "builtin"},
                "metadata": {
                    "name": "weekly_report",
                    "description": "stale manifest text",
                    "source": {"type": "builtin"}
                }
            }
        }
    }"#;
    write_manifest(ws, manifest);

    let ctx = SkillsContext {
        workspace_dir: Some(ws.to_path_buf()),
        channel_id: "console".into(),
        locale: "en".into(),
    };
    let out = handle_skills_command(&ctx);

    assert!(out.contains("**weekly_report**"), "name missing in: {out}");

    assert!(
        out.contains("Generate the weekly status report from all team channels, including project updates, blockers, completed tasks, and u..."),
        "frontmatter description not preferred / not truncated in: {out}"
    );
    assert!(
        !out.contains("stale manifest text"),
        "manifest description must lose to SKILL.md frontmatter: {out}"
    );
    assert!(
        out.contains("Use `/<skill_name>`"),
        "trailing usage hint missing in: {out}"
    );
}

#[test]
fn disabled_skill_excluded_so_status_distinguishes() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    let skills_root = ws.join("skills");
    fs::create_dir_all(&skills_root).unwrap();

    write_skill(
        &skills_root,
        "active",
        "name: active\ndescription: Active skill",
    );
    write_skill(
        &skills_root,
        "disabled_one",
        "name: disabled_one\ndescription: Should not show",
    );

    let manifest = r#"{
        "schema_version": "workspace-skill-manifest.v1",
        "version": 2,
        "skills": {
            "active": {
                "enabled": true,
                "channels": ["all"],
                "source": {"type": "builtin"},
                "metadata": {"name": "active", "source": {"type": "builtin"}}
            },
            "disabled_one": {
                "enabled": false,
                "channels": ["all"],
                "source": {"type": "builtin"},
                "metadata": {"name": "disabled_one", "source": {"type": "builtin"}}
            }
        }
    }"#;
    write_manifest(ws, manifest);

    let ctx = SkillsContext {
        workspace_dir: Some(ws.to_path_buf()),
        channel_id: "console".into(),
        locale: "en".into(),
    };
    let out = handle_skills_command(&ctx);

    assert!(out.contains("**active**"), "enabled skill missing: {out}");
    assert!(
        !out.contains("disabled_one"),
        "disabled skill leaked into output: {out}"
    );
}

#[test]
fn channel_routing_filters_skills() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    let skills_root = ws.join("skills");
    fs::create_dir_all(&skills_root).unwrap();

    write_skill(
        &skills_root,
        "discord_only",
        "name: discord_only\ndescription: Only on discord",
    );
    write_skill(
        &skills_root,
        "all_channels",
        "name: all_channels\ndescription: Everywhere",
    );

    let manifest = r#"{
        "schema_version": "workspace-skill-manifest.v1",
        "version": 3,
        "skills": {
            "discord_only": {
                "enabled": true,
                "channels": ["discord"],
                "source": {"type": "builtin"},
                "metadata": {"name": "discord_only", "source": {"type": "builtin"}}
            },
            "all_channels": {
                "enabled": true,
                "channels": ["all"],
                "source": {"type": "builtin"},
                "metadata": {"name": "all_channels", "source": {"type": "builtin"}}
            }
        }
    }"#;
    write_manifest(ws, manifest);

    let ctx = SkillsContext {
        workspace_dir: Some(ws.to_path_buf()),
        channel_id: "console".into(),
        locale: "en".into(),
    };
    let out = handle_skills_command(&ctx);

    assert!(
        out.contains("all_channels"),
        "channel=all should be visible from console: {out}"
    );
    assert!(
        !out.contains("discord_only"),
        "channel=discord must be hidden from console: {out}"
    );
}

#[test]
fn empty_description_falls_back_to_no_description_marker() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    let skills_root = ws.join("skills");
    fs::create_dir_all(&skills_root).unwrap();
    write_skill(&skills_root, "blank_desc", "name: blank_desc");

    let manifest = r#"{
        "schema_version": "workspace-skill-manifest.v1",
        "version": 4,
        "skills": {
            "blank_desc": {
                "enabled": true,
                "channels": ["all"],
                "source": {"type": "builtin"},
                "metadata": {
                    "name": "blank_desc",
                    "description": "",
                    "source": {"type": "builtin"}
                }
            }
        }
    }"#;
    write_manifest(ws, manifest);

    let ctx = SkillsContext {
        workspace_dir: Some(ws.to_path_buf()),
        channel_id: "console".into(),
        locale: "en".into(),
    };
    let out = handle_skills_command(&ctx);

    assert!(out.contains("**blank_desc**"), "name missing in: {out}");
    assert!(
        out.contains("No description."),
        "blank description should fall back to 'No description.' in: {out}"
    );
}

#[test]
fn skill_dir_without_skill_md_is_dropped_by_reconcile() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    let skills_root = ws.join("skills");
    fs::create_dir_all(skills_root.join("no_md_skill")).unwrap();

    let manifest = r#"{
        "schema_version": "workspace-skill-manifest.v1",
        "version": 5,
        "skills": {
            "no_md_skill": {
                "enabled": true,
                "channels": ["all"],
                "source": {"type": "builtin"},
                "metadata": {
                    "name": "no_md_skill",
                    "description": "should be dropped",
                    "source": {"type": "builtin"}
                }
            }
        }
    }"#;
    write_manifest(ws, manifest);

    let ctx = SkillsContext {
        workspace_dir: Some(ws.to_path_buf()),
        channel_id: "console".into(),
        locale: "en".into(),
    };
    let out = handle_skills_command(&ctx);

    assert!(
        out.contains("No skills are currently enabled"),
        "skill dir without SKILL.md must not be listed: {out}"
    );
}
