use serde::Serialize;

use crate::registry::{register_check, CheckContext, CheckResult, CheckStatus};

#[derive(Debug, Clone, Serialize)]
pub struct SecurityBaselineDetails {
    pub tool_guard_enabled: bool,
    pub file_guard_enabled: bool,
    pub skill_scanner_mode: String,
}

pub fn evaluate_security(
    tool_guard_enabled: bool,
    file_guard_enabled: bool,
    skill_scanner_mode: &str,
) -> (CheckStatus, String) {
    let mut disabled = Vec::new();
    if !tool_guard_enabled {
        disabled.push("tool_guard");
    }
    if !file_guard_enabled {
        disabled.push("file_guard");
    }
    if skill_scanner_mode == "off" || skill_scanner_mode.is_empty() {
        disabled.push("skill_scanner");
    }
    if disabled.is_empty() {
        (CheckStatus::Pass, "all security features enabled".into())
    } else {
        (
            CheckStatus::Warn,
            format!("disabled: {}", disabled.join(", ")),
        )
    }
}

fn check_security_baseline(ctx: &CheckContext) -> CheckResult {
    let wd = &ctx.paths.rua_root;
    let config_path = rua_config::get_config_path(wd);

    if !config_path.is_file() {
        return CheckResult {
            name: "security_baseline".into(),
            status: CheckStatus::Warn,
            message: "config not found — cannot check security".into(),
            details: None,
        };
    }

    let security = (|| -> std::io::Result<_> {
        let root = rua_config::load_config(&config_path, wd)?;
        let agent = rua_config::load_agent_config(&root.agents.active_agent, wd)?;
        let path = std::path::Path::new(&agent.workspace_dir).join("security.json");
        let sec: rua_config::SecurityConfig = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(error) => return Err(error),
        };
        Ok((
            sec,
            root.security.map(|s| s.skill_scanner).unwrap_or_default(),
        ))
    })();
    let (sec, scanner) = match security {
        Ok(config) => config,
        Err(error) => {
            return CheckResult {
                name: "security_baseline".into(),
                status: CheckStatus::Fail,
                message: error.to_string(),
                details: None,
            }
        }
    };

    let details = SecurityBaselineDetails {
        tool_guard_enabled: sec.tool_guard.enabled,
        file_guard_enabled: sec.file_guard.enabled,
        skill_scanner_mode: scanner.mode.clone(),
    };

    let (status, message) = evaluate_security(
        details.tool_guard_enabled,
        details.file_guard_enabled,
        &details.skill_scanner_mode,
    );

    CheckResult {
        name: "security_baseline".into(),
        status,
        message,
        details: Some(serde_json::to_value(&details).unwrap()),
    }
}

inventory::submit! { register_check("security_baseline", check_security_baseline) }
