use std::collections::HashMap;
use std::sync::Arc;

use rua_contract::Tool;

use crate::error::ToolError;
use crate::executor::CommandExecutor;
use crate::schema::ToolSpec;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToolNamespace {
    Builtin,
    Mcp,
    Skill,
}

impl std::fmt::Display for ToolNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin => write!(f, "builtin"),
            Self::Mcp => write!(f, "mcp"),
            Self::Skill => write!(f, "skill"),
        }
    }
}

struct ToolEntry {
    tool: Arc<dyn Tool>,
    spec: ToolSpec,
    namespace: ToolNamespace,
    enabled: bool,
}

pub struct ToolRegistry {
    tools: HashMap<String, ToolEntry>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(
        &mut self,
        tool: Arc<dyn Tool>,
        spec: ToolSpec,
        namespace: ToolNamespace,
    ) -> Result<(), ToolError> {
        let name = spec.name.clone();

        if self.tools.contains_key(&name) {
            return Err(ToolError::Duplicate { name });
        }

        self.tools.insert(
            name,
            ToolEntry {
                tool,
                spec,
                namespace,
                enabled: true,
            },
        );

        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .get(name)
            .filter(|e| e.enabled)
            .map(|e| e.tool.clone())
    }

    pub fn get_spec(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.get(name).map(|e| &e.spec)
    }

    pub fn enable(&mut self, name: &str) -> bool {
        if let Some(entry) = self.tools.get_mut(name) {
            entry.enabled = true;
            true
        } else {
            false
        }
    }

    pub fn disable(&mut self, name: &str) -> bool {
        if let Some(entry) = self.tools.get_mut(name) {
            entry.enabled = false;
            true
        } else {
            false
        }
    }

    pub fn list_enabled(&self) -> Vec<&str> {
        self.tools
            .iter()
            .filter(|(_, e)| e.enabled)
            .map(|(name, _)| name.as_str())
            .collect()
    }

    pub fn list_specs(&self) -> Vec<&ToolSpec> {
        self.tools.values().map(|e| &e.spec).collect()
    }

    pub fn list_specs_in_namespace(&self, namespace: &ToolNamespace) -> Vec<&ToolSpec> {
        self.tools
            .values()
            .filter(|e| &e.namespace == namespace)
            .map(|e| &e.spec)
            .collect()
    }

    pub fn export_tool_definitions(&self) -> Vec<agent_types::ToolDefinition> {
        self.tools
            .values()
            .filter(|e| e.enabled)
            .map(|e| e.spec.to_tool_definition())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn register_core_tools(
        &mut self,
        executor: Arc<dyn CommandExecutor>,
    ) -> Result<(), ToolError> {
        use crate::core::{EditFileTool, ReadFileTool, ShellTool};

        self.register(
            Arc::new(ReadFileTool::new()),
            ReadFileTool::spec(),
            ToolNamespace::Builtin,
        )?;
        self.register(
            Arc::new(EditFileTool::new()),
            EditFileTool::spec(),
            ToolNamespace::Builtin,
        )?;
        self.register(
            Arc::new(ShellTool::new(executor)),
            ShellTool::spec(),
            ToolNamespace::Builtin,
        )?;

        Ok(())
    }

    pub fn iter_tools(
        &self,
    ) -> impl Iterator<Item = (&str, agent_types::ToolDefinition, Arc<dyn Tool>)> + '_ {
        self.tools
            .iter()
            .filter(|(_, e)| e.enabled)
            .map(|(name, e)| (name.as_str(), e.spec.to_tool_definition(), e.tool.clone()))
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.tools.remove(name).is_some()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn tool_catalog_from_specs<'a>(
    specs: impl IntoIterator<Item = &'a ToolSpec>,
) -> serde_json::Map<String, serde_json::Value> {
    let defaults = rua_config::types::tools::default_builtin_tools();
    specs
        .into_iter()
        .map(|spec| {
            let display_group = defaults
                .get(&spec.name)
                .and_then(|value| value.get("display_group"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            (
                spec.name.clone(),
                serde_json::json!({
                    "name": spec.name,
                    "enabled": true,
                    "description": spec.description,
                    "display_to_user": true,
                    "async_execution": false,
                    "display_group": display_group,
                }),
            )
        })
        .collect()
}
