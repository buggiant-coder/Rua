use serde::Serialize;

use crate::registry::{register_check, CheckContext, CheckResult, CheckStatus};

#[derive(Debug, Clone, Serialize)]
pub struct AgentJsonDetails {
    pub agents: Vec<AgentJsonDiag>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentJsonDiag {
    pub agent_id: String,
    pub path: String,
    pub deserialize_ok: bool,
    pub field_errors: Vec<String>,
    pub provider_warnings: Vec<String>,
}

pub fn validate_agent_json(
    agent_id: &str,
    agent_json_path: &std::path::Path,
    known_provider_ids: &[String],
) -> AgentJsonDiag {
    let path_str = agent_json_path.display().to_string();

    let raw = match std::fs::read_to_string(agent_json_path) {
        Ok(c) => c,
        Err(e) => {
            return AgentJsonDiag {
                agent_id: agent_id.to_string(),
                path: path_str,
                deserialize_ok: false,
                field_errors: vec![format!("read error: {e}")],
                provider_warnings: vec![],
            };
        }
    };

    let config: rua_config::AgentProfileConfig = match serde_json::from_str(&raw) {
        Ok(c) => c,
        Err(e) => {
            return AgentJsonDiag {
                agent_id: agent_id.to_string(),
                path: path_str,
                deserialize_ok: false,
                field_errors: vec![format!("deserialize error: {e}")],
                provider_warnings: vec![],
            };
        }
    };

    let mut field_errors = Vec::new();

    if config.name.is_empty() {
        field_errors.push("field 'name' is empty".to_string());
    }
    if config.id.is_empty() {
        field_errors.push("field 'id' is empty".to_string());
    }

    let mut provider_warnings = Vec::new();

    if let Some(ref slot) = config.default_model {
        if !slot.provider_id.is_empty()
            && !known_provider_ids.is_empty()
            && !known_provider_ids.contains(&slot.provider_id)
        {
            provider_warnings.push(format!(
                "default_model references unknown provider '{}'",
                slot.provider_id
            ));
        }
    }

    if let Some(routing) = config.extra.get("llm_routing") {
        if let Some(local) = routing.get("local") {
            if let Some(pid) = local.get("provider_id").and_then(|v| v.as_str()) {
                if !pid.is_empty()
                    && !known_provider_ids.is_empty()
                    && !known_provider_ids.contains(&pid.to_string())
                {
                    provider_warnings.push(format!(
                        "llm_routing.local references unknown provider '{pid}'"
                    ));
                }
            }
        }
        if let Some(cloud) = routing.get("cloud") {
            if let Some(pid) = cloud.get("provider_id").and_then(|v| v.as_str()) {
                if !pid.is_empty()
                    && !known_provider_ids.is_empty()
                    && !known_provider_ids.contains(&pid.to_string())
                {
                    provider_warnings.push(format!(
                        "llm_routing.cloud references unknown provider '{pid}'"
                    ));
                }
            }
        }
    }

    AgentJsonDiag {
        agent_id: agent_id.to_string(),
        path: path_str,
        deserialize_ok: true,
        field_errors,
        provider_warnings,
    }
}

fn check_agent_json_schema(ctx: &CheckContext) -> CheckResult {
    let wd = &ctx.paths.rua_root;
    let config_path = rua_config::get_config_path(wd);

    if !config_path.is_file() {
        return CheckResult {
            name: "agent_json_schema".into(),
            status: CheckStatus::Warn,
            message: "config not found — cannot check agent.json schemas".into(),
            details: None,
        };
    }

    let config = match rua_config::load_config(&config_path, wd) {
        Ok(config) => config,
        Err(error) => {
            return CheckResult {
                name: "agent_json_schema".into(),
                status: CheckStatus::Fail,
                message: error.to_string(),
                details: None,
            }
        }
    };

    let known_providers = collect_known_provider_ids(&ctx.paths.secret_dir);

    let mut agents = Vec::new();
    let mut total_errors = 0;
    let mut total_warnings = 0;

    for (agent_id, profile_val) in &config.agents.profiles {
        let profile_ref: Option<rua_config::AgentProfileRef> =
            serde_json::from_value(profile_val.clone()).ok();
        let pr = match profile_ref {
            Some(pr) => pr,
            None => continue,
        };

        let ws = rua_config::dirs::shellexpand_path(&pr.workspace_dir);
        let agent_json = ws.join("agent.json");

        if !agent_json.is_file() {
            continue;
        }

        let diag = validate_agent_json(agent_id, &agent_json, &known_providers);
        total_errors += diag.field_errors.len();
        total_warnings += diag.provider_warnings.len();
        agents.push(diag);
    }

    let status = if total_errors > 0 {
        CheckStatus::Fail
    } else if total_warnings > 0 {
        CheckStatus::Warn
    } else {
        CheckStatus::Pass
    };

    let message = if total_errors > 0 {
        format!(
            "{total_errors} schema error(s) across {} agent.json file(s)",
            agents.len()
        )
    } else if total_warnings > 0 {
        format!(
            "{total_warnings} provider warning(s) across {} agent.json file(s)",
            agents.len()
        )
    } else {
        format!("{} agent.json file(s) valid", agents.len())
    };

    CheckResult {
        name: "agent_json_schema".into(),
        status,
        message,
        details: Some(serde_json::to_value(&AgentJsonDetails { agents }).unwrap()),
    }
}

fn collect_known_provider_ids(secret_dir: &std::path::Path) -> Vec<String> {
    let providers_dir = crate::find_providers_dir(secret_dir);
    let providers_dir = match providers_dir {
        Some(d) => d,
        None => return vec![],
    };
    crate::load_providers(&providers_dir)
        .into_iter()
        .map(|p| p.id)
        .collect()
}

inventory::submit! { register_check("agent_json_schema", check_agent_json_schema) }
