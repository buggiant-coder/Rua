use std::path::Path;
use std::process::{Command, Output};

use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchRequest, ToolchainIntent,
};
use rua_core::runtime_layout::{ProcessLaunchPlan, RuntimeKind};
use serde::{Deserialize, Serialize};

const GIT: &str = "git";

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

/// Runtime-selected `git` runner.
///
/// The resident Android server is a native process whose PATH has no `git`; the Linux
/// Compatibility Runtime holds it. Planning the launch here keeps that boundary explicit
/// instead of spawning a bare `git` that does not exist on the host.
struct GitRuntime<'a> {
    authority: &'a RuntimeAuthority,
    linux_compat: bool,
}

impl<'a> GitRuntime<'a> {
    fn new(authority: &'a RuntimeAuthority) -> Self {
        Self {
            authority,
            linux_compat: authority.runtime_kind() == RuntimeKind::LinuxCompat,
        }
    }

    fn plan(
        &self,
        root: &Path,
        label: &str,
        args: Vec<String>,
    ) -> Result<ProcessLaunchPlan, String> {
        let mut planned_args = vec!["-C".to_string(), root.to_string_lossy().into_owned()];
        if self.linux_compat {
            // PRoot maps the host uid to root, so git rejects the repository as "dubious
            // ownership". The guest path is not stable enough to name a single directory.
            planned_args.push("-c".to_string());
            planned_args.push("safe.directory=*".to_string());
        }
        planned_args.extend(args);

        self.authority
            .plan(
                ProcessShape::Direct,
                ToolchainIntent::GenericCommand {
                    command: GIT.to_string(),
                },
                RuntimeLaunchRequest {
                    args: planned_args,
                    ..Default::default()
                },
            )
            .into_process()
            .map_err(|error| format!("{label}: {error}"))
    }

    fn run(&self, root: &Path, label: &str, args: &[String]) -> Result<Output, String> {
        let process = self.plan(root, label, args.to_vec())?;
        let mut command = Command::new(&process.command);
        command.args(&process.args);
        if self.linux_compat {
            // The launch plan carries the complete guest environment.
            command.env_clear();
        }
        command.envs(&process.env);
        command
            .output()
            .map_err(|error| format!("{label}: {error}"))
    }
}

fn authority_for(root: &Path) -> RuntimeAuthority {
    RuntimeAuthority::from_current_process(
        None,
        root,
        std::env::temp_dir().join("rua-runtime-cache"),
    )
}

/// Exposes the launch plan for the `git` command this crate would run.
///
/// Lets device-less tests assert the Runtime Authority boundary without spawning PRoot.
#[doc(hidden)]
pub fn project_git_launch_plan(
    authority: &RuntimeAuthority,
    root: &Path,
) -> Result<ProcessLaunchPlan, String> {
    GitRuntime::new(authority).plan(root, "plan git", Vec::new())
}

/// Runtime Authority injected form of [`inspect_project_git`].
#[doc(hidden)]
pub fn inspect_project_git_with(
    authority: &RuntimeAuthority,
    root: &Path,
) -> Result<ProjectGitStatus, String> {
    let git = GitRuntime::new(authority);
    inspect_git_with(&git, root)
}

/// Runtime Authority injected form of [`inspect_project_git_history`].
#[doc(hidden)]
pub fn inspect_project_git_history_with(
    authority: &RuntimeAuthority,
    root: &Path,
    offset: usize,
    limit: usize,
) -> Result<ProjectGitHistoryPage, String> {
    let git = GitRuntime::new(authority);
    inspect_git_history_with(&git, root, offset, limit)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectGitHistoryPage {
    pub items: Vec<ProjectGitCommit>,
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
}

pub fn inspect_project_git(root: &Path) -> Result<ProjectGitStatus, String> {
    let authority = authority_for(root);
    inspect_project_git_with(&authority, root)
}

fn inspect_git_with(git: &GitRuntime<'_>, root: &Path) -> Result<ProjectGitStatus, String> {
    let status = git.run(
        root,
        "run git status",
        &[
            "-c".to_string(),
            "status.relativePaths=true".to_string(),
            "status".to_string(),
            "--porcelain=v1".to_string(),
            "-z".to_string(),
            "--branch".to_string(),
            "--untracked-files=all".to_string(),
            "--".to_string(),
            ".".to_string(),
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

    let head = inspect_head(git, root);
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
    let authority = authority_for(root);
    inspect_project_git_history_with(&authority, root, offset, limit)
}

fn inspect_git_history_with(
    git: &GitRuntime<'_>,
    root: &Path,
    offset: usize,
    limit: usize,
) -> Result<ProjectGitHistoryPage, String> {
    let count = git.run(
        root,
        "count git history",
        &[
            "rev-list".to_string(),
            "--all".to_string(),
            "--count".to_string(),
        ],
    )?;
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
    let output = git.run(
        root,
        "read git history",
        &[
            "log".to_string(),
            "--all".to_string(),
            "--topo-order".to_string(),
            format!("--skip={offset}"),
            format!("--max-count={limit}"),
            "--format=%H%x00%P%x00%s%x00%aI%x00%D%x1e".to_string(),
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

fn inspect_head(git: &GitRuntime<'_>, root: &Path) -> Option<ProjectGitHead> {
    let output = git
        .run(
            root,
            "read git head",
            &[
                "log".to_string(),
                "-1".to_string(),
                "--format=%H%x00%s%x00%aI".to_string(),
            ],
        )
        .ok()?;
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
