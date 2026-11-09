use std::fs;
use std::path::PathBuf;

use rua_doctor::fix;

fn temp_working_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("rua-doctor-test-validate")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_config_with_agent(wd: &std::path::Path, agent_id: &str) -> PathBuf {
    let ws_dir = wd.join("workspaces").join(agent_id);
    fs::create_dir_all(&ws_dir).unwrap();

    let config = serde_json::json!({
        "agents": {
            "active_agent": agent_id,
            "profiles": {
                agent_id: {
                    "id": agent_id,
                    "workspace_dir": ws_dir.to_str().unwrap(),
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
    ws_dir
}

#[test]
fn validate_all_jobs_json_is_recognized() {
    assert!(fix::is_safe_fix("validate-all-jobs-json"));
    assert!(fix::is_noninteractive_fix("validate-all-jobs-json"));
    assert!(fix::is_known_fix("validate-all-jobs-json"));
    assert!(!fix::is_interactive_fix("validate-all-jobs-json"));
}

#[test]
fn validate_no_config_succeeds() {
    let wd = temp_working_dir("no-config");
    let result = fix::run_fix("validate-all-jobs-json", &wd);
    assert!(result.success);
    assert!(result.message.contains("no config"));
}

#[test]
fn validate_no_jobs_files_succeeds() {
    let wd = temp_working_dir("no-jobs");
    write_config_with_agent(&wd, "test-agent");
    let result = fix::run_fix("validate-all-jobs-json", &wd);
    assert!(result.success);
    assert!(result.message.contains("no jobs.json"));
}

#[test]
fn validate_valid_jobs_json_ok() {
    let wd = temp_working_dir("valid-jobs");
    let ws = wd.join("workspaces").join("a1");
    fs::create_dir_all(&ws).unwrap();

    let config = serde_json::json!({
        "agents": {
            "active_agent": "a1",
            "profiles": {
                "a1": {
                    "id": "a1",
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

    let jobs = serde_json::json!({
        "version": 1,
        "jobs": {
            "j1": {
                "id": "j1",
                "name": "test-job",
                "enabled": true,
                "schedule": { "cron": "0 * * * *" },
                "agent_id": "a1",
                "prompt": "hello"
            }
        }
    });
    fs::write(
        ws.join("jobs.json"),
        serde_json::to_string_pretty(&jobs).unwrap(),
    )
    .unwrap();

    let result = fix::run_fix("validate-all-jobs-json", &wd);
    assert!(result.success, "result: {:?}", result);
    assert!(result.message.contains("OK"));
    assert!(!result.actions.is_empty());
    assert!(result.actions[0].applied);
}

#[test]
fn validate_corrupted_jobs_json_fails() {
    let wd = temp_working_dir("bad-jobs");
    let ws = wd.join("workspaces").join("a1");
    fs::create_dir_all(&ws).unwrap();

    let config = serde_json::json!({
        "agents": {
            "active_agent": "a1",
            "profiles": {
                "a1": {
                    "id": "a1",
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

    fs::write(ws.join("jobs.json"), "{ not valid json !!!").unwrap();

    let result = fix::run_fix("validate-all-jobs-json", &wd);
    assert!(!result.success, "corrupted JSON should fail: {:?}", result);
    assert!(!result.actions.is_empty());
    assert!(!result.actions[0].applied);
    assert!(result.actions[0].error.is_some());
}

#[test]
fn validate_root_jobs_json_also_checked() {
    let wd = temp_working_dir("root-jobs");

    let config = serde_json::json!({
        "agents": {
            "active_agent": "",
            "profiles": {}
        }
    });
    fs::write(
        wd.join("config.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();

    let jobs = serde_json::json!({
        "version": 1,
        "jobs": {}
    });
    fs::write(
        wd.join("jobs.json"),
        serde_json::to_string_pretty(&jobs).unwrap(),
    )
    .unwrap();

    let result = fix::run_fix("validate-all-jobs-json", &wd);
    assert!(result.success);
    assert!(result
        .actions
        .iter()
        .any(|a| a.description.contains("(root)")));
}
