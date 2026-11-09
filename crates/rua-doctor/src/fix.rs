use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use rua_config::dirs;
use rua_config::types::{AgentProfileRef, JobsFile, SkillEntry, SkillManifestFile};

pub const SAFE_FIX_IDS: &[&str] = &[
    "ensure-working-dir",
    "ensure-workspace-dirs",
    "validate-all-jobs-json",
];

pub const SYNC_FIX_IDS: &[&str] = &["reconcile-workspace-skills"];

pub const NONINTERACTIVE_FIX_IDS: &[&str] = &[
    "ensure-working-dir",
    "ensure-workspace-dirs",
    "validate-all-jobs-json",
    "reconcile-workspace-skills",
];

pub const INTERACTIVE_FIX_IDS: &[&str] = &[
    "seed-missing-agent-json",
    "reset-invalid-agent-json",
    "write-empty-jobs-json",
    "normalize-jobs-cron",
    "rebuild-console-npm",
    "install-git-bash",
];

#[derive(Debug, Clone, Serialize)]
pub struct FixResult {
    pub fix_id: String,
    pub success: bool,
    pub message: String,
    pub actions: Vec<FixAction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FixAction {
    pub description: String,
    pub applied: bool,
    pub error: Option<String>,
}

pub fn is_safe_fix(fix_id: &str) -> bool {
    SAFE_FIX_IDS.contains(&fix_id)
}

pub fn is_sync_fix(fix_id: &str) -> bool {
    SYNC_FIX_IDS.contains(&fix_id)
}

pub fn is_noninteractive_fix(fix_id: &str) -> bool {
    NONINTERACTIVE_FIX_IDS.contains(&fix_id)
}

pub fn is_interactive_fix(fix_id: &str) -> bool {
    INTERACTIVE_FIX_IDS.contains(&fix_id)
}

pub fn is_known_fix(fix_id: &str) -> bool {
    is_safe_fix(fix_id) || is_sync_fix(fix_id) || is_interactive_fix(fix_id)
}

pub fn run_fix(fix_id: &str, wd: &Path) -> FixResult {
    if !is_known_fix(fix_id) {
        return FixResult {
            fix_id: fix_id.to_string(),
            success: false,
            message: format!(
                "unknown fix_id: {fix_id:?}; known: {:?}",
                SAFE_FIX_IDS
                    .iter()
                    .chain(INTERACTIVE_FIX_IDS.iter())
                    .collect::<Vec<_>>()
            ),
            actions: vec![],
        };
    }

    match fix_id {
        "ensure-working-dir" => fix_ensure_working_dir(wd),
        "ensure-workspace-dirs" => fix_ensure_workspace_dirs(wd),
        "validate-all-jobs-json" => fix_validate_all_jobs_json(wd),
        "reconcile-workspace-skills" => fix_reconcile_workspace_skills(wd),
        "install-git-bash" => fix_install_git_bash(),
        _ => FixResult {
            fix_id: fix_id.to_string(),
            success: false,
            message: format!("{fix_id} is not yet implemented in Rust"),
            actions: vec![],
        },
    }
}

fn fix_install_git_bash() -> FixResult {
    let fix_id = "install-git-bash".to_string();
    if !cfg!(windows) {
        return FixResult {
            fix_id,
            success: false,
            message: "install-git-bash is only available on Windows".into(),
            actions: vec![],
        };
    }

    let command = git_bash_winget_command();
    let action = run_winget_action(&command);
    if action.applied {
        FixResult {
            fix_id,
            success: true,
            message: "Git for Windows installed via winget; restart Rua or your shell".into(),
            actions: vec![action],
        }
    } else {
        FixResult {
            fix_id,
            success: false,
            message: action
                .error
                .as_ref()
                .map(|error| format!("failed to install Git for Windows via winget: {error}"))
                .unwrap_or_else(|| "failed to install Git for Windows via winget".into()),
            actions: vec![action],
        }
    }
}

fn run_winget_action(command: &[String]) -> FixAction {
    match std::process::Command::new(&command[0])
        .args(&command[1..])
        .status()
    {
        Ok(status) if status.success() => FixAction {
            description: command.join(" "),
            applied: true,
            error: None,
        },
        Ok(status) => FixAction {
            description: command.join(" "),
            applied: false,
            error: Some(format!("exit status {status}")),
        },
        Err(error) => FixAction {
            description: command.join(" "),
            applied: false,
            error: Some(error.to_string()),
        },
    }
}

pub fn git_bash_winget_command() -> Vec<String> {
    vec![
        "winget".into(),
        "install".into(),
        "--id".into(),
        "Git.Git".into(),
        "-e".into(),
        "--source".into(),
        "winget".into(),
        "--silent".into(),
        "--accept-package-agreements".into(),
        "--accept-source-agreements".into(),
    ]
}

fn fix_ensure_working_dir(wd: &Path) -> FixResult {
    let fix_id = "ensure-working-dir".to_string();

    if wd.is_dir() {
        return FixResult {
            fix_id,
            success: true,
            message: "working directory already exists".to_string(),
            actions: vec![],
        };
    }

    let parent = match wd.parent() {
        Some(p) if p.is_dir() && crate::is_dir_writable(p) => p,
        _ => {
            return FixResult {
                fix_id,
                success: false,
                message: format!(
                    "cannot create {} (parent missing or not writable)",
                    wd.display()
                ),
                actions: vec![],
            };
        }
    };
    let _ = parent;

    match std::fs::create_dir_all(wd) {
        Ok(()) => FixResult {
            fix_id,
            success: true,
            message: format!("created {}", wd.display()),
            actions: vec![FixAction {
                description: format!("create working directory {}", wd.display()),
                applied: true,
                error: None,
            }],
        },
        Err(e) => FixResult {
            fix_id,
            success: false,
            message: format!("failed to create {}: {e}", wd.display()),
            actions: vec![FixAction {
                description: format!("create working directory {}", wd.display()),
                applied: false,
                error: Some(e.to_string()),
            }],
        },
    }
}

fn fix_ensure_workspace_dirs(wd: &Path) -> FixResult {
    let fix_id = "ensure-workspace-dirs".to_string();
    let config_path = rua_config::get_config_path(wd);

    if !config_path.is_file() {
        return FixResult {
            fix_id,
            success: true,
            message: "no config file found; nothing to do".to_string(),
            actions: vec![],
        };
    }

    let config = match rua_config::load_config(&config_path, wd) {
        Ok(config) => config,
        Err(error) => {
            return FixResult {
                fix_id,
                success: false,
                message: error.to_string(),
                actions: vec![],
            }
        }
    };
    let mut actions = Vec::new();
    let mut any_error = false;

    for (agent_id, profile_val) in &config.agents.profiles {
        let profile_ref: Option<AgentProfileRef> = serde_json::from_value(profile_val.clone()).ok();
        let Some(pr) = profile_ref else { continue };

        let ws = dirs::shellexpand_path(&pr.workspace_dir);
        if ws.is_dir() {
            continue;
        }
        if !ws.starts_with(wd) {
            continue;
        }

        match std::fs::create_dir_all(&ws) {
            Ok(()) => {
                actions.push(FixAction {
                    description: format!(
                        "created workspace directory for agent {agent_id}: {}",
                        ws.display()
                    ),
                    applied: true,
                    error: None,
                });
            }
            Err(e) => {
                any_error = true;
                actions.push(FixAction {
                    description: format!(
                        "create workspace directory for agent {agent_id}: {}",
                        ws.display()
                    ),
                    applied: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    if actions.is_empty() {
        return FixResult {
            fix_id,
            success: true,
            message: "all workspace directories already exist".to_string(),
            actions,
        };
    }

    let applied = actions.iter().filter(|a| a.applied).count();
    FixResult {
        fix_id,
        success: !any_error,
        message: if any_error {
            format!(
                "{applied}/{} workspace directories created (some failed)",
                actions.len()
            )
        } else {
            format!("{applied} workspace directory(ies) created")
        },
        actions,
    }
}

fn fix_validate_all_jobs_json(wd: &Path) -> FixResult {
    let fix_id = "validate-all-jobs-json".to_string();
    let config_path = rua_config::get_config_path(wd);

    if !config_path.is_file() {
        return FixResult {
            fix_id,
            success: true,
            message: "no config file found; nothing to validate".to_string(),
            actions: vec![],
        };
    }

    let config = match rua_config::load_config(&config_path, wd) {
        Ok(config) => config,
        Err(error) => {
            return FixResult {
                fix_id,
                success: false,
                message: error.to_string(),
                actions: vec![],
            }
        }
    };
    let mut actions = Vec::new();
    let mut any_error = false;

    for (agent_id, profile_val) in &config.agents.profiles {
        let profile_ref: Option<AgentProfileRef> = serde_json::from_value(profile_val.clone()).ok();
        let Some(pr) = profile_ref else { continue };

        let ws = dirs::shellexpand_path(&pr.workspace_dir);
        let jobs_path = ws.join("jobs.json");
        if !jobs_path.is_file() {
            continue;
        }

        match std::fs::read_to_string(&jobs_path) {
            Ok(content) => match serde_json::from_str::<JobsFile>(&content) {
                Ok(jf) => {
                    let count = jf.jobs.len();
                    let enabled = jf.jobs.values().filter(|j| j.enabled).count();
                    actions.push(FixAction {
                        description: format!(
                            "{agent_id}: {} OK ({count} job(s), {enabled} enabled)",
                            jobs_path.display()
                        ),
                        applied: true,
                        error: None,
                    });
                }
                Err(e) => {
                    any_error = true;
                    actions.push(FixAction {
                        description: format!("{agent_id}: {} parse error", jobs_path.display()),
                        applied: false,
                        error: Some(e.to_string()),
                    });
                }
            },
            Err(e) => {
                any_error = true;
                actions.push(FixAction {
                    description: format!("{agent_id}: {} read error", jobs_path.display()),
                    applied: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    let root_jobs = rua_config::get_jobs_path(wd);
    if root_jobs.is_file() {
        match std::fs::read_to_string(&root_jobs) {
            Ok(content) => match serde_json::from_str::<JobsFile>(&content) {
                Ok(jf) => {
                    let count = jf.jobs.len();
                    actions.push(FixAction {
                        description: format!("(root): {} OK ({count} job(s))", root_jobs.display()),
                        applied: true,
                        error: None,
                    });
                }
                Err(e) => {
                    any_error = true;
                    actions.push(FixAction {
                        description: format!("(root): {} parse error", root_jobs.display()),
                        applied: false,
                        error: Some(e.to_string()),
                    });
                }
            },
            Err(e) => {
                any_error = true;
                actions.push(FixAction {
                    description: format!("(root): {} read error", root_jobs.display()),
                    applied: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    if actions.is_empty() {
        return FixResult {
            fix_id,
            success: true,
            message: "no jobs.json files found to validate".to_string(),
            actions,
        };
    }

    let validated = actions.iter().filter(|a| a.applied).count();
    let failed = actions.iter().filter(|a| !a.applied).count();
    FixResult {
        fix_id,
        success: !any_error,
        message: if any_error {
            format!("{validated} file(s) OK, {failed} failed")
        } else {
            format!("{validated} file(s) OK")
        },
        actions,
    }
}

fn fix_reconcile_workspace_skills(wd: &Path) -> FixResult {
    let fix_id = "reconcile-workspace-skills".to_string();
    let config_path = rua_config::get_config_path(wd);

    if !config_path.is_file() {
        return FixResult {
            fix_id,
            success: true,
            message: "no config file found; nothing to reconcile".to_string(),
            actions: vec![],
        };
    }

    let config = match rua_config::load_config(&config_path, wd) {
        Ok(config) => config,
        Err(error) => {
            return FixResult {
                fix_id,
                success: false,
                message: error.to_string(),
                actions: vec![],
            }
        }
    };
    let mut actions = Vec::new();
    let mut any_error = false;

    for (agent_id, profile_val) in &config.agents.profiles {
        let profile_ref: Option<AgentProfileRef> = serde_json::from_value(profile_val.clone()).ok();
        let Some(pr) = profile_ref else { continue };

        let ws = dirs::shellexpand_path(&pr.workspace_dir);
        if !ws.is_dir() {
            continue;
        }

        let ws_resolved = ws.canonicalize().unwrap_or_else(|_| ws.clone());
        let wd_resolved = wd.canonicalize().unwrap_or_else(|_| wd.to_path_buf());
        if !ws_resolved.starts_with(&wd_resolved) {
            continue;
        }

        match reconcile_single_workspace(&ws) {
            Ok(summary) => {
                actions.push(FixAction {
                    description: format!("{agent_id}: reconciled skill.json ({summary})"),
                    applied: true,
                    error: None,
                });
            }
            Err(e) => {
                any_error = true;
                actions.push(FixAction {
                    description: format!("{agent_id}: reconcile failed"),
                    applied: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    if actions.is_empty() {
        return FixResult {
            fix_id,
            success: true,
            message: "no workspace directories found to reconcile".to_string(),
            actions,
        };
    }

    let applied = actions.iter().filter(|a| a.applied).count();
    FixResult {
        fix_id,
        success: !any_error,
        message: if any_error {
            format!(
                "{applied}/{} workspace(s) reconciled (some failed)",
                actions.len()
            )
        } else {
            format!("{applied} workspace(s) reconciled")
        },
        actions,
    }
}

fn reconcile_single_workspace(ws: &Path) -> Result<String, std::io::Error> {
    let skills_dir = ws.join("skills");
    let manifest_path = ws.join("skill.json");

    if !skills_dir.is_dir() {
        std::fs::create_dir_all(&skills_dir)?;
    }

    let mut manifest: SkillManifestFile = if manifest_path.is_file() {
        let content = std::fs::read_to_string(&manifest_path)?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        SkillManifestFile::default()
    };

    let mut discovered: HashMap<String, ()> = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(&skills_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("SKILL.md").is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    discovered.insert(name.to_string(), ());
                }
            }
        }
    }

    let mut added = 0usize;
    for name in discovered.keys() {
        if !manifest.skills.contains_key(name) {
            manifest.skills.insert(
                name.clone(),
                SkillEntry {
                    name: name.clone(),
                    source: "customized".to_string(),
                    channels: vec!["all".to_string()],
                    ..Default::default()
                },
            );
            added += 1;
        }
    }

    let mut removed = 0usize;
    let stale: Vec<String> = manifest
        .skills
        .keys()
        .filter(|k| !discovered.contains_key(k.as_str()))
        .cloned()
        .collect();
    for name in &stale {
        manifest.skills.remove(name);
        removed += 1;
    }

    let json = serde_json::to_string_pretty(&manifest).map_err(std::io::Error::other)?;
    std::fs::write(&manifest_path, json)?;

    Ok(format!(
        "{} skill(s), +{added} added, -{removed} removed",
        manifest.skills.len()
    ))
}
