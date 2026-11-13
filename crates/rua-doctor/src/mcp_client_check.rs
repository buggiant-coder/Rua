use std::path::Path;

use serde::Serialize;

use crate::registry::{register_check, CheckContext, CheckResult, CheckStatus};

#[derive(Debug, Clone, Serialize)]
pub struct McpClientDetails {
    pub clients: Vec<McpClientEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpClientEntry {
    pub name: String,
    pub transport: String,
    pub ok: bool,
    pub issue: Option<String>,
}

pub fn check_stdio_command(command: &str) -> Result<(), String> {
    if command.is_empty() {
        return Err("empty command".into());
    }
    let program = command.split_whitespace().next().unwrap_or("");
    if program.contains('/') || program.contains('\\') {
        if !Path::new(program).exists() {
            return Err(format!("command not found: {program}"));
        }
    } else {
        let found = command_exists_on_path(program);
        if !found {
            return Err(format!("command not in PATH: {program}"));
        }
    }
    Ok(())
}

fn command_exists_on_path(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        command_file_names(program)
            .into_iter()
            .any(|name| dir.join(name).is_file())
    })
}

fn command_file_names(program: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let mut names = vec![program.to_string()];
        if Path::new(program).extension().is_none() {
            let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
            for ext in pathext.split(';').filter(|ext| !ext.trim().is_empty()) {
                names.push(format!("{program}{ext}"));
            }
        }
        names
    }
    #[cfg(not(windows))]
    {
        vec![program.to_string()]
    }
}

pub fn check_streamable_http_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("empty URL".into());
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!("invalid URL scheme: {url}"));
    }
    if url
        .split("://")
        .nth(1)
        .is_none_or(|h| h.is_empty() || h.starts_with('/') || h.starts_with(':'))
    {
        return Err(format!("invalid URL host: {url}"));
    }
    Ok(())
}

fn check_mcp_client(ctx: &CheckContext) -> CheckResult {
    let wd = &ctx.paths.rua_root;
    let config_path = rua_config::get_config_path(wd);

    if !config_path.is_file() {
        return CheckResult {
            name: "mcp_client".into(),
            status: CheckStatus::Warn,
            message: "config not found — cannot check MCP clients".into(),
            details: None,
        };
    }

    let config = match rua_config::load_config(&config_path, wd) {
        Ok(config) => config,
        Err(error) => {
            return CheckResult {
                name: "mcp_client".into(),
                status: CheckStatus::Fail,
                message: error.to_string(),
                details: None,
            }
        }
    };
    let agent_id = &config.agents.active_agent;
    let agent = match rua_config::load_agent_config(agent_id, wd) {
        Ok(agent) => agent,
        Err(error) => {
            return CheckResult {
                name: "mcp_client".into(),
                status: CheckStatus::Fail,
                message: format!("Agent {agent_id}: {error}"),
                details: None,
            }
        }
    };
    let clients_val = agent.mcp.as_ref().and_then(|mcp| mcp.get("clients"));

    let clients_map = match clients_val.and_then(|v| v.as_object()) {
        Some(m) => m,
        None => {
            return CheckResult {
                name: "mcp_client".into(),
                status: CheckStatus::Pass,
                message: "no MCP clients configured".into(),
                details: Some(
                    serde_json::to_value(&McpClientDetails {
                        clients: Vec::new(),
                    })
                    .unwrap(),
                ),
            };
        }
    };

    let mut entries = Vec::new();
    let mut warn_count = 0;

    for (name, val) in clients_map {
        let transport = val
            .get("transport")
            .and_then(|v| v.as_str())
            .unwrap_or("stdio");

        let result = match transport {
            "stdio" => {
                let cmd = val.get("command").and_then(|v| v.as_str()).unwrap_or("");
                check_stdio_command(cmd)
            }
            "streamable_http" | "sse" => {
                let url = val.get("url").and_then(|v| v.as_str()).unwrap_or("");
                check_streamable_http_url(url)
            }
            _ => Ok(()),
        };

        let (ok, issue) = match result {
            Ok(()) => (true, None),
            Err(e) => {
                warn_count += 1;
                (false, Some(e))
            }
        };

        entries.push(McpClientEntry {
            name: name.clone(),
            transport: transport.into(),
            ok,
            issue,
        });
    }

    let status = if warn_count > 0 {
        CheckStatus::Warn
    } else {
        CheckStatus::Pass
    };
    let message = format!(
        "{}/{} MCP clients OK",
        entries.len() - warn_count,
        entries.len()
    );

    CheckResult {
        name: "mcp_client".into(),
        status,
        message,
        details: Some(serde_json::to_value(&McpClientDetails { clients: entries }).unwrap()),
    }
}

inventory::submit! { register_check("mcp_client", check_mcp_client) }
