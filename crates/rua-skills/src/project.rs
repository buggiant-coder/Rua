use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::frontmatter::{parse_skill_md_file, ParsedSkillMd};

pub const PROJECT_SKILLS_RELATIVE_ROOT: &str = ".agents/skills";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectSkillDiscoveryLimits {
    pub max_depth: usize,
    pub max_directories: usize,
}

impl Default for ProjectSkillDiscoveryLimits {
    fn default() -> Self {
        Self {
            max_depth: 6,
            max_directories: 2_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProjectSkill {
    pub parsed: ParsedSkillMd,

    pub base_dir: PathBuf,
    pub skill_md_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectSkillDiagnosticKind {
    DirectoryUnreadable,
    SkillUnreadableOrMalformed,
    DepthLimitReached,
    DirectoryLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSkillDiagnostic {
    pub kind: ProjectSkillDiagnosticKind,
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectSkillDiscovery {
    pub skills: Vec<ProjectSkill>,
    pub diagnostics: Vec<ProjectSkillDiagnostic>,
    pub visited_directories: usize,
}

pub fn discover_project_skills(project_root: &Path) -> ProjectSkillDiscovery {
    discover_project_skills_with_limits(project_root, ProjectSkillDiscoveryLimits::default())
}

pub fn discover_project_skills_with_limits(
    project_root: &Path,
    limits: ProjectSkillDiscoveryLimits,
) -> ProjectSkillDiscovery {
    let skills_root = project_root.join(PROJECT_SKILLS_RELATIVE_ROOT);
    match std::fs::metadata(&skills_root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return ProjectSkillDiscovery {
                diagnostics: vec![ProjectSkillDiagnostic {
                    kind: ProjectSkillDiagnosticKind::DirectoryUnreadable,
                    path: skills_root,
                    detail: "Project Skill root is not a directory".to_string(),
                }],
                ..ProjectSkillDiscovery::default()
            };
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProjectSkillDiscovery::default();
        }
        Err(error) => {
            return ProjectSkillDiscovery {
                diagnostics: vec![ProjectSkillDiagnostic {
                    kind: ProjectSkillDiagnosticKind::DirectoryUnreadable,
                    path: skills_root,
                    detail: error.to_string(),
                }],
                ..ProjectSkillDiscovery::default()
            };
        }
    }

    let mut discovery = ProjectSkillDiscovery::default();
    let mut queue = VecDeque::from([(skills_root.clone(), 0_usize)]);
    let mut visited_directories = HashSet::new();
    let mut visited_skill_files = HashSet::new();
    let mut depth_reported = false;

    while let Some((directory, depth)) = queue.pop_front() {
        if discovery.visited_directories >= limits.max_directories {
            discovery.diagnostics.push(ProjectSkillDiagnostic {
                kind: ProjectSkillDiagnosticKind::DirectoryLimitReached,
                path: directory,
                detail: format!(
                    "Project Skill scan stopped after {} visited directories",
                    limits.max_directories
                ),
            });
            break;
        }
        let canonical_directory = match std::fs::canonicalize(&directory) {
            Ok(path) => path,
            Err(error) => {
                discovery.diagnostics.push(ProjectSkillDiagnostic {
                    kind: ProjectSkillDiagnosticKind::DirectoryUnreadable,
                    path: directory,
                    detail: error.to_string(),
                });
                continue;
            }
        };
        if !visited_directories.insert(canonical_directory) {
            continue;
        }
        discovery.visited_directories += 1;

        let skill_md_path = directory.join("SKILL.md");
        if std::fs::metadata(&skill_md_path).is_ok_and(|metadata| metadata.is_file()) {
            match std::fs::canonicalize(&skill_md_path) {
                Ok(canonical_skill_md) => {
                    if visited_skill_files.insert(canonical_skill_md) {
                        match parse_skill_md_file(&directory) {
                            Ok(parsed) => discovery.skills.push(ProjectSkill {
                                parsed,
                                base_dir: directory.clone(),
                                skill_md_path,
                            }),
                            Err(error) => {
                                discovery.diagnostics.push(ProjectSkillDiagnostic {
                                    kind: ProjectSkillDiagnosticKind::SkillUnreadableOrMalformed,
                                    path: skill_md_path,
                                    detail: error.to_string(),
                                });
                            }
                        }
                    }
                }
                Err(error) => discovery.diagnostics.push(ProjectSkillDiagnostic {
                    kind: ProjectSkillDiagnosticKind::SkillUnreadableOrMalformed,
                    path: skill_md_path,
                    detail: error.to_string(),
                }),
            }
        }

        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                discovery.diagnostics.push(ProjectSkillDiagnostic {
                    kind: ProjectSkillDiagnosticKind::DirectoryUnreadable,
                    path: directory,
                    detail: error.to_string(),
                });
                continue;
            }
        };
        let mut entries = entries.flatten().collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            if name.to_str().is_none_or(|name| name.starts_with('.')) {
                continue;
            }
            let path = entry.path();
            let is_directory = std::fs::metadata(&path)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false);
            if !is_directory {
                continue;
            }
            if depth >= limits.max_depth {
                if !depth_reported {
                    discovery.diagnostics.push(ProjectSkillDiagnostic {
                        kind: ProjectSkillDiagnosticKind::DepthLimitReached,
                        path,
                        detail: format!(
                            "Project Skill scan does not descend beyond {} directory levels",
                            limits.max_depth
                        ),
                    });
                    depth_reported = true;
                }
                continue;
            }
            queue.push_back((path, depth + 1));
        }
    }

    discovery
        .skills
        .sort_by(|left, right| left.base_dir.cmp(&right.base_dir));
    discovery
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSkillWatchPlan {
    pub path: PathBuf,
    pub recursive: bool,
}

pub fn project_skill_watch_plan(project_root: &Path) -> ProjectSkillWatchPlan {
    let skills_root = project_root.join(PROJECT_SKILLS_RELATIVE_ROOT);
    if skills_root.is_dir() {
        return ProjectSkillWatchPlan {
            path: skills_root,
            recursive: true,
        };
    }
    let agents_root = project_root.join(".agents");
    if agents_root.is_dir() {
        return ProjectSkillWatchPlan {
            path: agents_root,
            recursive: true,
        };
    }
    ProjectSkillWatchPlan {
        path: project_root.to_path_buf(),
        recursive: false,
    }
}

pub struct ProjectSkillWatcher {
    _watcher: RecommendedWatcher,
    plan: ProjectSkillWatchPlan,
}

impl ProjectSkillWatcher {
    pub fn start(
        project_root: &Path,
        on_change: Arc<dyn Fn() + Send + Sync + 'static>,
    ) -> Result<Self, notify::Error> {
        let plan = project_skill_watch_plan(project_root);
        let callback = Arc::clone(&on_change);
        let mut watcher =
            notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
                let Ok(event) = result else {
                    return;
                };
                if matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) {
                    callback();
                }
            })?;
        watcher.watch(
            &plan.path,
            if plan.recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            },
        )?;
        Ok(Self {
            _watcher: watcher,
            plan,
        })
    }

    pub fn plan(&self) -> &ProjectSkillWatchPlan {
        &self.plan
    }
}
