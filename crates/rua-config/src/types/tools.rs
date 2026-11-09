use crate::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub builtin_tools: serde_json::Map<String, serde_json::Value>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinToolConfig {
    pub name: String,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub enabled: bool,
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
                "enabled": true,
                "description": description,
                "display_to_user": true,
                "async_execution": false,
                "display_group": group,
            }),
        );
    }
    map
}
