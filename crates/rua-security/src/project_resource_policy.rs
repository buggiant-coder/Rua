use std::path::{Path, PathBuf};

use rua_config::types::security::SecurityConfig;
use serde::{Deserialize, Serialize};

pub const PROJECT_RESOURCE_POLICY_KEY: &str = "project_resource_policy";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectResourceProfile {
    ReadOnly,
    #[default]
    ResourceWrite,
    DangerFullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectResourcePolicySource {
    ProjectDefault,
    AgentSecurity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveProjectResourcePolicy {
    pub profile: ProjectResourceProfile,
    pub source: ProjectResourcePolicySource,
    pub cwd: String,
    pub writable_roots: Vec<String>,
    pub dedicated_operations_root_confined: bool,
    pub tool_guard_enabled: bool,
    pub approval_mode: String,
}

impl EffectiveProjectResourcePolicy {
    pub fn resolve(
        config: &SecurityConfig,
        resource_root: &Path,
    ) -> EffectiveProjectResourcePolicy {
        Self::resolve_with_roots(config, resource_root, &[resource_root.to_path_buf()])
    }

    pub fn resolve_with_roots(
        config: &SecurityConfig,
        working_dir: &Path,
        resource_roots: &[PathBuf],
    ) -> EffectiveProjectResourcePolicy {
        let resource_root = working_dir;
        let configured = config
            .extra
            .get(PROJECT_RESOURCE_POLICY_KEY)
            .and_then(|value| value.get("profile"))
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok());
        let (profile, source) = match configured {
            Some(profile) => (profile, ProjectResourcePolicySource::AgentSecurity),
            None => (
                ProjectResourceProfile::ResourceWrite,
                ProjectResourcePolicySource::ProjectDefault,
            ),
        };
        let cwd = resource_root.to_string_lossy().to_string();
        let writable_roots = match profile {
            ProjectResourceProfile::ResourceWrite => resource_roots
                .iter()
                .map(|root| root.to_string_lossy().to_string())
                .collect(),
            ProjectResourceProfile::ReadOnly | ProjectResourceProfile::DangerFullAccess => {
                Vec::new()
            }
        };
        Self {
            profile,
            source,
            cwd,
            writable_roots,
            dedicated_operations_root_confined: true,
            tool_guard_enabled: config.tool_guard.enabled,
            approval_mode: config.tool_guard.approval_mode.clone(),
        }
    }

    pub fn resolve_dedicated_path(&self, requested: &Path) -> Result<PathBuf, String> {
        let root = PathBuf::from(&self.cwd)
            .canonicalize()
            .map_err(|error| format!("Project root is unavailable: {error}"))?;
        let candidate = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            root.join(requested)
        };
        let resolved = if candidate.exists() {
            candidate
                .canonicalize()
                .map_err(|error| format!("Project path is unavailable: {error}"))?
        } else {
            let parent = candidate
                .parent()
                .ok_or_else(|| "Project path has no parent".to_string())?
                .canonicalize()
                .map_err(|error| format!("Project path parent is unavailable: {error}"))?;
            let name = candidate
                .file_name()
                .ok_or_else(|| "Project path has no file name".to_string())?;
            parent.join(name)
        };
        if !resolved.starts_with(&root) {
            return Err(
                "Project Files and Git paths must stay inside the Resource Root".to_string(),
            );
        }
        Ok(resolved)
    }
}

/// Resolve existing symlinks before checking containment, including a symlink at
/// the destination itself. Nonexistent suffixes are appended to the nearest
/// existing ancestor so creation cannot escape through an intermediate link.
fn canonical_destination(path: &Path) -> Result<PathBuf, String> {
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                suffix.push(
                    ancestor
                        .file_name()
                        .ok_or("invalid resource path")?
                        .to_os_string(),
                );
                ancestor = ancestor.parent().ok_or("resource path has no ancestor")?;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    let mut resolved = ancestor.canonicalize().map_err(|error| error.to_string())?;
    for part in suffix.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

impl EffectiveProjectResourcePolicy {
    pub fn permits_write(&self, path: &Path) -> bool {
        match self.profile {
            ProjectResourceProfile::DangerFullAccess => return true,
            ProjectResourceProfile::ReadOnly => return false,
            ProjectResourceProfile::ResourceWrite => {}
        }
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            Path::new(&self.cwd).join(path)
        };
        // Normalize after symlink resolution; parent traversal in a missing suffix
        // cannot be treated as a child of an authorized root.
        let Ok(path) = canonical_destination(&path) else {
            return false;
        };
        let path = PathBuf::from(crate::file_guardian::normalize_path(
            &path.to_string_lossy(),
        ));
        self.writable_roots.iter().any(|root| {
            Path::new(root)
                .canonicalize()
                .is_ok_and(|root| path.starts_with(root))
        })
    }
}

pub(crate) fn tool_finding(
    config: &SecurityConfig,
    ctx: &crate::ToolCallContext,
) -> Option<crate::ToolGuardFinding> {
    let roots: Vec<PathBuf> =
        serde_json::from_value(ctx.metadata.get("project_resource_roots")?.clone()).ok()?;
    let policy = EffectiveProjectResourcePolicy::resolve_with_roots(
        config,
        Path::new(&ctx.working_dir),
        &roots,
    );
    if policy.profile == ProjectResourceProfile::DangerFullAccess {
        return None;
    }
    let reason = match ctx.tool_name.as_str() {
        "write" | "edit" => {
            let paths: Vec<String> = if ctx.tool_name == "edit" {
                ctx.tool_input.get("patch").and_then(serde_json::Value::as_str)
                    .map(|patch| patch.lines().filter_map(|line| {
                        line.strip_prefix("*** Add File: ").or_else(|| line.strip_prefix("*** Update File: ")).or_else(||line.strip_prefix("*** Delete File: ")).or_else(||line.strip_prefix("*** Move to: "))
                            .map(|path| path.trim().to_string())
                    }).collect()).unwrap_or_default()
            } else {
                ctx.tool_input.get("path").and_then(serde_json::Value::as_str)
                    .map(|path| vec![path.to_string()]).unwrap_or_default()
            };
            if !paths.is_empty() && paths.iter().all(|path| policy.permits_write(Path::new(path))) { return None; }
            "The write is outside the Project's writable resource directories"
        },
        // Arbitrary programs and external tools do not expose a trustworthy write
        // set. Use existing approvals instead of pretending cwd is a sandbox.
        "bash" if ctx.tool_input.get("command").and_then(serde_json::Value::as_str).is_some_and(simple_read_command) => return None,
        "bash" | "python" => "This program can write beyond the Project resource directories; explicit approval is required",
        name if name.starts_with("mcp_") || name.contains("__") => "This external tool does not declare Project resource confinement; explicit approval is required",
        _ => return None,
    };
    Some(crate::ToolGuardFinding {
        rule_id: "PROJECT_RESOURCE_WRITE".into(),
        category: "project_resources".into(),
        severity: crate::Severity::High,
        title: "Project resource access".into(),
        description: reason.into(),
        tool_name: ctx.tool_name.clone(),
        guardian: "project_resource_policy".into(),
        message: reason.into(),
        remediation: "Use a bound directory, or explicitly approve this operation".into(),
        matched_pattern: None,
        matched_value: None,
    })
}

fn simple_read_command(command: &str) -> bool {
    if command.chars().any(|c| {
        matches!(
            c,
            '$' | '`' | ';' | '|' | '&' | '<' | '>' | '\n' | '\r' | '(' | ')' | '\\'
        )
    }) {
        return false;
    }
    shlex::split(command)
        .and_then(|words| words.first().cloned())
        .is_some_and(|name| {
            matches!(
                name.as_str(),
                "ls" | "cat" | "stat" | "head" | "tail" | "pwd"
            )
        })
}
