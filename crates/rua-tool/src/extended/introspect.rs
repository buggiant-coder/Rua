use async_trait::async_trait;

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::schema::ToolSpec;

#[derive(Debug, Clone)]
struct ToolSummary {
    name: String,
    description: String,
}

pub struct ListToolsTool {
    summaries: Vec<ToolSummary>,
}

impl ListToolsTool {
    pub fn new(tool_list: Vec<(String, String)>) -> Self {
        let mut summaries: Vec<ToolSummary> = tool_list
            .into_iter()
            .map(|(name, description)| ToolSummary { name, description })
            .collect();
        summaries.sort_by(|a, b| a.name.cmp(&b.name));
        Self { summaries }
    }

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
        _ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let total = self.summaries.len();
        let mut lines = Vec::with_capacity(total + 2);
        lines.push(format!("Available tools ({total}):"));
        lines.push(String::new());
        for s in &self.summaries {
            let desc = truncate_description(&s.description, 120);
            lines.push(format!("- **{}**: {}", s.name, desc));
        }
        Ok(ToolResult::text("list_tools", lines.join("\n")))
    }
}

fn truncate_description(desc: &str, max_len: usize) -> String {
    let first_line = desc.lines().next().unwrap_or(desc);
    if first_line.len() <= max_len {
        first_line.to_string()
    } else {
        format!("{}...", &first_line[..max_len])
    }
}
