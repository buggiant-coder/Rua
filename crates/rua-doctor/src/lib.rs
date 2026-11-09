#![forbid(unsafe_code)]

pub mod agent_json;
pub mod browser;
pub mod connectivity;
pub mod cron_jobs;
pub mod disk;
pub mod fix;
mod http_proxy;
pub mod local_model;
pub mod mcp_client_check;
pub mod provider_http;
pub mod registry;
pub mod security_baseline;
pub mod windows_shell;
pub mod workspace_hygiene;
pub mod writable;

use std::path::{Path, PathBuf};

use rua_config::dirs::Paths;
use rua_config::{AgentProfileRef, RootConfig};
use rua_core::runtime_authority::{RuntimeAuthority, RuntimeInventory};
use rua_provider::{DefaultModel, ProviderConfig};
use serde::{Deserialize, Serialize};

use crate::registry::{register_check, CheckContext, CheckResult, CheckStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub checks: Vec<CheckResult>,
}

impl DoctorReport {
    pub fn version(&self) -> VersionInfo {
        extract_typed(&self.checks, "version").unwrap_or_default()
    }

    pub fn runtime(&self) -> RuntimeInfo {
        extract_typed(&self.checks, "runtime").unwrap_or_default()
    }

    pub fn config(&self) -> ConfigDiagnostic {
        extract_typed(&self.checks, "config").unwrap_or_default()
    }

    pub fn providers(&self) -> ProvidersDiagnostic {
        extract_typed(&self.checks, "providers").unwrap_or_default()
    }

    pub fn workspace(&self) -> WorkspaceDiagnostic {
        extract_typed(&self.checks, "workspace").unwrap_or_default()
    }

    pub fn cron_jobs(&self) -> cron_jobs::CronJobsDiagnostic {
        extract_typed(&self.checks, "cron_jobs").unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VersionInfo {
    pub rua_version: String,
    pub rust_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeInfo {
    pub platform: String,
    pub arch: String,
    pub working_dir: String,
    pub working_dir_exists: bool,
    pub working_dir_writable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_authority: Option<RuntimeInventory>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigDiagnostic {
    pub config_path: String,
    pub config_exists: bool,
    pub config_parseable: bool,
    pub parse_error: Option<String>,
    pub agent_count: usize,
    pub active_agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvidersDiagnostic {
    pub secret_dir: String,
    pub secret_dir_exists: bool,
    pub providers_dir_exists: bool,
    pub providers: Vec<ProviderDiagEntry>,
    pub default_model: Option<DefaultModelDiag>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDiagEntry {
    pub id: String,
    pub name: String,
    pub has_api_key: bool,
    pub has_base_url: bool,
    pub model_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultModelDiag {
    pub provider_id: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceDiagnostic {
    pub agents: Vec<AgentWorkspaceDiag>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentWorkspaceDiag {
    pub agent_id: String,
    pub workspace_dir: String,
    pub enabled: bool,
    pub workspace_exists: bool,
    pub agent_json_exists: bool,
}

pub(crate) fn is_dir_writable(path: &Path) -> bool {
    use std::fs;
    if !path.is_dir() {
        return false;
    }
    let probe = path.join(".rua_doctor_probe");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn find_providers_dir(sd: &Path) -> Option<PathBuf> {
    let nested = sd.join("providers").join("providers");
    if nested.is_dir() {
        return Some(nested);
    }
    let flat = sd.join("providers");
    if flat.is_dir() {
        return Some(flat);
    }
    None
}

pub fn find_default_model_path(sd: &Path) -> PathBuf {
    let nested = sd.join("providers").join("default_model.json");
    if nested.is_file() {
        return nested;
    }
    sd.join("default_model.json")
}

pub fn load_providers(providers_dir: &Path) -> Vec<ProviderDiagEntry> {
    let mut entries = Vec::new();
    let read_dir = match std::fs::read_dir(providers_dir) {
        Ok(rd) => rd,
        Err(_) => return entries,
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let provider: ProviderConfig = match serde_json::from_str(&content) {
            Ok(p) => p,
            Err(_) => continue,
        };
        entries.push(ProviderDiagEntry {
            id: provider.id,
            name: provider.name,
            has_api_key: !provider.api_key.is_empty(),
            has_base_url: !provider.base_url.is_empty(),
            model_count: provider.models.len(),
        });
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    entries
}

pub fn load_default_model(path: &Path) -> Option<DefaultModelDiag> {
    let content = std::fs::read_to_string(path).ok()?;
    let am: DefaultModel = serde_json::from_str(&content).ok()?;
    if am.provider_id.is_empty() && am.model.is_empty() {
        return None;
    }
    Some(DefaultModelDiag {
        provider_id: am.provider_id,
        model: am.model,
    })
}

fn check_version(_ctx: &CheckContext) -> CheckResult {
    let info = VersionInfo {
        rua_version: env!("CARGO_PKG_VERSION").to_string(),
        rust_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    CheckResult {
        name: "version".into(),
        status: CheckStatus::Pass,
        message: format!("rua v{}", info.rua_version),
        details: Some(serde_json::to_value(&info).unwrap()),
    }
}

fn check_runtime(ctx: &CheckContext) -> CheckResult {
    let wd = &ctx.paths.rua_root;
    let runtime_authority = RuntimeAuthority::from_current_process(None, wd, &ctx.paths.cache_dir);
    let info = RuntimeInfo {
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        working_dir: wd.display().to_string(),
        working_dir_exists: wd.is_dir(),
        working_dir_writable: is_dir_writable(wd),
        runtime_authority: Some(runtime_authority.inventory().clone()),
    };
    let status = if info.working_dir_exists {
        CheckStatus::Pass
    } else {
        CheckStatus::Warn
    };
    CheckResult {
        name: "runtime".into(),
        status,
        message: format!("{}/{}", info.platform, info.arch),
        details: Some(serde_json::to_value(&info).unwrap()),
    }
}

fn check_config(ctx: &CheckContext) -> CheckResult {
    let wd = &ctx.paths.rua_root;
    let config_file =
        std::env::var("RUA_CONFIG_FILE").unwrap_or_else(|_| "config.json".to_string());
    let config_path = wd.join(config_file);

    if !config_path.is_file() {
        let diag = ConfigDiagnostic {
            config_path: config_path.display().to_string(),
            config_exists: false,
            config_parseable: false,
            parse_error: Some("config.json not found".to_string()),
            agent_count: 0,
            active_agent: String::new(),
        };
        return CheckResult {
            name: "config".into(),
            status: CheckStatus::Fail,
            message: "config.json not found".into(),
            details: Some(serde_json::to_value(&diag).unwrap()),
        };
    }

    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) => {
            let diag = ConfigDiagnostic {
                config_path: config_path.display().to_string(),
                config_exists: true,
                config_parseable: false,
                parse_error: Some(format!("read error: {e}")),
                agent_count: 0,
                active_agent: String::new(),
            };
            return CheckResult {
                name: "config".into(),
                status: CheckStatus::Fail,
                message: format!("read error: {e}"),
                details: Some(serde_json::to_value(&diag).unwrap()),
            };
        }
    };

    let config: RootConfig = match serde_json::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            let diag = ConfigDiagnostic {
                config_path: config_path.display().to_string(),
                config_exists: true,
                config_parseable: false,
                parse_error: Some(format!("parse error: {e}")),
                agent_count: 0,
                active_agent: String::new(),
            };
            return CheckResult {
                name: "config".into(),
                status: CheckStatus::Fail,
                message: format!("parse error: {e}"),
                details: Some(serde_json::to_value(&diag).unwrap()),
            };
        }
    };

    let diag = ConfigDiagnostic {
        config_path: config_path.display().to_string(),
        config_exists: true,
        config_parseable: true,
        parse_error: None,
        agent_count: config.agents.profiles.len(),
        active_agent: config.agents.active_agent,
    };
    CheckResult {
        name: "config".into(),
        status: CheckStatus::Pass,
        message: format!("{} agents configured", diag.agent_count),
        details: Some(serde_json::to_value(&diag).unwrap()),
    }
}

fn check_providers(ctx: &CheckContext) -> CheckResult {
    let sd = &ctx.paths.secret_dir;
    let providers_dir = find_providers_dir(sd);
    let providers_list = providers_dir
        .as_ref()
        .map(|d| load_providers(d))
        .unwrap_or_default();
    let default_model_path = find_default_model_path(sd);
    let default_model = if default_model_path.is_file() {
        load_default_model(&default_model_path)
    } else {
        None
    };

    let diag = ProvidersDiagnostic {
        secret_dir: sd.display().to_string(),
        secret_dir_exists: sd.is_dir(),
        providers_dir_exists: providers_dir.is_some(),
        providers: providers_list,
        default_model,
    };
    let status = if diag.secret_dir_exists {
        CheckStatus::Pass
    } else {
        CheckStatus::Warn
    };
    CheckResult {
        name: "providers".into(),
        status,
        message: format!("{} providers found", diag.providers.len()),
        details: Some(serde_json::to_value(&diag).unwrap()),
    }
}

fn check_workspace(ctx: &CheckContext) -> CheckResult {
    let wd = &ctx.paths.rua_root;
    let config_file =
        std::env::var("RUA_CONFIG_FILE").unwrap_or_else(|_| "config.json".to_string());
    let config_path = wd.join(config_file);

    if !config_path.is_file() {
        let diag = WorkspaceDiagnostic { agents: Vec::new() };
        return CheckResult {
            name: "workspace".into(),
            status: CheckStatus::Warn,
            message: "no config → no workspace info".into(),
            details: Some(serde_json::to_value(&diag).unwrap()),
        };
    }

    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => {
            return CheckResult {
                name: "workspace".into(),
                status: CheckStatus::Warn,
                message: "cannot read config for workspace info".into(),
                details: Some(
                    serde_json::to_value(&WorkspaceDiagnostic { agents: Vec::new() }).unwrap(),
                ),
            };
        }
    };

    let config: RootConfig = match serde_json::from_str(&content) {
        Ok(c) => c,
        Err(_) => {
            return CheckResult {
                name: "workspace".into(),
                status: CheckStatus::Warn,
                message: "cannot parse config for workspace info".into(),
                details: Some(
                    serde_json::to_value(&WorkspaceDiagnostic { agents: Vec::new() }).unwrap(),
                ),
            };
        }
    };

    let mut agents = Vec::new();
    for (agent_id, profile_val) in &config.agents.profiles {
        let profile_ref: Option<AgentProfileRef> = serde_json::from_value(profile_val.clone()).ok();
        if let Some(pr) = profile_ref {
            let ws = rua_config::dirs::shellexpand_path(&pr.workspace_dir);
            let agent_json = ws.join("agent.json");
            agents.push(AgentWorkspaceDiag {
                agent_id: agent_id.clone(),
                workspace_dir: pr.workspace_dir.clone(),
                enabled: pr.enabled,
                workspace_exists: ws.is_dir(),
                agent_json_exists: agent_json.is_file(),
            });
        }
    }

    let diag = WorkspaceDiagnostic { agents };
    CheckResult {
        name: "workspace".into(),
        status: CheckStatus::Pass,
        message: format!("{} agent workspaces", diag.agents.len()),
        details: Some(serde_json::to_value(&diag).unwrap()),
    }
}

inventory::submit! { register_check("version", check_version) }
inventory::submit! { register_check("runtime", check_runtime) }
inventory::submit! { register_check("config", check_config) }
inventory::submit! { register_check("providers", check_providers) }
inventory::submit! { register_check("workspace", check_workspace) }

pub fn extract_typed<T: serde::de::DeserializeOwned>(
    checks: &[CheckResult],
    name: &str,
) -> Option<T> {
    checks
        .iter()
        .find(|c| c.name == name)
        .and_then(|c| c.details.as_ref())
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub fn run_diagnostics(paths: &Paths) -> DoctorReport {
    let ctx = CheckContext { paths };
    let checks = registry::run_all_checks(&ctx);
    DoctorReport { checks }
}

pub fn run_diagnostics_filtered(paths: &Paths, deep: bool) -> DoctorReport {
    let ctx = CheckContext { paths };
    let (checks, _skipped) = registry::run_all_checks_filtered(&ctx, deep);
    DoctorReport { checks }
}
