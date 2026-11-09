use std::path::Path;

use serde::Serialize;

use rua_config::types::cron::JobsFile;

use crate::registry::{register_check, CheckContext, CheckResult, CheckStatus};

#[derive(Debug, Clone, Serialize, serde::Deserialize, Default)]
pub struct CronJobsDiagnostic {
    pub files: Vec<CronJobsFileDiag>,
    pub total_jobs: usize,
    pub total_errors: usize,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct CronJobsFileDiag {
    pub label: String,
    pub path: String,
    pub exists: bool,
    pub parseable: bool,
    pub parse_error: Option<String>,
    pub jobs: Vec<CronJobDiag>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct CronJobDiag {
    pub id: String,
    pub errors: Vec<String>,
}

pub fn validate_jobs_file(label: &str, path: &Path) -> CronJobsFileDiag {
    let path_str = path.display().to_string();

    if !path.is_file() {
        return CronJobsFileDiag {
            label: label.to_string(),
            path: path_str,
            exists: false,
            parseable: false,
            parse_error: None,
            jobs: vec![],
        };
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return CronJobsFileDiag {
                label: label.to_string(),
                path: path_str,
                exists: true,
                parseable: false,
                parse_error: Some(format!("read error: {e}")),
                jobs: vec![],
            };
        }
    };

    let jobs_file: JobsFile = match serde_json::from_str(&content) {
        Ok(jf) => jf,
        Err(e) => {
            return CronJobsFileDiag {
                label: label.to_string(),
                path: path_str,
                exists: true,
                parseable: false,
                parse_error: Some(format!("parse error: {e}")),
                jobs: vec![],
            };
        }
    };

    let mut jobs = Vec::new();
    for spec in jobs_file.jobs.values() {
        let mut errors = Vec::new();

        if spec.id.is_empty() {
            errors.push("missing required field 'id'".to_string());
        }
        if spec.name.is_empty() {
            errors.push("missing required field 'name'".to_string());
        }
        if spec.schedule.cron.is_empty() {
            errors.push("missing required field 'schedule.cron'".to_string());
        }

        let tz = &spec.schedule.timezone;
        if tz.is_empty() {
            errors.push("missing required field 'schedule.timezone'".to_string());
        } else if tz.parse::<chrono_tz::Tz>().is_err() {
            errors.push(format!("invalid timezone '{tz}'"));
        }

        jobs.push(CronJobDiag {
            id: if spec.id.is_empty() {
                "(empty)".to_string()
            } else {
                spec.id.clone()
            },
            errors,
        });
    }

    CronJobsFileDiag {
        label: label.to_string(),
        path: path_str,
        exists: true,
        parseable: true,
        parse_error: None,
        jobs,
    }
}

fn check_cron_jobs(ctx: &CheckContext) -> CheckResult {
    let wd = &ctx.paths.rua_root;
    let mut files = Vec::new();

    let root_jobs_path = rua_config::get_jobs_path(wd);
    let root_diag = validate_jobs_file("root", &root_jobs_path);
    if root_diag.exists {
        files.push(root_diag);
    }

    let config_path = rua_config::get_config_path(wd);
    if config_path.is_file() {
        let config = match rua_config::load_config(&config_path, wd) {
            Ok(config) => config,
            Err(error) => {
                return CheckResult {
                    name: "cron_jobs".into(),
                    status: CheckStatus::Fail,
                    message: error.to_string(),
                    details: None,
                }
            }
        };
        for (agent_id, profile_val) in &config.agents.profiles {
            let profile_ref: Option<rua_config::AgentProfileRef> =
                serde_json::from_value(profile_val.clone()).ok();
            if let Some(pr) = profile_ref {
                let ws = rua_config::dirs::shellexpand_path(&pr.workspace_dir);
                let ws_jobs = ws.join("jobs.json");
                if ws_jobs.is_file() {
                    files.push(validate_jobs_file(agent_id, &ws_jobs));
                }
            }
        }
    }

    let total_jobs: usize = files.iter().map(|f| f.jobs.len()).sum();
    let total_errors: usize = files
        .iter()
        .flat_map(|f| &f.jobs)
        .map(|j| j.errors.len())
        .sum::<usize>()
        + files.iter().filter(|f| !f.parseable).count();

    let diag = CronJobsDiagnostic {
        files,
        total_jobs,
        total_errors,
    };

    let status = if total_errors > 0 {
        CheckStatus::Fail
    } else {
        CheckStatus::Pass
    };

    let message = if total_errors > 0 {
        format!("{total_errors} error(s) in {total_jobs} cron job(s)")
    } else {
        format!("{total_jobs} cron job(s) valid")
    };

    CheckResult {
        name: "cron_jobs".into(),
        status,
        message,
        details: Some(serde_json::to_value(&diag).unwrap()),
    }
}

inventory::submit! { register_check("cron_jobs", check_cron_jobs) }
