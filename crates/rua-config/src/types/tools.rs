use crate::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub builtin_tools: serde_json::Map<String, serde_json::Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_policy: Option<ToolSelectionPolicy>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolIdentityKind {
    Builtin,
    Mcp,
    Plugin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolIdentity {
    pub kind: ToolIdentityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub name: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ToolIdentity {
    pub fn builtin(name: impl Into<String>) -> Self {
        Self {
            kind: ToolIdentityKind::Builtin,
            source: None,
            name: name.into(),
            extra: Default::default(),
        }
    }

    pub fn mcp(source: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: ToolIdentityKind::Mcp,
            source: Some(source.into()),
            name: name.into(),
            extra: Default::default(),
        }
    }

    pub fn plugin(source: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: ToolIdentityKind::Plugin,
            source: Some(source.into()),
            name: name.into(),
            extra: Default::default(),
        }
    }

    pub fn same_identity(&self, other: &Self) -> bool {
        self.kind == other.kind && self.source == other.source && self.name == other.name
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Tool identity name must not be empty".into());
        }
        match self.kind {
            ToolIdentityKind::Builtin if self.source.is_some() => {
                Err("Builtin Tool identity must not have a source".into())
            }
            ToolIdentityKind::Mcp | ToolIdentityKind::Plugin
                if self
                    .source
                    .as_ref()
                    .is_none_or(|source| source.trim().is_empty()) =>
            {
                Err("External Tool identity requires a source".into())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ToolSelectionPolicy {
    DefaultEnabled {
        disabled_tools: Vec<ToolIdentity>,
        disabled_capabilities: Vec<String>,
        #[serde(flatten)]
        extra: serde_json::Map<String, serde_json::Value>,
    },
    Restricted {
        allowed_tools: Vec<ToolIdentity>,
        #[serde(flatten)]
        extra: serde_json::Map<String, serde_json::Value>,
    },
}

impl ToolSelectionPolicy {
    pub fn default_enabled() -> Self {
        Self::DefaultEnabled {
            disabled_tools: Vec::new(),
            disabled_capabilities: Vec::new(),
            extra: Default::default(),
        }
    }

    pub fn restricted(allowed_tools: Vec<ToolIdentity>) -> Self {
        Self::Restricted {
            allowed_tools,
            extra: Default::default(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let identities = match self {
            Self::DefaultEnabled {
                disabled_tools,
                disabled_capabilities,
                extra,
            } => {
                if extra.contains_key("allowed_tools") {
                    return Err(
                        "Default-Enabled policy cannot contain an allowed Tool inventory".into(),
                    );
                }
                if disabled_capabilities
                    .iter()
                    .any(|name| name.trim().is_empty())
                {
                    return Err("Disabled capability identity must not be empty".into());
                }
                disabled_tools
            }
            Self::Restricted {
                allowed_tools,
                extra,
            } => {
                if extra.contains_key("disabled_tools")
                    || extra.contains_key("disabled_capabilities")
                {
                    return Err("Restricted policy cannot contain a disabled Tool inventory".into());
                }
                allowed_tools
            }
        };
        for identity in identities {
            identity.validate()?;
        }
        Ok(())
    }

    pub fn selects(&self, identity: &ToolIdentity, capability: Option<&str>) -> bool {
        if identity.validate().is_err() {
            return false;
        }
        match self {
            Self::DefaultEnabled {
                disabled_tools,
                disabled_capabilities,
                ..
            } => {
                !disabled_tools
                    .iter()
                    .any(|tool| tool.same_identity(identity))
                    && !capability.is_some_and(|name| {
                        disabled_capabilities
                            .iter()
                            .any(|disabled| disabled == name)
                    })
            }
            Self::Restricted { allowed_tools, .. } => allowed_tools
                .iter()
                .any(|tool| tool.same_identity(identity)),
        }
    }

    pub fn set_tool_enabled(&mut self, identity: ToolIdentity, enabled: bool) {
        let (tools, include) = match self {
            Self::DefaultEnabled { disabled_tools, .. } => (disabled_tools, !enabled),
            Self::Restricted { allowed_tools, .. } => (allowed_tools, enabled),
        };
        if include {
            if !tools.iter().any(|tool| tool.same_identity(&identity)) {
                tools.push(identity);
            }
        } else {
            tools.retain(|tool| !tool.same_identity(&identity));
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinToolConfig {
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub description: String,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub display_to_user: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub async_execution: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub display_group: String,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}

impl ToolsConfig {
    pub fn get_tool(&self, name: &str) -> Option<BuiltinToolConfig> {
        self.builtin_tools
            .get(name)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}

/// Canonical builtin tool catalog.
///
/// `display_group` decouples UI grouping from tool naming — the console renders
/// tools by group, not by parsing the name prefix. `bash` and `python` share
/// the `shell` group; `list_tools` and `recall_tool_result` share `tool`;
/// `update_plan` sits alone in `plan`.
pub fn default_builtin_tools() -> serde_json::Map<String, serde_json::Value> {
    let tools: &[(&str, &str, &str)] = &[
        ("bash", "Execute shell commands", "shell"),
        (
            "python",
            "Execute Python code in a sandboxed subprocess",
            "shell",
        ),
        ("read", "Read file contents", "file"),
        ("edit", "Apply Codex-style file patches", "file"),
        ("grep", "Search file contents by pattern", "file"),
        ("glob", "Find files matching a glob pattern", "file"),
        (
            "view_image",
            "Load an image into LLM context for visual analysis",
            "media",
        ),
        (
            "view_video",
            "Load a video into LLM context for visual analysis",
            "media",
        ),
        ("time_now", "Get current date and time", "time"),
        ("set_timezone", "Set user timezone", "time"),
        ("token_usage", "Get llm token usage", "usage"),
        (
            "runtime_info",
            "Inspect current device, OS, runtime, paths, and tool capabilities",
            "runtime",
        ),
        (
            "conversation_search",
            "Search raw transcript spans omitted from compacted current context",
            "conversation",
        ),
        (
            "conversation_read_span",
            "Read a span of conversation messages",
            "conversation",
        ),
        (
            "list_tools",
            "List tools currently available to the agent",
            "tool",
        ),
        (
            "recall_tool_result",
            "Retrieve the full input and output of a previous tool call",
            "tool",
        ),
        ("web_fetch", "Fetch a URL and return content as Markdown", "web"),
        ("web_search", "Search the internet for information", "web"),
        ("task_create", "Create a task with DAG dependencies", "task"),
        (
            "task_send_input",
            "Send text or explicit EOF to a live Task Runner Session",
            "task",
        ),
        ("task_cancel", "Cancel a running or pending task", "task"),
        ("task_retry", "Retry a failed or cancelled task", "task"),
        (
            "task_read",
            "Read bounded output and current state from a Task",
            "task",
        ),
        ("task_list", "List all tasks in the current session", "task"),
        (
            "memory_search",
            "Search the agent's memory for relevant past conversations, knowledge, and stored information",
            "memory",
        ),
        (
            "memory_get",
            "Load complete long-term memory records by id",
            "memory",
        ),
        (
            "memory_write",
            "Save durable facts to the agent's long-term memory through MemoryManager",
            "memory",
        ),
        (
            "memory_update",
            "Update one existing long-term Memory record by id",
            "memory",
        ),
        (
            "memory_delete",
            "Delete one long-term Memory record by id",
            "memory",
        ),
        (
            "update_plan",
            "Update the current Turn's structured work checklist",
            "plan",
        ),
        (
            "chat_send",
            "Send input to an existing or explicitly new collaboration chat",
            "chat",
        ),
        ("chat_read", "Read messages from a collaboration chat", "chat"),
        (
            "chat_stop",
            "Request the target collaboration chat to stop its running turn",
            "chat",
        ),
        ("sync_ls", "List sync-transfer inbox entries", "sync"),
        ("sync_send", "Send data through the sync-transfer channel", "sync"),
    ];

    let mut map = serde_json::Map::with_capacity(tools.len());
    for &(name, description, group) in tools {
        map.insert(
            name.to_string(),
            serde_json::json!({
                "name": name,
                "description": description,
                "display_to_user": true,
                "async_execution": false,
                "display_group": group,
            }),
        );
    }
    map
}
