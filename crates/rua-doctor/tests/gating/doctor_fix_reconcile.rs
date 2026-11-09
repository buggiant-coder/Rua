use std::fs;
use std::path::PathBuf;

use rua_doctor::fix;

fn temp_working_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("rua-doctor-test-reconcile")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn setup_workspace(wd: &std::path::Path, agent_id: &str) -> PathBuf {
    let ws = wd.join("workspaces").join(agent_id);
    fs::create_dir_all(&ws).unwrap();

    let config = serde_json::json!({
        "agents": {
            "active_agent": agent_id,
            "profiles": {
                agent_id: {
                    "id": agent_id,
                    "workspace_dir": ws.to_str().unwrap(),
                    "enabled": true
                }
            }
        }
    });
    fs::write(
        wd.join("config.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
    ws
}

#[test]
fn reconcile_is_recognized() {
    assert!(fix::is_sync_fix("reconcile-workspace-skills"));
    assert!(fix::is_noninteractive_fix("reconcile-workspace-skills"));
    assert!(fix::is_known_fix("reconcile-workspace-skills"));
    assert!(!fix::is_interactive_fix("reconcile-workspace-skills"));
    assert!(!fix::is_safe_fix("reconcile-workspace-skills"));
}

#[test]
fn reconcile_no_config_succeeds() {
    let wd = temp_working_dir("no-config");
    let result = fix::run_fix("reconcile-workspace-skills", &wd);
    assert!(result.success);
    assert!(result.message.contains("no config"));
}

#[test]
fn reconcile_empty_workspace_creates_manifest() {
    let wd = temp_working_dir("empty-ws");
    let ws = setup_workspace(&wd, "a1");

    let result = fix::run_fix("reconcile-workspace-skills", &wd);
    assert!(result.success, "result: {:?}", result);
    assert!(!result.actions.is_empty());

    let manifest_path = ws.join("skill.json");
    assert!(manifest_path.is_file(), "skill.json should be created");

    let skills_dir = ws.join("skills");
    assert!(skills_dir.is_dir(), "skills/ dir should be created");
}

#[test]
fn reconcile_adds_missing_skill_to_manifest() {
    let wd = temp_working_dir("add-skill");
    let ws = setup_workspace(&wd, "a1");

    let skill_dir = ws.join("skills").join("demo_skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "# Demo Skill\nA test skill.").unwrap();

    let result = fix::run_fix("reconcile-workspace-skills", &wd);
    assert!(result.success, "result: {:?}", result);

    let manifest_content = fs::read_to_string(ws.join("skill.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_content).unwrap();
    let skills = manifest["skills"].as_object().unwrap();
    assert!(
        skills.contains_key("demo_skill"),
        "manifest should contain demo_skill: {manifest_content}"
    );
}

#[test]
fn reconcile_removes_stale_skill_from_manifest() {
    let wd = temp_working_dir("remove-skill");
    let ws = setup_workspace(&wd, "a1");

    fs::create_dir_all(ws.join("skills")).unwrap();

    let manifest = serde_json::json!({
        "schema_version": "1",
        "version": 0,
        "skills": {
            "ghost_skill": {
                "name": "ghost_skill",
                "enabled": true,
                "channels": ["all"],
                "source": "customized"
            }
        }
    });
    fs::write(
        ws.join("skill.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let result = fix::run_fix("reconcile-workspace-skills", &wd);
    assert!(result.success, "result: {:?}", result);

    let manifest_content = fs::read_to_string(ws.join("skill.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_content).unwrap();
    let skills = manifest["skills"].as_object().unwrap();
    assert!(
        !skills.contains_key("ghost_skill"),
        "ghost_skill should be removed: {manifest_content}"
    );
}

#[test]
fn reconcile_preserves_existing_enabled_state() {
    let wd = temp_working_dir("preserve-state");
    let ws = setup_workspace(&wd, "a1");

    let skill_dir = ws.join("skills").join("my_skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "# My Skill").unwrap();

    let manifest = serde_json::json!({
        "schema_version": "1",
        "version": 0,
        "skills": {
            "my_skill": {
                "name": "my_skill",
                "enabled": true,
                "channels": ["discord"],
                "source": "customized"
            }
        }
    });
    fs::write(
        ws.join("skill.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let result = fix::run_fix("reconcile-workspace-skills", &wd);
    assert!(result.success);

    let manifest_content = fs::read_to_string(ws.join("skill.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_content).unwrap();
    let skill = &manifest["skills"]["my_skill"];
    assert_eq!(skill["enabled"], true);
}
