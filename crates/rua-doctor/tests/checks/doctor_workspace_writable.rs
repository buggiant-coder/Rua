use rua_config::dirs::Paths;
use rua_doctor::registry::{run_all_checks, CheckContext, CheckStatus};

fn write_config_with_agent(wd: &std::path::Path, workspace_dir: &str, enabled: bool) {
    let config = serde_json::json!({
        "agents": {
            "active_agent": "test-agent",
            "profiles": {
                "test-agent": {
                    "id": "test-agent",
                    "workspace_dir": workspace_dir,
                    "enabled": enabled
                }
            }
        }
    });
    std::fs::write(
        wd.join("config.json"),
        serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
}

#[test]
fn workspace_writable_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path().to_path_buf();
    let ws = wd.join("workspaces").join("test-agent");
    std::fs::create_dir_all(&ws).unwrap();
    write_config_with_agent(&wd, ws.to_str().unwrap(), true);

    let paths = Paths {
        rua_root: wd.clone(),
        secret_dir: wd.join(".secret"),
        cache_dir: wd.join(".cache"),
    };
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let check = results
        .iter()
        .find(|r| r.name == "agent_workspace_writable")
        .expect("agent_workspace_writable check must be registered");
    assert_eq!(check.status, CheckStatus::Pass);
    assert!(check.message.contains("1 agent workspace(s) writable"));

    let leftover: Vec<_> = std::fs::read_dir(&ws)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with(".rua-writable-test-"))
                .unwrap_or(false)
        })
        .collect();
    assert!(leftover.is_empty(), "temp probe files must be cleaned up");
}

#[cfg(unix)]
#[test]
fn workspace_writable_fail_when_readonly() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path().to_path_buf();
    let ws = wd.join("workspaces").join("test-agent");
    std::fs::create_dir_all(&ws).unwrap();
    write_config_with_agent(&wd, ws.to_str().unwrap(), true);
    std::fs::set_permissions(&ws, std::fs::Permissions::from_mode(0o444)).unwrap();

    let paths = Paths {
        rua_root: wd.clone(),
        secret_dir: wd.join(".secret"),
        cache_dir: wd.join(".cache"),
    };
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let check = results
        .iter()
        .find(|r| r.name == "agent_workspace_writable")
        .expect("agent_workspace_writable check must be registered");
    assert_eq!(check.status, CheckStatus::Fail);
    assert!(
        check.message.contains("test-agent"),
        "fail message must name failing agent"
    );

    std::fs::set_permissions(&ws, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn workspace_writable_skip_disabled_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path().to_path_buf();
    let ws = wd.join("workspaces").join("test-agent");
    std::fs::create_dir_all(&ws).unwrap();
    write_config_with_agent(&wd, ws.to_str().unwrap(), false);

    let paths = Paths {
        rua_root: wd.clone(),
        secret_dir: wd.join(".secret"),
        cache_dir: wd.join(".cache"),
    };
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let check = results
        .iter()
        .find(|r| r.name == "agent_workspace_writable")
        .expect("agent_workspace_writable check must be registered");
    assert_eq!(check.status, CheckStatus::Pass);
    assert!(check.message.contains("0 agent workspace(s) writable"));
}

#[test]
fn workspace_writable_warn_no_config() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path().to_path_buf();

    let paths = Paths {
        rua_root: wd.clone(),
        secret_dir: wd.join(".secret"),
        cache_dir: wd.join(".cache"),
    };
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let check = results
        .iter()
        .find(|r| r.name == "agent_workspace_writable")
        .expect("agent_workspace_writable check must be registered");
    assert_eq!(check.status, CheckStatus::Warn);
}
