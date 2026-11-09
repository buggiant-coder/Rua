use std::path::Path;

use serde::Serialize;

use crate::registry::{register_check, CheckContext, CheckResult, CheckStatus};

const BOOTSTRAP_WARN_BYTES: u64 = 350 * 1024;
const DIR_ENTRY_WARN: usize = 1000;

#[derive(Debug, Clone, Serialize)]
pub struct HygieneDetails {
    pub agents: Vec<AgentHygiene>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentHygiene {
    pub agent_id: String,
    pub bootstrap_bytes: u64,
    pub tool_result_entries: usize,
    pub dialog_entries: usize,
    pub warnings: Vec<String>,
}

pub fn count_dir_entries(path: &Path) -> usize {
    std::fs::read_dir(path).map(|rd| rd.count()).unwrap_or(0)
}

pub fn sum_prompt_file_sizes(workspace: &Path, files: &[String]) -> u64 {
    files
        .iter()
        .map(|f| {
            let p = workspace.join(f);
            std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0)
        })
        .sum()
}

fn check_workspace_hygiene(ctx: &CheckContext) -> CheckResult {
    let wd = &ctx.paths.rua_root;
    let config_path = rua_config::get_config_path(wd);

    if !config_path.is_file() {
        return CheckResult {
            name: "workspace_hygiene".into(),
            status: CheckStatus::Warn,
            message: "config not found — cannot check workspace hygiene".into(),
            details: None,
        };
    }

    let config = match rua_config::load_config(&config_path, wd) {
        Ok(config) => config,
        Err(error) => {
            return CheckResult {
                name: "workspace_hygiene".into(),
                status: CheckStatus::Fail,
                message: error.to_string(),
                details: None,
            }
        }
    };
    let mut agents = Vec::new();
    let mut total_warnings = 0;

    for (agent_id, profile_val) in &config.agents.profiles {
        let profile_ref: Option<rua_config::AgentProfileRef> =
            serde_json::from_value(profile_val.clone()).ok();
        let pr = match profile_ref {
            Some(pr) => pr,
            None => continue,
        };

        let ws = rua_config::dirs::shellexpand_path(&pr.workspace_dir);
        if !ws.is_dir() {
            continue;
        }

        let agent_json = ws.join("agent.json");
        let prompt_files: Vec<String> = if agent_json.is_file() {
            std::fs::read_to_string(&agent_json)
                .ok()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .and_then(|v| {
                    v.get("system_prompt_files")
                        .and_then(|a| a.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let bootstrap_bytes = sum_prompt_file_sizes(&ws, &prompt_files);
        let tool_result_entries = count_dir_entries(&ws.join("tool_result"));
        let dialog_entries = count_dir_entries(&ws.join("dialog"));

        let mut warnings = Vec::new();
        if bootstrap_bytes > BOOTSTRAP_WARN_BYTES {
            warnings.push(format!(
                "bootstrap prompt {} KiB > 350 KiB",
                bootstrap_bytes / 1024
            ));
        }
        if tool_result_entries > DIR_ENTRY_WARN {
            warnings.push(format!(
                "tool_result {tool_result_entries} entries > {DIR_ENTRY_WARN}"
            ));
        }
        if dialog_entries > DIR_ENTRY_WARN {
            warnings.push(format!(
                "dialog {dialog_entries} entries > {DIR_ENTRY_WARN}"
            ));
        }
        total_warnings += warnings.len();

        agents.push(AgentHygiene {
            agent_id: agent_id.clone(),
            bootstrap_bytes,
            tool_result_entries,
            dialog_entries,
            warnings,
        });
    }

    let status = if total_warnings > 0 {
        CheckStatus::Warn
    } else {
        CheckStatus::Pass
    };
    let message = if total_warnings > 0 {
        format!(
            "{total_warnings} hygiene warning(s) across {} agent(s)",
            agents.len()
        )
    } else {
        format!("{} agent workspace(s) clean", agents.len())
    };

    CheckResult {
        name: "workspace_hygiene".into(),
        status,
        message,
        details: Some(serde_json::to_value(&HygieneDetails { agents }).unwrap()),
    }
}

inventory::submit! { register_check("workspace_hygiene", check_workspace_hygiene) }
