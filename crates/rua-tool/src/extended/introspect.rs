use async_trait::async_trait;

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::schema::ToolSpec;

pub const EFFECTIVE_TOOL_CATALOG_METADATA_KEY: &str = "effective_tool_catalog";
pub const EFFECTIVE_TOOL_NAMES_METADATA_KEY: &str = "effective_tool_names";

pub struct ListToolsTool;

impl ListToolsTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "list_tools".to_string(),
            description: "List all available tools with their names and descriptions. \
                          Use when you need to discover what tools are available."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            annotations: ExtendedToolAnnotations {
                read_only: true,
                idempotent: true,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

#[async_trait]
impl Tool for ListToolsTool {
    fn name(&self) -> &str {
        "list_tools"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(
        &self,
        _args: serde_json::Value,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let value = ctx
            .metadata
            .get(EFFECTIVE_TOOL_CATALOG_METADATA_KEY)
            .ok_or_else(|| rua_core::RuaError::Tool {
                tool: "list_tools".into(),
                message: "Current capability snapshot is unavailable".into(),
            })?;
        let mut summaries: Vec<(String, String)> =
            serde_json::from_value(value.clone()).map_err(|error| rua_core::RuaError::Tool {
                tool: "list_tools".into(),
                message: format!("Invalid capability catalog: {error}"),
            })?;
        summaries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut lines = vec![
            format!("Available tools ({}):", summaries.len()),
            String::new(),
        ];
        for (name, description) in summaries {
            lines.push(format!(
                "- **{}**: {}",
                name,
                truncate_description(&description, 120)
            ));
        }
        Ok(ToolResult::text("list_tools", lines.join("\n")))
    }
}

fn truncate_description(desc: &str, max_len: usize) -> String {
    let first_line = desc.lines().next().unwrap_or(desc);
    if first_line.chars().count() <= max_len {
        first_line.to_string()
    } else {
        format!(
            "{}...",
            first_line.chars().take(max_len).collect::<String>()
        )
    }
}
