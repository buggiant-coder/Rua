use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_usage::{TokenUsageFilter, TokenUsageQuery};

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::schema::ToolSpec;

#[derive(Debug, Deserialize)]
struct UsageArgs {
    #[serde(default = "default_days")]
    days: Option<u32>,
    #[serde(default)]
    model_name: Option<String>,
    #[serde(default)]
    provider_id: Option<String>,
}

fn default_days() -> Option<u32> {
    Some(30)
}

pub struct GetTokenUsageTool;

impl GetTokenUsageTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "token_usage".to_string(),
            description: "Query LLM token usage over the past N days. \
                          Use when the user asks about token consumption or API usage."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "days": {
                        "type": "integer",
                        "description": "Number of days to look back (default: 30, max: 365)"
                    },
                    "model_name": {
                        "type": "string",
                        "description": "Optional model name to filter by"
                    },
                    "provider_id": {
                        "type": "string",
                        "description": "Optional provider ID to filter by"
                    }
                }
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

impl Default for GetTokenUsageTool {
    fn default() -> Self {
        Self::new()
    }
}

fn format_number(n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let s = n.to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

#[async_trait]
impl Tool for GetTokenUsageTool {
    fn name(&self) -> &str {
        "token_usage"
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
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let args: UsageArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "token_usage".into(),
                message: e.to_string(),
            })?;

        let days = args.days.unwrap_or(30).clamp(1, 365);

        let query = TokenUsageQuery::new(ctx.working_dir);
        let summary = match query.query(&TokenUsageFilter {
            days: Some(days),
            model_name: args.model_name.clone(),
            provider_id: args.provider_id.clone(),
            ..Default::default()
        }) {
            Ok(summary) => summary,
            Err(e) => {
                return Ok(ToolResult::text(
                    "token_usage",
                    format!("Failed to read token_usage_ledger.json: {e}"),
                ));
            }
        };

        let today = Utc::now().date_naive();
        let start = today - chrono::Duration::days(i64::from(days));
        let total_prompt = summary.total_prompt_tokens;
        let total_completion = summary.total_completion_tokens;
        let total_calls = summary.total_calls;
        let by_model: BTreeMap<_, _> = summary.by_model.into_iter().collect();
        let by_date: BTreeMap<_, _> = summary.by_date.into_iter().collect();

        let mut filter_parts = Vec::new();
        if let Some(model) = &args.model_name {
            filter_parts.push(format!("model={model}"));
        }
        if let Some(provider) = &args.provider_id {
            filter_parts.push(format!("provider={provider}"));
        }
        if filter_parts.is_empty() {
            filter_parts.push("all models".into());
        }
        let filter_desc = filter_parts.join(", ");

        let total_tokens = total_prompt + total_completion;
        let mut lines = vec![
            format!("Token usage ({start} ~ {today}, {filter_desc}):"),
            String::new(),
            format!("- Total tokens: {}", format_number(total_tokens)),
            format!("- Prompt tokens: {}", format_number(total_prompt)),
            format!("- Completion tokens: {}", format_number(total_completion)),
            format!("- Total calls: {}", format_number(total_calls)),
            String::new(),
        ];

        if !by_model.is_empty() {
            lines.push("By model:".to_string());
            for (model, stats) in &by_model {
                let tokens = stats.prompt_tokens + stats.completion_tokens;
                lines.push(format!(
                    "  - {model}: {} tokens ({} calls)",
                    format_number(tokens),
                    format_number(stats.call_count)
                ));
            }
            lines.push(String::new());
        }

        if !by_date.is_empty() && by_date.len() <= 14 {
            lines.push("By date:".to_string());
            for (dt, stats) in by_date
                .iter()
                .rev()
                .take(7)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                let tokens = stats.prompt_tokens + stats.completion_tokens;
                lines.push(format!(
                    "  - {dt}: {} tokens ({} calls)",
                    format_number(tokens),
                    format_number(stats.call_count)
                ));
            }
        } else if !by_date.is_empty() {
            lines.push(format!(
                "By date: {} days with usage (showing last 7 days for brevity)",
                by_date.len()
            ));
            for (dt, stats) in by_date
                .iter()
                .rev()
                .take(7)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                let tokens = stats.prompt_tokens + stats.completion_tokens;
                lines.push(format!(
                    "  - {dt}: {} tokens ({} calls)",
                    format_number(tokens),
                    format_number(stats.call_count)
                ));
            }
        }

        if total_calls == 0 {
            lines.push("No token usage data in this period.".to_string());
        }

        Ok(ToolResult::text("token_usage", lines.join("\n")))
    }
}
