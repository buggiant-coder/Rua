use std::path::Path;
use std::process::{Command, Output};

use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchRequest, ToolchainIntent,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectGitStatus {
    pub repository: bool,
    pub branch: Option<String>,
    pub detached: bool,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<ProjectGitChange>,
    pub head: Option<ProjectGitHead>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectGitChange {
    pub path: String,
    pub original_path: Option<String>,
    pub status: String,
    pub staged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectGitHead {
    pub oid: String,
    pub summary: String,
    pub authored_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectGitCommit {
    pub oid: String,
    pub parent_oids: Vec<String>,
    pub summary: String,
    pub authored_at: String,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectGitHistoryPage {
    pub items: Vec<ProjectGitCommit>,
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
}

fn git_output(root: &Path, args: &[&str]) -> Result<Output, String> {
    let cache_dir = rua_config::dirs::resolve_cache_dir(&rua_config::dirs::resolve_rua_root(None));
    let authority = RuntimeAuthority::from_current_process(None, root, cache_dir);
    let launch = authority.plan(
        ProcessShape::Direct,
        ToolchainIntent::GenericCommand {
            command: "git".into(),
        },
        RuntimeLaunchRequest {
            cwd: Some(root.to_owned()),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: [
                ("LC_ALL".into(), "C".into()),
                ("GIT_OPTIONAL_LOCKS".into(), "0".into()),
            ]
            .into(),
            ..Default::default()
        },
    );
    let process = launch.process().map_err(|error| error.to_string())?;
    let mut command = Command::new(&process.command);
    command.args(&process.args).envs(&process.env);
    if let Some(cwd) = &process.cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .map_err(|error| format!("Run Git in {}: {error}", root.display()))?;
    if !output.status.success()
        && !String::from_utf8_lossy(&output.stderr).contains("not a git repository")
    {
        return Err(format!(
            "Git in {}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output)
}

pub fn inspect_project_git(root: &Path) -> Result<ProjectGitStatus, String> {
    let status = git_output(
        root,
        &[
            "-c",
            "status.relativePaths=true",
            "status",
            "--porcelain=v1",
            "-z",
            "--branch",
            "--untracked-files=all",
            "--",
            ".",
        ],
    )?;
    if !status.status.success() {
        return Ok(ProjectGitStatus {
            repository: false,
            branch: None,
            detached: false,
            ahead: 0,
            behind: 0,
            changes: Vec::new(),
            head: None,
        });
    }

    let records = status
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| String::from_utf8_lossy(record).into_owned())
        .collect::<Vec<_>>();
    let mut branch = None;
    let mut detached = false;
    let mut ahead = 0;
    let mut behind = 0;
    let mut changes = Vec::new();
    let mut index = 0;

    if let Some(header) = records
        .first()
        .and_then(|record| record.strip_prefix("## "))
    {
        let branch_part = header.split("...").next().unwrap_or(header);
        detached = branch_part.starts_with("HEAD ");
        if !detached && branch_part != "No commits yet on" {
            branch = Some(branch_part.trim().to_string());
        }
        ahead = parse_tracking_count(header, "ahead ");
        behind = parse_tracking_count(header, "behind ");
        index = 1;
    }

    while index < records.len() {
        let record = &records[index];
        if record.len() < 3 {
            index += 1;
            continue;
        }
        let status_code = &record[..2];
        let index_status = status_code.as_bytes()[0] as char;
        let worktree_status = status_code.as_bytes()[1] as char;
        let path = record[3..].to_string();
        let rename = matches!(index_status, 'R' | 'C') || matches!(worktree_status, 'R' | 'C');
        let original_path = if rename {
            index += 1;
            records.get(index).cloned()
        } else {
            None
        };
        let display_status = if status_code == "??" {
            "?"
        } else if index_status != ' ' {
            &record[..1]
        } else {
            &record[1..2]
        };
        changes.push(ProjectGitChange {
            path,
            original_path,
            status: display_status.to_string(),
            staged: index_status != ' ' && index_status != '?',
        });
        index += 1;
    }

    let head = inspect_head(root);
    Ok(ProjectGitStatus {
        repository: true,
        branch,
        detached,
        ahead,
        behind,
        changes,
        head,
    })
}

pub fn inspect_project_git_history(
    root: &Path,
    offset: usize,
    limit: usize,
) -> Result<ProjectGitHistoryPage, String> {
    let count = git_output(root, &["rev-list", "--all", "--count"])?;
    if !count.status.success() {
        return Ok(ProjectGitHistoryPage {
            items: Vec::new(),
            offset,
            limit,
            total: 0,
        });
    }
    let total = String::from_utf8_lossy(&count.stdout)
        .trim()
        .parse()
        .map_err(|error| format!("parse git history count: {error}"))?;
    let output = git_output(
        root,
        &[
            "log",
            "--all",
            "--topo-order",
            &format!("--skip={offset}"),
            &format!("--max-count={limit}"),
            "--format=%H%x00%P%x00%s%x00%aI%x00%D%x1e",
        ],
    )?;
    if !output.status.success() {
        return Err(format!(
            "read git history: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let items = output
        .stdout
        .split(|byte| *byte == 0x1e)
        .filter_map(|record| {
            let fields = record
                .split(|byte| *byte == 0)
                .map(|field| String::from_utf8_lossy(field).trim().to_string())
                .collect::<Vec<_>>();
            Some(ProjectGitCommit {
                oid: fields.first()?.clone(),
                parent_oids: fields
                    .get(1)?
                    .split_whitespace()
                    .map(str::to_string)
                    .collect(),
                summary: fields.get(2)?.clone(),
                authored_at: fields.get(3)?.clone(),
                refs: fields
                    .get(4)?
                    .split(", ")
                    .map(str::trim)
                    .filter(|reference| !reference.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect();
    Ok(ProjectGitHistoryPage {
        items,
        offset,
        limit,
        total,
    })
}

fn parse_tracking_count(header: &str, prefix: &str) -> u32 {
    header
        .split(['[', ',', ']'])
        .map(str::trim)
        .find_map(|part| part.strip_prefix(prefix))
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn inspect_head(root: &Path) -> Option<ProjectGitHead> {
    let output = git_output(root, &["log", "-1", "--format=%H%x00%s%x00%aI"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let fields = output
        .stdout
        .split(|byte| *byte == 0)
        .map(|field| String::from_utf8_lossy(field).trim().to_string())
        .collect::<Vec<_>>();
    Some(ProjectGitHead {
        oid: fields.first()?.clone(),
        summary: fields.get(1)?.clone(),
        authored_at: fields.get(2)?.clone(),
    })
}
