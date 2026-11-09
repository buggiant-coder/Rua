use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::services::product_hooks::{
    CreateProjectHookDefinition, HookHandlerView, ProductHookService,
};

pub const RESERVED_PROJECT_HOOK_MANIFEST: &str = ".rua/hooks.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectHookDiscoveryReport {
    pub registered: Vec<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ProjectHookManifest {
    version: u32,
    hooks: Vec<ProjectHookDeclaration>,
}

#[derive(Debug, Deserialize)]
struct ProjectHookDeclaration {
    id: String,
    display_name: String,
    event: String,
    #[serde(default)]
    capabilities: Vec<String>,
    handler: HookHandlerView,
}

pub async fn discover(
    project_id: &str,
    project_root: &Path,
    hooks: &ProductHookService,
) -> ProjectHookDiscoveryReport {
    let path = project_root.join(RESERVED_PROJECT_HOOK_MANIFEST);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProjectHookDiscoveryReport {
                registered: Vec::new(),
                diagnostics: Vec::new(),
            };
        }
        Err(error) => {
            return ProjectHookDiscoveryReport {
                registered: Vec::new(),
                diagnostics: vec![format!(
                    "failed to read Project Hook manifest '{}': {error}",
                    path.display()
                )],
            };
        }
    };
    let manifest: ProjectHookManifest = match serde_json::from_slice(&bytes) {
        Ok(manifest) => manifest,
        Err(error) => {
            return ProjectHookDiscoveryReport {
                registered: Vec::new(),
                diagnostics: vec![format!(
                    "failed to parse Project Hook manifest '{}': {error}",
                    path.display()
                )],
            };
        }
    };
    if manifest.version != 1 {
        return ProjectHookDiscoveryReport {
            registered: Vec::new(),
            diagnostics: vec![format!(
                "unsupported Project Hook manifest version {} in '{}'; expected version 1",
                manifest.version,
                path.display()
            )],
        };
    }

    let declarations = match manifest
        .hooks
        .into_iter()
        .map(|declaration| -> Result<_, String> {
            let mut handler = declaration.handler;
            if handler.kind == "command" {
                if let Some(working_directory) = handler.working_directory.as_deref() {
                    handler.working_directory = Some(resolve_project_working_directory(
                        project_root,
                        Path::new(working_directory),
                    )?);
                }
            }
            Ok(CreateProjectHookDefinition {
                logical_id: declaration.id,
                display_name: declaration.display_name,
                event: declaration.event,
                capabilities: declaration.capabilities,
                handler,
            })
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(declarations) => declarations,
        Err(error) => {
            return ProjectHookDiscoveryReport {
                registered: Vec::new(),
                diagnostics: vec![error],
            };
        }
    };
    match hooks
        .register_project_definitions(project_id, declarations)
        .await
    {
        Ok(definitions) => {
            let registered = definitions
                .iter()
                .map(|definition| definition.id.clone())
                .collect();
            let diagnostics = definitions
                .iter()
                .flat_map(|definition| {
                    definition
                        .availability
                        .diagnostics
                        .iter()
                        .chain(definition.validation.diagnostics.iter())
                        .chain(definition.diagnostics.iter())
                        .map(move |diagnostic| format!("{}: {diagnostic}", definition.id))
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            ProjectHookDiscoveryReport {
                registered,
                diagnostics,
            }
        }
        Err(error) => ProjectHookDiscoveryReport {
            registered: Vec::new(),
            diagnostics: vec![error],
        },
    }
}

fn resolve_project_working_directory(
    project_root: &Path,
    working_directory: &Path,
) -> Result<String, String> {
    if working_directory.is_absolute() {
        return Ok(working_directory.to_string_lossy().into_owned());
    }
    let mut resolved = PathBuf::from(project_root);
    for component in working_directory.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => resolved.push(segment),
            Component::ParentDir => {
                return Err(format!(
                    "Project Hook working_directory '{}' cannot escape the Project root",
                    working_directory.display()
                ));
            }
            Component::Prefix(_) | Component::RootDir => {
                unreachable!("absolute Project Hook working directories returned above")
            }
        }
    }
    Ok(resolved.to_string_lossy().into_owned())
}
