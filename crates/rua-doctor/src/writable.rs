use std::path::Path;
use std::time::SystemTime;

use rua_config::{AgentProfileRef, RootConfig};

use crate::registry::{register_check, CheckContext, CheckResult, CheckStatus};

fn probe_writable(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("directory does not exist: {}", dir.display()));
    }
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let probe = dir.join(format!(".rua-writable-test-{nanos}"));
    std::fs::write(&probe, b"probe").map_err(|e| format!("write failed: {e}"))?;
    std::fs::remove_file(&probe).map_err(|e| format!("cleanup failed: {e}"))?;
    Ok(())
}

fn check_app_log_writable(ctx: &CheckContext) -> CheckResult {
    let log_dir = ctx.paths.rua_root.join("logs");
    match probe_writable(&log_dir) {
        Ok(()) => CheckResult {
            name: "app_log_writable".into(),
            status: CheckStatus::Pass,
            message: format!("log dir writable: {}", log_dir.display()),
            details: None,
        },
        Err(reason) => CheckResult {
            name: "app_log_writable".into(),
            status: CheckStatus::Fail,
            message: format!("log dir not writable: {reason}"),
            details: None,
        },
    }
}

fn check_agent_workspace_writable(ctx: &CheckContext) -> CheckResult {
    let wd = &ctx.paths.rua_root;
    let config_file =
        std::env::var("RUA_CONFIG_FILE").unwrap_or_else(|_| "config.json".to_string());
    let config_path = wd.join(config_file);

    let config: RootConfig = match std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|c| serde_json::from_str::<RootConfig>(&c).ok())
    {
        Some(c) => c,
        None => {
            return CheckResult {
                name: "agent_workspace_writable".into(),
                status: CheckStatus::Warn,
                message: "no config → skipped workspace writable check".into(),
                details: None,
            };
        }
    };

    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for (agent_id, profile_val) in &config.agents.profiles {
        let profile_ref: Option<AgentProfileRef> = serde_json::from_value(profile_val.clone()).ok();
        if let Some(pr) = profile_ref {
            if !pr.enabled {
                continue;
            }
            let ws = rua_config::dirs::shellexpand_path(&pr.workspace_dir);
            checked += 1;
            if let Err(reason) = probe_writable(&ws) {
                failures.push(format!("{agent_id}: {reason}"));
            }
        }
    }

    if failures.is_empty() {
        CheckResult {
            name: "agent_workspace_writable".into(),
            status: CheckStatus::Pass,
            message: format!("{checked} agent workspace(s) writable"),
            details: None,
        }
    } else {
        CheckResult {
            name: "agent_workspace_writable".into(),
            status: CheckStatus::Fail,
            message: failures.join("; "),
            details: None,
        }
    }
}

inventory::submit! { register_check("app_log_writable", check_app_log_writable) }
inventory::submit! { register_check("agent_workspace_writable", check_agent_workspace_writable) }
